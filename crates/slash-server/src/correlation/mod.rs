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
//!
//! Split (R2 #29): each webhook handler lives in its own submodule;
//! this module holds the shared helpers, the re-exports, and the test
//! suite (which exercises every handler).

mod rerequest;
mod synchronize;
mod workflow_run;

pub(crate) use rerequest::handle_check_run_rerequested;
pub(crate) use synchronize::handle_pull_request_synchronize;
pub(crate) use workflow_run::{apply_completed_run, handle_workflow_run};

use octocrab::params::checks::CheckRunConclusion;
use slash_core::CheckConclusion;

use crate::catalog::CatalogError;
use crate::pipeline::PipelineContext;

/// Maps a `slash_core::CheckConclusion` to the octocrab check-run conclusion
/// enum GitHub's API accepts. Shared by the `workflow_run.completed` path
/// and the sweeper's re-fetch terminal writes.
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

pub(crate) fn record_catalog_error(
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
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::db;
    use crate::invocations;
    use crate::invocations::{ClaimOutcome, NewInvocation};
    use crate::metrics::Metrics;
    use slash_core::InvocationStatus;
    use slash_github::GithubApp;
    use slash_github::WebhookEvent;
    use sqlx::PgPool;
    use uuid::Uuid;

    const TEST_KEY_PEM: &[u8] =
        include_bytes!("../../../slash-github/tests/fixtures/test-app-key.pem");

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

    /// Seed the DB so `preload_grants` + `GrantsTrustGate` let the
    /// rerequester (GitHub user id `github_user_id`) re-run a write-tier
    /// command in `installation_id`'s org (grants M2-4 strict deny-by-default).
    async fn seed_rerequest_grant(pool: &PgPool, installation_id: i64, github_user_id: i64) {
        let org = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, installation_id, state)
             VALUES ($1, 'test-org', 'Test', $2, 'active')",
        )
        .bind(org)
        .bind(installation_id)
        .execute(pool)
        .await
        .unwrap();
        let uid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, status, github_user_id)
             VALUES ($1, 'bob@example.com', 'x', 'Bob', 'active', $2)",
        )
        .bind(uid)
        .bind(github_user_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect)
             VALUES ($1, $2, 'user', $3, 'org', NULL, NULL, 'write', 'allow')",
        )
        .bind(Uuid::new_v4()).bind(org).bind(uid)
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
        // Grants-backed authorization (M3 #23): the rerequester needs an org
        // write grant, not a GitHub collaborator role.
        seed_rerequest_grant(&pool, 1, 9).await;
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        mount_rerequest_common(&server).await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/check-runs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 56, "node_id": "n2", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/56",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
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
            .expect(1)
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
    async fn rerequest_with_unavailable_catalog_is_denied_without_a_new_invocation() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        mount_rerequest_common(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/contents/.slash"))
            .and(query_param("ref", "config-sha"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({"message": "GitHub unavailable"})),
            )
            .with_priority(1)
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
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        handle_check_run_rerequested(
            &ctx(&pool, &app, &server, &metrics),
            &check_run_rerequested_event(),
        )
        .await
        .unwrap();

        let attempts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM invocations WHERE comment_id = 100")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(attempts, 1);
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
    async fn check_run_rerequested_is_denied_for_a_rerequester_without_a_grant() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // No grant is seeded: grants-backed authorization denies by default
        // (fail-closed, spec §5.2). This also pins the fix for the previous
        // collaborator-role check, which would have allowed a read-only
        // collaborator with no grant to re-run.
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        mount_rerequest_common(&server).await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "action_required",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
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

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn check_run_rerequested_allows_a_granted_rerequester_without_a_collaborator_role() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // The rerequester has an org write grant but is NOT a repo
        // collaborator. The grants-backed decision (M3 #23) allows the re-run;
        // the old collaborator-role check would have denied it. No
        // collaborators/:permission mock is mounted, proving the decision no
        // longer consults the GitHub collaborator role at all.
        seed_rerequest_grant(&pool, 1, 9).await;
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        mount_rerequest_common(&server).await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/widgets/check-runs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 56, "node_id": "n2", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/56",
                "html_url": null, "details_url": null, "conclusion": null,
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
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
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        handle_check_run_rerequested(
            &ctx(&pool, &app, &server, &metrics),
            &check_run_rerequested_event(),
        )
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
    async fn check_run_rerequested_denies_a_collaborator_without_a_grant() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // The rerequester is a repo collaborator but has no grant. The
        // grants-backed decision denies (fail-closed); the old
        // collaborator-role check would have allowed this re-run (fail-open,
        // violating deny-by-default, spec §5.2).
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
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "action_required",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let app = GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri())).unwrap();
        let metrics = Metrics::new().unwrap();
        handle_check_run_rerequested(
            &ctx(&pool, &app, &server, &metrics),
            &check_run_rerequested_event(),
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "a collaborator without a grant must be denied (fail-closed)"
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
