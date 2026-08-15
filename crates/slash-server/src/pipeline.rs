//! The spec §5 dispatch pipeline: the guard sequence from a webhook
//! `issue_comment` through `workflow_dispatch`. Guards run in the specified
//! order — cheap syntactic checks and the trust gate before any
//! user-visible side effect (spec §5).
//!
//! Scope note: this milestone implements the guard order and the full
//! happy path (through dispatch and run-id storage), plus the highest-value
//! rejection branches (permission denied/unresolved, fork, not-open,
//! config error, usage error, head-moved). Deliberately deferred to a
//! follow-up pass, and noted here rather than silently skipped: the §3.2
//! misplaced-command scan (needs config loaded before the first guard even
//! runs — see the comment at that guard), §6.7 supersede's check-run
//! summary update (the row is marked `superseded`; its check run is not yet
//! patched), per-PR comment dedup, and workflow-file input introspection
//! (all injected inputs plus args are sent unconditionally, which spec
//! explicitly allows as the fallback when the workflow file isn't
//! introspected).

use serde_json::{Map, Value as Json};
use slash_core::{ResolvedRole, TrustGate, messages};
use slash_github::octocrab_types::ReactionContent;
use slash_github::{GithubApp, RepoClient, WebhookEventPayload};
use sqlx::PgPool;
use uuid::Uuid;

use crate::catalog::{CatalogError, CatalogOutcome, load_catalog, resolve_default_branch};
use crate::invocations::{self, ClaimOutcome, NewInvocation};

/// The permissions requested for the least-privilege, per-repo installation
/// token (spec §7.5) — exactly what this pipeline's API calls need.
pub(crate) const TOKEN_PERMISSIONS: &[(&str, &str)] = &[
    ("contents", "read"),
    ("pull_requests", "write"),
    ("issues", "write"),
    ("checks", "write"),
    ("actions", "write"),
];

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("GitHub API error: {0}")]
    GitHub(#[from] slash_github::ClientError),
    #[error("GitHub App error: {0}")]
    Auth(#[from] slash_github::AppAuthError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub struct PipelineContext<'a> {
    pub app: &'a GithubApp,
    pub pool: &'a PgPool,
    pub installation_id: u64,
    pub repository_id: u64,
    pub owner: String,
    pub repo: String,
    /// Overrides `https://api.github.com` for every `RepoClient` this
    /// pipeline run constructs; used in tests to point at a mock server.
    pub base_uri: Option<&'a str>,
    pub metrics: &'a crate::metrics::Metrics,
}

/// Handles one `issue_comment` webhook event. Returns `Ok(())` for every
/// outcome, including guard rejections — those are terminal, user-visible
/// results, not pipeline errors. `Err` is reserved for infrastructure
/// failure (GitHub API, database).
pub async fn handle_issue_comment(
    ctx: &PipelineContext<'_>,
    event: &WebhookEventPayload,
) -> Result<(), PipelineError> {
    let WebhookEventPayload::IssueComment(payload) = event else {
        return Ok(());
    };

    use octocrab::models::webhook_events::payload::IssueCommentWebhookEventAction as Action;
    if payload.action != Action::Created {
        return Ok(());
    }

    // Bots (including Slash itself) are ignored (spec §3.1).
    if payload.comment.user.r#type == "Bot" {
        return Ok(());
    }

    // issue_comment fires for both issues and PRs; PR comments carry
    // `issue.pull_request` (spec §5.3).
    if payload.issue.pull_request.is_none() {
        return Ok(());
    }

    let body = payload.comment.body.clone().unwrap_or_default();
    if body.contains('\n') || body.contains('\r') {
        return Ok(());
    }
    let Ok(Some(parsed)) = slash_command::parse_comment(&body) else {
        // Not a command, or a syntax error in one: ignored silently (spec
        // §3.1). The §3.2 misplaced-command scan is deferred — it needs the
        // repository's configured command names, which requires loading
        // config before this guard runs, an ordering the spec's guard list
        // doesn't fully resolve and this pass doesn't attempt.
        return Ok(());
    };

    let token = ctx
        .app
        .installation_token(ctx.installation_id, ctx.repository_id, TOKEN_PERMISSIONS)
        .await?;
    let client =
        RepoClient::with_base_uri(&token, ctx.owner.clone(), ctx.repo.clone(), ctx.base_uri)?;

    // Resolve author permission (spec §5.2). Fail closed: any resolution
    // failure denies, with at most a best-effort 😕 reaction, never a
    // comment (the author's trust level is unknown).
    let permission = match client
        .get_collaborator_permission(&payload.comment.user.login)
        .await
    {
        Ok(permission) => permission,
        Err(error) => {
            log_permission_api_failure(
                "lookup",
                &ctx.owner,
                &ctx.repo,
                &payload.comment.user.login,
                payload.comment.id.0,
                &error,
            );
            if let Err(reaction_error) = client
                .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
                .await
            {
                log_permission_api_failure(
                    "reaction",
                    &ctx.owner,
                    &ctx.repo,
                    &payload.comment.user.login,
                    payload.comment.id.0,
                    &reaction_error,
                );
            }
            return Ok(());
        }
    };
    let role =
        slash_core::ResolvedRole::from_role_name(&permission.role_name).unwrap_or_else(|| {
            ResolvedRole::from_permission_booleans(
                permission.user.permissions.admin,
                permission.user.permissions.maintain,
                permission.user.permissions.push,
                permission.user.permissions.triage,
                permission.user.permissions.pull,
            )
        });

    // Every comment Slash posts from here on requires a trusted actor —
    // resolved to at least `read` (spec §6.4).
    let can_comment = role >= ResolvedRole::Read;

    let pr = client.get_pull_request(payload.issue.number).await?;

    if pr.state != Some(octocrab::models::IssueState::Open) {
        if can_comment {
            let _ = client
                .create_comment(payload.issue.number, &messages::pr_not_open())
                .await;
        }
        let _ = client
            .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
            .await;
        return Ok(());
    }

    let is_fork = match (&pr.head.repo, &pr.base.repo) {
        (Some(head_repo), Some(base_repo)) => head_repo.id != base_repo.id,
        _ => true,
    };
    if is_fork {
        if can_comment {
            let _ = client
                .create_comment(payload.issue.number, &messages::fork_unsupported())
                .await;
        }
        let _ = client
            .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
            .await;
        return Ok(());
    }

    let hinted_default_branch = pr
        .base
        .repo
        .as_ref()
        .and_then(|repo| repo.default_branch.as_deref());
    let resolved = match resolve_default_branch(&client, hinted_default_branch).await {
        Ok(resolved) => resolved,
        Err(error) => {
            report_catalog_error(
                ctx,
                &client,
                payload.issue.number,
                payload.comment.id.0,
                can_comment,
                &error,
            )
            .await;
            return Ok(());
        }
    };
    tracing::debug!(
        owner = %ctx.owner,
        repo = %ctx.repo,
        default_branch = %resolved.name,
        config_sha = %resolved.sha,
        "resolved command catalog snapshot"
    );
    let catalog = match load_catalog(&client, &resolved.sha).await {
        Ok(CatalogOutcome::Loaded(catalog)) => {
            ctx.metrics
                .command_catalog_loads_total
                .with_label_values(&["loaded", "complete"])
                .inc();
            catalog
        }
        Ok(CatalogOutcome::NotConfigured) => {
            ctx.metrics
                .command_catalog_loads_total
                .with_label_values(&["not_configured", "complete"])
                .inc();
            if can_comment {
                let _ = client
                    .create_comment(
                        payload.issue.number,
                        &messages::installed_but_not_configured(),
                    )
                    .await;
            }
            let _ = client
                .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
                .await;
            return Ok(());
        }
        Err(error) => {
            report_catalog_error(
                ctx,
                &client,
                payload.issue.number,
                payload.comment.id.0,
                can_comment,
                &error,
            )
            .await;
            return Ok(());
        }
    };

    let Some(validated) = catalog.find(&parsed.name) else {
        let names = catalog.names();
        if can_comment && slash_core::should_suggest_commands(&parsed.name, &names) {
            let _ = client
                .create_comment(
                    payload.issue.number,
                    &messages::unknown_command_suggestion(&parsed.name, &names),
                )
                .await;
            let _ = client
                .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
                .await;
        }
        return Ok(());
    };

    // Command authorization now runs through the R2 TrustGate (org/user M2-4
    // + #23): async preload of the actor's grants, then the sync grants
    // decision. Fail-closed: any load/decision error or a missing grant that
    // reaches the required tier denies. This replaces the GitHub-
    // collaborator-role comparison for dispatch.
    let github_user_id = payload.comment.user.id.0 as i64;
    let grants = crate::grants_loader::preload_grants(
        ctx.pool,
        github_user_id,
        ctx.installation_id as i64,
        &ctx.owner,
        &ctx.repo,
    )
    .await;
    let actor = slash_core::pipeline::Actor {
        login: payload.comment.user.login.clone(),
        github_user_id: payload.comment.user.id.0,
    };
    let outcome = match grants {
        Ok(grants) => {
            let gate = crate::grants_trust_gate::GrantsTrustGate;
            gate.check(&grants, &actor, &parsed.name, validated.permission)
        }
        // Fail closed: a preload DB/parse error is a deny (TrustOutcome::Error).
        Err(e) => slash_core::pipeline::TrustOutcome::Error(e.to_string()),
    };
    let authorized = outcome.is_granted();
    if !authorized {
        if can_comment {
            let required = match validated.permission {
                slash_config::Permission::Read => "read",
                slash_config::Permission::Write => "write",
                slash_config::Permission::Admin => "admin",
            };
            let _ = client
                .create_comment(
                    payload.issue.number,
                    &messages::permission_denied(&parsed.name, required),
                )
                .await;
        }
        let _ = client
            .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
            .await;
        return Ok(());
    }

    let bound = slash_config::bind(&parsed, validated);
    let bound = match bound {
        Ok(bound) => bound,
        Err(errors) => {
            if can_comment {
                let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
                let _ = client
                    .create_comment(
                        payload.issue.number,
                        &messages::usage_error(&parsed.name, &messages),
                    )
                    .await;
            }
            let _ = client
                .create_comment_reaction(payload.comment.id.0, ReactionContent::Confused)
                .await;
            return Ok(());
        }
    };

    let invocation_id = Uuid::new_v4();
    let mut args_json = Map::new();
    for (k, v) in &bound {
        args_json.insert(k.clone(), Json::String(v.clone()));
    }

    let new_invocation = NewInvocation {
        id: invocation_id,
        installation_id: ctx.installation_id as i64,
        repository_id: ctx.repository_id as i64,
        owner: &ctx.owner,
        repo: &ctx.repo,
        comment_id: payload.comment.id.0 as i64,
        attempt: 1,
        pr_number: payload.issue.number as i64,
        head_sha: &pr.head.sha,
        head_branch: &pr.head.ref_field,
        actor: &payload.comment.user.login,
        actor_id: payload.comment.user.id.0 as i64,
        command: &parsed.name,
        raw_comment_line: &parsed.raw_line,
        args: Json::Object(args_json),
        workflow_file: &validated.workflow,
    };

    let claim_outcome = invocations::claim(ctx.pool, &new_invocation).await?;
    let id = match claim_outcome {
        ClaimOutcome::Claimed(id) => id,
        ClaimOutcome::Resume(id) => id,
        // Later states are owned by workflow_run events and the sweeper.
        ClaimOutcome::AlreadyClaimed => return Ok(()),
    };

    let _ = client
        .create_comment_reaction(payload.comment.id.0, ReactionContent::Rocket)
        .await;

    let check_run_name = format!("slash/{}", parsed.name);
    let check_run = client
        .create_check_run(&check_run_name, &pr.head.sha, &id.to_string())
        .await?;
    invocations::set_check_run_id(ctx.pool, id, check_run.id.0 as i64).await?;

    supersede_older_invocations(ctx, &client, payload.issue.number, &parsed.name, id).await?;

    // Pre-dispatch tip re-read (spec §5, §8): the head may have moved since
    // the comment was captured.
    let current_pr = client.get_pull_request(payload.issue.number).await?;
    if current_pr.head.sha != pr.head.sha {
        invocations::transition_status(ctx.pool, id, slash_core::InvocationStatus::Aborted).await?;
        let _ = client
            .update_check_run(
                check_run.id.0,
                slash_github::CheckRunUpdate {
                    status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                    conclusion: Some(octocrab::params::checks::CheckRunConclusion::Neutral),
                    details_url: None,
                    output: Some(("Aborted", "The PR head moved, re-issue the command.")),
                },
            )
            .await;
        if can_comment {
            let _ = client
                .create_comment(payload.issue.number, &messages::head_moved())
                .await;
        }
        return Ok(());
    }

    invocations::transition_status(ctx.pool, id, slash_core::InvocationStatus::Dispatched).await?;

    let mut inputs = Map::new();
    for (k, v) in &bound {
        inputs.insert(k.clone(), Json::String(v.clone()));
    }
    inputs.insert("slash_run_id".to_string(), Json::String(id.to_string()));
    inputs.insert(
        "slash_pr_number".to_string(),
        Json::String(payload.issue.number.to_string()),
    );
    inputs.insert(
        "slash_head_sha".to_string(),
        Json::String(pr.head.sha.clone()),
    );
    inputs.insert(
        "slash_actor".to_string(),
        Json::String(payload.comment.user.login.clone()),
    );
    inputs.insert(
        "slash_actor_id".to_string(),
        Json::String(payload.comment.user.id.0.to_string()),
    );

    let dispatch_ref = format!("refs/heads/{}", pr.head.ref_field);
    match client
        .dispatch_workflow(&validated.workflow, &dispatch_ref, Json::Object(inputs))
        .await
    {
        Ok(outcome) => {
            invocations::set_workflow_run_id(ctx.pool, id, outcome.workflow_run_id as i64).await?;
            invocations::transition_status(ctx.pool, id, slash_core::InvocationStatus::Correlated)
                .await?;
            ctx.metrics
                .correlation_total
                .with_label_values(&["dispatch_response"])
                .inc();
            let _ = client
                .update_check_run(
                    check_run.id.0,
                    slash_github::CheckRunUpdate {
                        status: None,
                        conclusion: None,
                        details_url: Some(&outcome.html_url),
                        output: None,
                    },
                )
                .await;
        }
        Err(error) => {
            invocations::set_failure_reason(ctx.pool, id, &error.to_string()).await?;
            invocations::transition_status(
                ctx.pool,
                id,
                slash_core::InvocationStatus::DispatchFailed,
            )
            .await?;
            ctx.metrics
                .dispatch_failures_total
                .with_label_values(&["api_error"])
                .inc();
            let _ = client
                .update_check_run(
                    check_run.id.0,
                    slash_github::CheckRunUpdate {
                        status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                        conclusion: Some(octocrab::params::checks::CheckRunConclusion::Failure),
                        details_url: None,
                        output: Some(("Dispatch failed", "workflow_dispatch could not be sent.")),
                    },
                )
                .await;
        }
    }

    Ok(())
}

fn log_permission_api_failure(
    stage: &'static str,
    owner: &str,
    repo: &str,
    username: &str,
    comment_id: u64,
    error: &slash_github::ClientError,
) {
    tracing::warn!(
        stage,
        owner,
        repo,
        username,
        comment_id,
        status = ?error.status_code(),
        error = %error,
        "collaborator permission API failed"
    );
}

/// Records a failed command-catalog load: bumps the `command_catalog_loads_total`
/// counter and logs the failure. Shared by the pipeline's user-facing
/// [`report_catalog_error`] (which additionally posts a comment/reaction) and
/// the correlation module's re-run path, which has no comment surface (spec
/// §6.5). Pure observability — never raises, never writes to GitHub.
pub(crate) fn record_catalog_load_metrics(
    ctx: &PipelineContext<'_>,
    error: &CatalogError,
    message: &'static str,
) {
    let outcome = match error {
        CatalogError::Invalid { .. } => "invalid",
        CatalogError::Unavailable { .. } => "unavailable",
    };
    ctx.metrics
        .command_catalog_loads_total
        .with_label_values(&[outcome, error.stage()])
        .inc();
    tracing::warn!(
        owner = %ctx.owner,
        repo = %ctx.repo,
        stage = error.stage(),
        path = ?error.path(),
        status = ?error.status_code(),
        error = %error,
        "{message}"
    );
}

async fn report_catalog_error(
    ctx: &PipelineContext<'_>,
    client: &RepoClient,
    issue_number: u64,
    comment_id: u64,
    can_comment: bool,
    error: &CatalogError,
) {
    record_catalog_load_metrics(ctx, error, "command catalog load failed");

    if can_comment {
        let body = match error {
            CatalogError::Invalid { details } => messages::config_error(details),
            CatalogError::Unavailable { .. } => messages::command_catalog_unavailable(),
        };
        if let Err(feedback_error) = client.create_comment(issue_number, &body).await {
            tracing::warn!(error = %feedback_error, "failed to post command catalog feedback");
        }
    }
    if let Err(feedback_error) = client
        .create_comment_reaction(comment_id, ReactionContent::Confused)
        .await
    {
        tracing::warn!(error = %feedback_error, "failed to react to command catalog failure");
    }
}

async fn supersede_older_invocations(
    ctx: &PipelineContext<'_>,
    client: &RepoClient,
    pr_number: u64,
    command: &str,
    except_id: Uuid,
) -> Result<(), PipelineError> {
    let candidates = invocations::find_supersede_candidates(
        ctx.pool,
        ctx.installation_id as i64,
        &ctx.owner,
        &ctx.repo,
        pr_number as i64,
        command,
        except_id,
    )
    .await?;

    for candidate in candidates {
        invocations::transition_status(
            ctx.pool,
            candidate.id,
            slash_core::InvocationStatus::Superseded,
        )
        .await?;

        if let Some(check_run_id) = candidate.check_run_id {
            let _ = client
                .update_check_run(
                    check_run_id as u64,
                    slash_github::CheckRunUpdate {
                        status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                        conclusion: Some(octocrab::params::checks::CheckRunConclusion::Neutral),
                        details_url: None,
                        output: Some((
                            "Superseded",
                            "A newer invocation of this command supersedes this one.",
                        )),
                    },
                )
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use octocrab::models::webhook_events::payload::IssueCommentWebhookEventPayload;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use chrono::Utc;

    use super::*;
    use crate::db;
    use crate::metrics::Metrics;

    const TEST_KEY_PEM: &[u8] =
        include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem");

    #[derive(Clone)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CaptureWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn permission_failure_log_contains_diagnostic_context() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = CaptureWriter(Arc::clone(&output));
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let error = slash_github::ClientError::Api {
            message: "GitHub rejected permission lookup".to_string(),
            status: Some(403),
        };

        tracing::subscriber::with_default(subscriber, || {
            log_permission_api_failure("lookup", "acme", "widgets", "alice", 555, &error);
        });

        let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(output.contains("collaborator permission API failed"));
        assert!(output.contains("stage=\"lookup\""));
        assert!(output.contains("owner=\"acme\""));
        assert!(output.contains("repo=\"widgets\""));
        assert!(output.contains("username=\"alice\""));
        assert!(output.contains("comment_id=555"));
        assert!(output.contains("status=Some(403)"));
        assert!(output.contains("GitHub rejected permission lookup"));
    }

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE invocations, grants, org_members, team_members, teams, organizations, users CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    /// Seed the DB so `authorize_command_grants` (grants M2-4, strict
    /// deny-by-default) lets the GitHub actor invoke a write-tier command.
    async fn seed_dispatch_grant(pool: &PgPool, installation_id: i64, github_user_id: i64) {
        let org = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, installation_id, state)
             VALUES ($1, 'test-org', 'Test', $2, 'active')",
        )
        .bind(org)
        .bind(installation_id)
        .execute(pool).await.unwrap();
        let uid = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, status, github_user_id)
             VALUES ($1, 'alice@example.com', 'x', 'Alice', 'active', $2)",
        )
        .bind(uid)
        .bind(github_user_id)
        .execute(pool).await.unwrap();
        // org-scope write allow so any write-tier command in this install/new repo dispatches.
        sqlx::query(
            "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect)
             VALUES ($1, $2, 'user', $3, 'org', NULL, NULL, 'write', 'allow')",
        )
        .bind(uuid::Uuid::new_v4()).bind(org).bind(uid)
        .execute(pool).await.unwrap();
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

    fn issue_comment_payload(body: &str) -> WebhookEventPayload {
        let json = serde_json::json!({
            "action": "created",
            "comment": {
                "id": 555, "node_id": "n", "url": "https://api.github.com/repos/acme/widgets/issues/comments/555",
                "html_url": "https://github.com/acme/widgets/pull/7#issuecomment-555",
                "body": body,
                "user": author_json("alice", 1),
                "created_at": "2024-01-01T00:00:00Z"
            },
            "issue": {
                "id": 1, "node_id": "n", "url": "https://api.github.com/repos/acme/widgets/issues/7",
                "repository_url": "https://api.github.com/repos/acme/widgets",
                "labels_url": "https://api.github.com/repos/acme/widgets/issues/7/labels{/name}",
                "comments_url": "https://api.github.com/repos/acme/widgets/issues/7/comments",
                "events_url": "https://api.github.com/repos/acme/widgets/issues/7/events",
                "html_url": "https://github.com/acme/widgets/pull/7",
                "number": 7, "state": "open", "title": "A PR", "body": null,
                "user": author_json("alice", 1), "labels": [], "assignees": [], "locked": false,
                "comments": 1,
                "pull_request": {
                    "url": "https://api.github.com/repos/acme/widgets/pulls/7",
                    "html_url": "https://github.com/acme/widgets/pull/7",
                    "diff_url": "https://github.com/acme/widgets/pull/7.diff",
                    "patch_url": "https://github.com/acme/widgets/pull/7.patch"
                },
                "created_at": "2024-01-01T00:00:00Z", "updated_at": "2024-01-01T00:00:00Z"
            }
        });
        let payload: IssueCommentWebhookEventPayload = serde_json::from_value(json).unwrap();
        WebhookEventPayload::IssueComment(Box::new(payload))
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

    async fn mount_common(server: &MockServer, head_sha: &str) {
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/collaborators/alice/permission"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "permission": "push",
                "role_name": "write",
                "user": {
                    "login": "alice", "id": 1, "node_id": "n", "avatar_url": "https://avatars.githubusercontent.com/u/1",
                    "gravatar_id": "", "url": "https://api.github.com/users/alice", "html_url": "https://github.com/alice",
                    "followers_url": "https://api.github.com/users/alice/followers",
                    "following_url": "https://api.github.com/users/alice/following{/other_user}",
                    "gists_url": "https://api.github.com/users/alice/gists{/gist_id}",
                    "starred_url": "https://api.github.com/users/alice/starred{/owner}{/repo}",
                    "subscriptions_url": "https://api.github.com/users/alice/subscriptions",
                    "organizations_url": "https://api.github.com/users/alice/orgs",
                    "repos_url": "https://api.github.com/users/alice/repos",
                    "events_url": "https://api.github.com/users/alice/events{/privacy}",
                    "received_events_url": "https://api.github.com/users/alice/received_events",
                    "type": "User", "site_admin": false,
                    "permissions": {"admin": false, "push": true, "pull": true}
                },
            })))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pull_request_json(head_sha)))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ref": "refs/heads/main",
                "node_id": "REF_1",
                "url": "https://api.github.com/repos/acme/widgets/git/refs/heads/main",
                "object": {
                    "type": "commit",
                    "sha": "config-sha",
                    "url": "https://api.github.com/repos/acme/widgets/git/commits/config-sha"
                }
            })))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "config-sha"))
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
            .and(query_param("ref", "config-sha"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "echo.yml", "path": ".slash/echo.yml", "sha": "abc",
                "size": echo_yaml.len(), "url": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
                "type": "file", "encoding": "base64", "content": BASE64.encode(echo_yaml),
                "_links": {"self": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml", "git": null, "html": null}
            })))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/issues/comments/555/reactions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 1, "content": "rocket"
            })))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/check-runs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": head_sha,
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .mount(server)
            .await;

        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": head_sha,
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .mount(server)
            .await;
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

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn echo_happy_path_dispatches_and_stores_the_run_id() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // Grants M2-4: dispatch now requires a grant for the actor at the
        // command's tier (strict deny-by-default). Seed one for alice(1)
        // in install 1 so the write-tier echo command is allowed.
        seed_dispatch_grant(&pool, 1, 1).await;
        let server = MockServer::start().await;
        mount_common(&server, "deadbeef").await;

        Mock::given(method("POST"))
            .and(path(
                "/repos/acme/widgets/actions/workflows/echo.yml/dispatches",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "workflow_run_id": 999,
                "run_url": "https://api.github.com/repos/acme/widgets/actions/runs/999",
                "html_url": "https://github.com/acme/widgets/actions/runs/999"
            })))
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let event = issue_comment_payload("/echo hello");

        handle_issue_comment(&context, &event).await.unwrap();

        let row: (String, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT status, check_run_id, workflow_run_id FROM invocations WHERE comment_id = 555",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "correlated");
        assert_eq!(row.1, Some(55));
        assert_eq!(row.2, Some(999));
        assert_eq!(
            metrics
                .correlation_total
                .with_label_values(&["dispatch_response"])
                .get(),
            1
        );
    }

    #[tokio::test]
    async fn multiline_command_makes_no_github_requests() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        let server = MockServer::start().await;
        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();

        let result = handle_issue_comment(
            &ctx(&pool, &app, &server, &metrics),
            &issue_comment_payload("/echo hello\nsecond line"),
        )
        .await;

        assert!(result.is_ok());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn unavailable_catalog_gets_feedback_and_creates_no_invocation() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let server = MockServer::start().await;
        mount_common(&server, "deadbeef").await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "config-sha"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({"message": "Forbidden"})),
            )
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/issues/7/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 999,
                "node_id": "IC_999",
                "url": "https://api.github.com/repos/acme/widgets/issues/comments/999",
                "html_url": "https://github.com/acme/widgets/pull/7#issuecomment-999",
                "issue_url": "https://api.github.com/repos/acme/widgets/issues/7",
                "body": "catalog unavailable",
                "user": author_json("slash-app", 10),
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        handle_issue_comment(
            &ctx(&pool, &app, &server, &metrics),
            &issue_comment_payload("/echo hello"),
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            metrics
                .command_catalog_loads_total
                .with_label_values(&["unavailable", "directory"])
                .get(),
            1
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn invalid_catalog_is_reported_instead_of_partially_loaded() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let server = MockServer::start().await;
        mount_common(&server, "deadbeef").await;
        let invalid_yaml = "not: valid: yaml";
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash/echo.yml"))
            .and(query_param("ref", "config-sha"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "echo.yml", "path": ".slash/echo.yml", "sha": "abc",
                "size": invalid_yaml.len(),
                "url": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
                "type": "file", "encoding": "base64",
                "content": BASE64.encode(invalid_yaml),
                "_links": {
                    "self": "https://api.github.com/repos/acme/widgets/contents/.slash/echo.yml",
                    "git": null, "html": null
                }
            })))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/issues/7/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 999,
                "node_id": "IC_999",
                "url": "https://api.github.com/repos/acme/widgets/issues/comments/999",
                "html_url": "https://github.com/acme/widgets/pull/7#issuecomment-999",
                "issue_url": "https://api.github.com/repos/acme/widgets/issues/7",
                "body": "invalid catalog",
                "user": author_json("slash-app", 10),
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        handle_issue_comment(
            &ctx(&pool, &app, &server, &metrics),
            &issue_comment_payload("/echo hello"),
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            metrics
                .command_catalog_loads_total
                .with_label_values(&["invalid", "validation"])
                .get(),
            1
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn redelivering_the_same_comment_does_not_double_dispatch() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // Grants M2-4: seed a write-tier grant so both dispatches are allowed.
        seed_dispatch_grant(&pool, 1, 1).await;
        let server = MockServer::start().await;
        mount_common(&server, "deadbeef").await;
        Mock::given(method("POST"))
            .and(path(
                "/repos/acme/widgets/actions/workflows/echo.yml/dispatches",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "workflow_run_id": 999,
                "run_url": "https://api.github.com/repos/acme/widgets/actions/runs/999",
                "html_url": "https://github.com/acme/widgets/actions/runs/999"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let event = issue_comment_payload("/echo hello");

        handle_issue_comment(&context, &event).await.unwrap();
        handle_issue_comment(&context, &event).await.unwrap();

        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM invocations WHERE comment_id = 555")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_reserved_input_attack_is_rejected_before_any_side_effect() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let server = MockServer::start().await;
        // Only the token mint and permission/PR/config reads should ever be
        // hit — no reaction, check run, or dispatch call for a usage error.
        mount_common(&server, "deadbeef").await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        let context = ctx(&pool, &app, &server, &metrics);
        let event = issue_comment_payload("/echo --slash_head_sha=refs/pull/1/head");

        handle_issue_comment(&context, &event).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "a rejected --slash_* key must never reach a claimed invocation"
        );
    }
}
