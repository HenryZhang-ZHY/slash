//! Correlation and status sync (spec §6, plan M6): the `workflow_run`
//! handler that keeps a check run in sync with its run, `check_run`
//! (`rerequested`) for the re-run button, and `pull_request.synchronize`
//! recording a moved head SHA. Guards mirror `pipeline.rs`'s: guarded,
//! monotonic transitions before any GitHub-visible side effect.
//!
//! The sweeper's polling fallbacks (missing-run-id, the 10-minute dispatch
//! deadline, the 72h run deadline — spec §6.3) live in `sweeper.rs`, not
//! here; [`apply_completed_run`] is the piece they share with this module's
//! webhook path, since both ultimately need to write the same terminal
//! conclusion from a freshly re-fetched run. Deliberately deferred and
//! flagged rather than silently skipped: failed-check-run-update retry.

use octocrab::models::webhook_events::payload::{
    CheckRunWebhookEventAction, CheckRunWebhookEventPayload, PullRequestWebhookEventAction,
    PullRequestWebhookEventPayload, WorkflowRunWebhookEventAction, WorkflowRunWebhookEventPayload,
};
use octocrab::params::checks::{CheckRunConclusion, CheckRunStatus};
use slash_core::{CheckConclusion, InvocationStatus, ResolvedRole, messages};
use slash_github::{CheckRunUpdate, RepoClient, WebhookEvent, WorkflowRun};
use sqlx::PgPool;

use crate::invocations::{self, Invocation, NewInvocation};
use crate::pipeline::{PipelineContext, PipelineError, TOKEN_PERMISSIONS};

pub(crate) fn to_octocrab_conclusion(conclusion: CheckConclusion) -> CheckRunConclusion {
    match conclusion {
        CheckConclusion::Success => CheckRunConclusion::Success,
        CheckConclusion::Failure => CheckRunConclusion::Failure,
        CheckConclusion::Cancelled => CheckRunConclusion::Cancelled,
        CheckConclusion::TimedOut => CheckRunConclusion::TimedOut,
        CheckConclusion::ActionRequired => CheckRunConclusion::ActionRequired,
        CheckConclusion::Neutral => CheckRunConclusion::Neutral,
    }
}

/// Writes a freshly re-fetched, `completed` run's outcome to both the
/// invocation row and its check run (spec §6.3's re-fetch-before-terminal
/// rule). Shared by the `workflow_run.completed` webhook path and the
/// sweeper's lost-webhook/run-deadline polling — both call this only after
/// confirming, via their own means, that the run has actually completed.
pub(crate) async fn apply_completed_run(
    pool: &PgPool,
    client: &RepoClient,
    invocation: &Invocation,
    fresh: &WorkflowRun,
) -> Result<(), PipelineError> {
    let transitioned =
        invocations::transition_status(pool, invocation.id, InvocationStatus::Completed).await?;
    if !transitioned {
        return Ok(());
    }

    let (conclusion, raw) = slash_core::map_conclusion(fresh.conclusion.as_deref());
    invocations::set_conclusion(pool, invocation.id, conclusion.as_str()).await?;
    invocations::set_last_reported_status(pool, invocation.id, "completed").await?;

    if let Some(check_run_id) = invocation.check_run_id {
        let head_sha_mismatch = fresh.head_sha != invocation.head_sha;
        let duration = fresh
            .run_started_at
            .map(|started| (fresh.created_at - started).num_seconds().abs());
        let mut summary = messages::check_run_summary(
            &invocation.raw_comment_line,
            &invocation.actor,
            &fresh.html_url,
            duration,
            head_sha_mismatch,
        );
        if let Some(raw_value) = raw {
            summary.push_str(&format!(
                "\nRaw conclusion: {}",
                messages::escape_user_text(&raw_value)
            ));
        }

        let _ = client
            .update_check_run(
                check_run_id as u64,
                CheckRunUpdate {
                    status: Some(CheckRunStatus::Completed),
                    conclusion: Some(to_octocrab_conclusion(conclusion)),
                    details_url: Some(&fresh.html_url),
                    output: Some(("Result", &summary)),
                },
            )
            .await;
    }

    Ok(())
}

/// Handles one `workflow_run` webhook event: exact match by run id (spec
/// §6.3), a guarded transition, and — only on `completed` — a re-fetch of
/// the run before writing a terminal conclusion, never trusting the webhook
/// body directly.
pub async fn handle_workflow_run(
    ctx: &PipelineContext<'_>,
    payload: &WorkflowRunWebhookEventPayload,
) -> Result<(), PipelineError> {
    let Ok(run) = serde_json::from_value::<WorkflowRun>(payload.workflow_run.clone()) else {
        return Ok(());
    };

    let Some(invocation) = invocations::find_by_workflow_run_id(
        ctx.pool,
        ctx.installation_id as i64,
        &ctx.owner,
        &ctx.repo,
        run.id as i64,
    )
    .await?
    else {
        // Not ours: never correlated (an ambiguous dispatch outcome, left
        // for the sweeper's polling fallback), or a human-started run of
        // the same workflow — never claimed by run id alone.
        return Ok(());
    };

    let Some(status) = InvocationStatus::parse(&invocation.status) else {
        return Ok(());
    };
    if status.is_terminal() {
        // Stale event after a terminal state, a duplicate `completed`, or
        // `completed` having already arrived before this `in_progress`
        // (spec §6.2's monotonic guarantee) — dropped, not reprocessed.
        return Ok(());
    }

    let token = ctx
        .app
        .installation_token(ctx.installation_id, ctx.repository_id, TOKEN_PERMISSIONS)
        .await?;
    let client =
        RepoClient::with_base_uri(&token, ctx.owner.clone(), ctx.repo.clone(), ctx.base_uri)?;

    match payload.action {
        WorkflowRunWebhookEventAction::Completed => {
            // Re-fetch before writing a terminal conclusion (spec §6.3).
            let fresh = client.get_workflow_run(run.id).await?;
            apply_completed_run(ctx.pool, &client, &invocation, &fresh).await?;
        }
        WorkflowRunWebhookEventAction::InProgress | WorkflowRunWebhookEventAction::Requested => {
            let target = if payload.action == WorkflowRunWebhookEventAction::InProgress {
                "in_progress"
            } else {
                "queued"
            };
            if invocation.last_reported_status.as_deref() == Some(target) {
                return Ok(()); // already reported; updates are idempotent (spec §7.2)
            }
            invocations::set_last_reported_status(ctx.pool, invocation.id, target).await?;

            if let Some(check_run_id) = invocation.check_run_id {
                let status = if target == "in_progress" {
                    CheckRunStatus::InProgress
                } else {
                    CheckRunStatus::Queued
                };
                let _ = client
                    .update_check_run(
                        check_run_id as u64,
                        CheckRunUpdate {
                            status: Some(status),
                            conclusion: None,
                            details_url: None,
                            output: None,
                        },
                    )
                    .await;
            }
        }
        // Non-exhaustive enum (octocrab may add actions); nothing else is
        // meaningful for correlation.
        _ => {}
    }

    Ok(())
}

/// Handles `check_run.rerequested` (spec §6.5): re-resolve the
/// *rerequester's* permission — never the original invoker's — against the
/// command's current configuration, claim a fresh invocation row for the
/// same comment at the next attempt, and run the normal pipeline from the
/// claim onward.
pub async fn handle_check_run_rerequested(
    ctx: &PipelineContext<'_>,
    event: &WebhookEvent,
) -> Result<(), PipelineError> {
    let slash_github::WebhookEventPayload::CheckRun(payload) = &event.specific else {
        return Ok(());
    };
    let CheckRunWebhookEventPayload {
        action: CheckRunWebhookEventAction::Rerequested,
        check_run,
        ..
    } = payload.as_ref()
    else {
        return Ok(());
    };

    #[derive(serde::Deserialize)]
    struct MinimalCheckRun {
        id: u64,
        external_id: Option<String>,
    }
    let Ok(check_run) = serde_json::from_value::<MinimalCheckRun>(check_run.clone()) else {
        return Ok(());
    };

    let Some(original) = invocations::find_by_check_run_id(
        ctx.pool,
        ctx.installation_id as i64,
        &ctx.owner,
        &ctx.repo,
        check_run.id as i64,
    )
    .await?
    else {
        // A stale or unknown check_run_id: nothing to re-run.
        return Ok(());
    };
    let _ = check_run.external_id;

    let Some(rerequester) = &event.sender else {
        return Ok(());
    };

    let token = ctx
        .app
        .installation_token(ctx.installation_id, ctx.repository_id, TOKEN_PERMISSIONS)
        .await?;
    let client =
        RepoClient::with_base_uri(&token, ctx.owner.clone(), ctx.repo.clone(), ctx.base_uri)?;

    // Re-resolve the rerequester's permission (never the original
    // invoker's). Fail closed with no comment surface here (spec §5.2,
    // §6.5): a resolution failure is dropped.
    let Ok(permission) = client.get_collaborator_permission(&rerequester.login).await else {
        return Ok(());
    };
    let role = ResolvedRole::from_role_name(&permission.role_name).unwrap_or_else(|| {
        ResolvedRole::from_permission_booleans(
            permission.user.permissions.admin,
            permission.user.permissions.maintain,
            permission.user.permissions.push,
            permission.user.permissions.triage,
            permission.user.permissions.pull,
        )
    });

    // Re-capture the PR head and re-check the command's *current*
    // permission requirement (config may have changed since the original
    // invocation).
    let pr = client.get_pull_request(original.pr_number as u64).await?;
    let default_branch = pr
        .base
        .repo
        .as_ref()
        .and_then(|r| r.default_branch.clone())
        .unwrap_or_else(|| "main".to_string());
    let commands = match client.get_content(".slash", &default_branch).await {
        Ok(files) => crate::pipeline::load_commands(&files, &client, &default_branch).await,
        Err(_) => Vec::new(),
    };
    let Some((_, validated)) = commands.iter().find(|(name, _)| *name == original.command) else {
        return Ok(());
    };

    if !slash_core::meets(role, validated.permission) {
        // No comment surface for a denied re-run (spec §6.5); the check
        // run itself communicates the denial.
        let _ = client
            .update_check_run(
                check_run.id,
                CheckRunUpdate {
                    status: Some(CheckRunStatus::Completed),
                    conclusion: Some(CheckRunConclusion::ActionRequired),
                    details_url: None,
                    output: Some((
                        "Re-run denied",
                        &messages::rerequest_permission_denied(&original.command, "write"),
                    )),
                },
            )
            .await;
        return Ok(());
    }

    let new_id = uuid::Uuid::new_v4();
    let new_invocation = NewInvocation {
        id: new_id,
        installation_id: ctx.installation_id as i64,
        repository_id: ctx.repository_id as i64,
        owner: &ctx.owner,
        repo: &ctx.repo,
        comment_id: original.comment_id,
        attempt: original.attempt + 1,
        pr_number: original.pr_number,
        head_sha: &pr.head.sha,
        head_branch: &pr.head.ref_field,
        actor: &rerequester.login,
        actor_id: rerequester.id.0 as i64,
        command: &original.command,
        raw_comment_line: &original.raw_comment_line,
        args: serde_json::Value::Object(serde_json::Map::new()),
        workflow_file: &original.workflow_file,
    };

    let claim_outcome = invocations::claim(ctx.pool, &new_invocation).await?;
    let invocations::ClaimOutcome::Claimed(id) = claim_outcome else {
        return Ok(());
    };

    let check_run_name = format!("slash/{}", original.command);
    let new_check_run = client
        .create_check_run(&check_run_name, &pr.head.sha, &id.to_string())
        .await?;
    invocations::set_check_run_id(ctx.pool, id, new_check_run.id.0 as i64).await?;
    invocations::transition_status(ctx.pool, id, InvocationStatus::Dispatched).await?;

    let dispatch_ref = format!("refs/heads/{}", pr.head.ref_field);
    let inputs = serde_json::json!({
        "slash_run_id": id.to_string(),
        "slash_pr_number": original.pr_number.to_string(),
        "slash_head_sha": pr.head.sha,
        "slash_actor": rerequester.login,
        "slash_actor_id": rerequester.id.0.to_string(),
    });

    if let Ok(outcome) = client
        .dispatch_workflow(&original.workflow_file, &dispatch_ref, inputs)
        .await
    {
        invocations::set_workflow_run_id(ctx.pool, id, outcome.workflow_run_id as i64).await?;
        invocations::transition_status(ctx.pool, id, InvocationStatus::Correlated).await?;
        ctx.metrics
            .correlation_total
            .with_label_values(&["dispatch_response"])
            .inc();
    }

    Ok(())
}

/// `pull_request.synchronize` (spec §7.3): records the moved head SHA on
/// the PR's open invocations, purely for the eventual completion summary to
/// note "the branch moved after this command was issued" — it never
/// re-triggers anything (spec §2.4).
pub async fn handle_pull_request_synchronize(
    ctx: &PipelineContext<'_>,
    payload: &PullRequestWebhookEventPayload,
) -> Result<(), PipelineError> {
    if payload.action != PullRequestWebhookEventAction::Synchronize {
        return Ok(());
    }

    invocations::record_new_head_sha(
        ctx.pool,
        ctx.installation_id as i64,
        &ctx.owner,
        &ctx.repo,
        payload.number as i64,
        &payload.pull_request.head.sha,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use octocrab::models::webhook_events::payload::{
        PullRequestWebhookEventPayload, WorkflowRunWebhookEventPayload,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::db;
    use crate::invocations::{ClaimOutcome, NewInvocation};
    use crate::metrics::Metrics;
    use slash_github::GithubApp;
    use sqlx::PgPool;
    use uuid::Uuid;

    const TEST_KEY_PEM: &[u8] =
        include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem");

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE invocations")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    fn author_json(login: &str, id: u64) -> serde_json::Value {
        serde_json::json!({
            "login": login, "id": id, "node_id": "n", "avatar_url": "https://avatars.githubusercontent.com/u/1",
            "gravatar_id": "", "url": "https://api.github.com/users/x", "html_url": "https://github.com/x",
            "followers_url": "https://api.github.com/users/x/followers",
            "following_url": "https://api.github.com/users/x/following{/other_user}",
            "gists_url": "https://api.github.com/users/x/gists{/gist_id}",
            "starred_url": "https://api.github.com/users/x/starred{/owner}{/repo}",
            "subscriptions_url": "https://api.github.com/users/x/subscriptions",
            "organizations_url": "https://api.github.com/users/x/orgs",
            "repos_url": "https://api.github.com/users/x/repos",
            "events_url": "https://api.github.com/users/x/events{/privacy}",
            "received_events_url": "https://api.github.com/users/x/received_events",
            "type": "User", "site_admin": false
        })
    }

    fn pull_request_json(head_sha: &str) -> serde_json::Value {
        serde_json::json!({
            "id": 1, "number": 7, "state": "open",
            "url": "https://api.github.com/repos/acme/widgets/pulls/7",
            "head": {
                "ref": "feature", "sha": head_sha,
                "repo": {
                    "id": 100, "name": "widgets", "owner": author_json("acme", 2),
                    "url": "https://api.github.com/repos/acme/widgets"
                }
            },
            "base": {
                "ref": "main", "sha": "basesha",
                "repo": {
                    "id": 100, "name": "widgets", "owner": author_json("acme", 2), "default_branch": "main",
                    "url": "https://api.github.com/repos/acme/widgets"
                }
            }
        })
    }

    fn sample(id: Uuid) -> NewInvocation<'static> {
        NewInvocation {
            id,
            installation_id: 1,
            repository_id: 100,
            owner: "acme",
            repo: "widgets",
            comment_id: 100,
            attempt: 1,
            pr_number: 7,
            head_sha: "deadbeef",
            head_branch: "feature",
            actor: "alice",
            actor_id: 1,
            command: "echo",
            raw_comment_line: "/echo hi",
            args: serde_json::json!({}),
            workflow_file: "echo.yml",
        }
    }

    fn ctx<'a>(
        pool: &'a PgPool,
        app: &'a GithubApp,
        server: &'a MockServer,
        metrics: &'a Metrics,
    ) -> PipelineContext<'a> {
        PipelineContext {
            app,
            pool,
            installation_id: 1,
            repository_id: 100,
            owner: "acme".to_string(),
            repo: "widgets".to_string(),
            base_uri: Some(server.uri().leak()),
            metrics,
        }
    }

    async fn mount_token(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(server)
            .await;
    }

    fn workflow_run_json(run_id: u64, status: &str, conclusion: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "id": run_id, "status": status, "conclusion": conclusion,
            "head_sha": "deadbeef", "head_branch": "feature", "event": "workflow_dispatch",
            "html_url": format!("https://github.com/acme/widgets/actions/runs/{run_id}"),
            "created_at": "2024-01-01T00:00:00Z",
            "run_started_at": "2024-01-01T00:00:05Z",
            "triggering_actor": {"login": "slash[bot]"}
        })
    }

    fn workflow_run_payload(
        run_id: u64,
        action: &str,
        status: &str,
        conclusion: Option<&str>,
    ) -> WorkflowRunWebhookEventPayload {
        let json = serde_json::json!({
            "action": action,
            "enterprise": null,
            "workflow": null,
            "workflow_run": workflow_run_json(run_id, status, conclusion),
        });
        serde_json::from_value(json).unwrap()
    }

    use chrono::Utc;

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn workflow_run_completed_transitions_to_completed_and_stores_conclusion() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        let ClaimOutcome::Claimed(_) = invocations::claim(&pool, &sample(id)).await.unwrap() else {
            panic!("expected a fresh claim");
        };
        invocations::transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();
        invocations::set_workflow_run_id(&pool, id, 999)
            .await
            .unwrap();

        let server = MockServer::start().await;
        mount_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/runs/999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(workflow_run_json(
                999,
                "completed",
                Some("success"),
            )))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "success",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let payload = workflow_run_payload(999, "completed", "completed", Some("success"));

        handle_workflow_run(&context, &payload).await.unwrap();

        let row: (String, Option<String>) =
            sqlx::query_as("SELECT status, conclusion FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "completed");
        assert_eq!(row.1.as_deref(), Some("success"));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn workflow_run_in_progress_updates_the_check_run_only_once() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();
        invocations::set_workflow_run_id(&pool, id, 999)
            .await
            .unwrap();

        let server = MockServer::start().await;
        mount_token(&server).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let payload = workflow_run_payload(999, "in_progress", "in_progress", None);

        handle_workflow_run(&context, &payload).await.unwrap();
        handle_workflow_run(&context, &payload).await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn workflow_run_for_an_unknown_run_id_is_ignored() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // No mocks mounted at all: a match miss must never mint a token or
        // call out to GitHub.
        let server = MockServer::start().await;
        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let payload = workflow_run_payload(4242, "completed", "completed", Some("success"));

        handle_workflow_run(&context, &payload).await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn workflow_run_a_duplicate_completed_event_is_dropped_without_a_second_api_call() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();
        invocations::set_workflow_run_id(&pool, id, 999)
            .await
            .unwrap();

        let server = MockServer::start().await;
        mount_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/runs/999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(workflow_run_json(
                999,
                "completed",
                Some("success"),
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "success",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let payload = workflow_run_payload(999, "completed", "completed", Some("success"));

        // First delivery applies normally.
        handle_workflow_run(&context, &payload).await.unwrap();
        // A redelivery of the same terminal event must be dropped by the
        // top-of-function `is_terminal()` guard before minting a token or
        // re-fetching — proven by `.expect(1)` on both mocks above, which
        // would fail the test if either were hit a second time.
        handle_workflow_run(&context, &payload).await.unwrap();

        let row: (String, Option<String>) =
            sqlx::query_as("SELECT status, conclusion FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "completed");
        assert_eq!(row.1.as_deref(), Some("success"));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn workflow_run_in_progress_arriving_after_completed_is_dropped() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();
        invocations::set_workflow_run_id(&pool, id, 999)
            .await
            .unwrap();

        let server = MockServer::start().await;
        mount_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/runs/999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(workflow_run_json(
                999,
                "completed",
                Some("success"),
            )))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "success",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);

        // `completed` arrives and is applied first.
        let completed = workflow_run_payload(999, "completed", "completed", Some("success"));
        handle_workflow_run(&context, &completed).await.unwrap();

        // A straggling `in_progress` for the same run, arriving out of
        // order, must be dropped by the terminal-state guard rather than
        // treated as a fresh status to report — proven by `.expect(1)` on
        // the PATCH mock above, which only the `completed` call may satisfy.
        let in_progress = workflow_run_payload(999, "in_progress", "in_progress", None);
        handle_workflow_run(&context, &in_progress).await.unwrap();

        let row: (String, Option<String>) =
            sqlx::query_as("SELECT status, last_reported_status FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "completed");
        assert_eq!(
            row.1.as_deref(),
            Some("completed"),
            "the late in_progress must not overwrite the terminal last_reported_status"
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn workflow_run_updates_for_a_superseded_invocation_are_dropped() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        invocations::transition_status(&pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();
        invocations::set_workflow_run_id(&pool, id, 999)
            .await
            .unwrap();
        // A newer attempt on the same PR/command superseded this one before
        // its run reported anything terminal (spec §6.7).
        invocations::transition_status(&pool, id, InvocationStatus::Superseded)
            .await
            .unwrap();

        // No mocks mounted at all: a superseded invocation must never mint a
        // token or call out to GitHub for a late webhook.
        let server = MockServer::start().await;
        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let payload = workflow_run_payload(999, "completed", "completed", Some("success"));

        handle_workflow_run(&context, &payload).await.unwrap();

        let status: (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "superseded");
    }

    async fn mount_rerequest_common(server: &MockServer) {
        mount_token(server).await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pull_request_json("deadbeef")))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "name": "echo.yml", "path": ".slash/echo.yml", "sha": "abc",
                    "size": 10, "url": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
                    "type": "file",
                    "_links": {"self": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml", "git": null, "html": null}
                }
            ])))
            .mount(server)
            .await;

        let echo_yaml = "command: echo\nworkflow: echo.yml\nargs:\n  - name: message\n";
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash/echo.yml"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "echo.yml", "path": ".slash/echo.yml", "sha": "abc",
                "size": echo_yaml.len(), "url": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
                "type": "file", "encoding": "base64", "content": BASE64.encode(echo_yaml),
                "_links": {"self": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml", "git": null, "html": null}
            })))
            .mount(server)
            .await;
    }

    fn check_run_rerequested_event() -> WebhookEvent {
        let json = serde_json::json!({
            "action": "rerequested",
            "check_run": {"id": 55, "external_id": null},
            "sender": author_json("bob", 9),
            "repository": null,
            "organization": null,
            "installation": null,
        });
        slash_github::parse_webhook_event("check_run", json.to_string().as_bytes()).unwrap()
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn check_run_rerequested_reissues_at_the_next_attempt_when_permitted() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        mount_rerequest_common(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/collaborators/bob/permission"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "permission": "push", "role_name": "write",
                "user": {
                    "login": "bob", "id": 9, "node_id": "n", "avatar_url": "https://avatars.githubusercontent.com/u/9",
                    "gravatar_id": "", "url": "https://api.github.com/users/bob", "html_url": "https://github.com/bob",
                    "followers_url": "https://api.github.com/users/bob/followers",
                    "following_url": "https://api.github.com/users/bob/following{/other_user}",
                    "gists_url": "https://api.github.com/users/bob/gists{/gist_id}",
                    "starred_url": "https://api.github.com/users/bob/starred{/owner}{/repo}",
                    "subscriptions_url": "https://api.github.com/users/bob/subscriptions",
                    "organizations_url": "https://api.github.com/users/bob/orgs",
                    "repos_url": "https://api.github.com/users/bob/repos",
                    "events_url": "https://api.github.com/users/bob/events{/privacy}",
                    "received_events_url": "https://api.github.com/users/bob/received_events",
                    "type": "User", "site_admin": false,
                    "permissions": {"admin": false, "push": true, "pull": true}
                },
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/check-runs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 56, "node_id": "n2", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/56",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/repos/acme/widgets/actions/workflows/echo.yml/dispatches",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "workflow_run_id": 1000,
                "run_url": "https://api.github.com/repos/acme/widgets/actions/runs/1000",
                "html_url": "https://github.com/acme/widgets/actions/runs/1000"
            })))
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let event = check_run_rerequested_event();

        handle_check_run_rerequested(&context, &event)
            .await
            .unwrap();

        let row: (i32, String, Option<i64>) = sqlx::query_as(
            "SELECT attempt, status, workflow_run_id FROM invocations WHERE comment_id = 100 AND attempt = 2",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, 2);
        assert_eq!(row.1, "correlated");
        assert_eq!(row.2, Some(1000));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn check_run_rerequested_is_denied_for_a_read_only_rerequester() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        mount_rerequest_common(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/collaborators/bob/permission"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "permission": "pull", "role_name": "read",
                "user": {
                    "login": "bob", "id": 9, "node_id": "n", "avatar_url": "https://avatars.githubusercontent.com/u/9",
                    "gravatar_id": "", "url": "https://api.github.com/users/bob", "html_url": "https://github.com/bob",
                    "followers_url": "https://api.github.com/users/bob/followers",
                    "following_url": "https://api.github.com/users/bob/following{/other_user}",
                    "gists_url": "https://api.github.com/users/bob/gists{/gist_id}",
                    "starred_url": "https://api.github.com/users/bob/starred{/owner}{/repo}",
                    "subscriptions_url": "https://api.github.com/users/bob/subscriptions",
                    "organizations_url": "https://api.github.com/users/bob/orgs",
                    "repos_url": "https://api.github.com/users/bob/repos",
                    "events_url": "https://api.github.com/users/bob/events{/privacy}",
                    "received_events_url": "https://api.github.com/users/bob/received_events",
                    "type": "User", "site_admin": false,
                    "permissions": {"admin": false, "push": false, "pull": true}
                },
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "action_required",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let event = check_run_rerequested_event();

        handle_check_run_rerequested(&context, &event)
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "a denied re-run must never claim a new invocation row"
        );
    }

    fn pull_request_synchronize_payload(
        pr_number: u64,
        new_head_sha: &str,
    ) -> PullRequestWebhookEventPayload {
        let json = serde_json::json!({
            "action": "synchronize",
            "assignee": null,
            "enterprise": null,
            "number": pr_number,
            "pull_request": pull_request_json(new_head_sha),
            "reason": null,
            "milestone": null,
            "label": null,
            "after": null,
            "before": "deadbeef",
            "changes": null,
            "requested_reviewer": null,
            "requested_team": null,
        });
        serde_json::from_value(json).unwrap()
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn pull_request_synchronize_records_the_new_head_sha_on_open_invocations_only() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let open_id = Uuid::new_v4();
        invocations::claim(&pool, &sample(open_id)).await.unwrap();

        let mut terminal = sample(Uuid::new_v4());
        terminal.comment_id = 200;
        let terminal_id = terminal.id;
        invocations::claim(&pool, &terminal).await.unwrap();
        invocations::transition_status(&pool, terminal_id, InvocationStatus::Aborted)
            .await
            .unwrap();

        let server = MockServer::start().await;
        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let payload = pull_request_synchronize_payload(7, "newsha");

        handle_pull_request_synchronize(&context, &payload)
            .await
            .unwrap();

        let open_row: (String,) = sqlx::query_as("SELECT head_sha FROM invocations WHERE id = $1")
            .bind(open_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(open_row.0, "newsha");

        let terminal_row: (String,) =
            sqlx::query_as("SELECT head_sha FROM invocations WHERE id = $1")
                .bind(terminal_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            terminal_row.0, "deadbeef",
            "a terminal invocation's head_sha must not move"
        );
    }
}
