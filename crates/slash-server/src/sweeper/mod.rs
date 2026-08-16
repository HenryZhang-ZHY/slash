//! The sweeper (spec §7.3): an interval loop running on every replica.
//! Delivery retention GC needs no explicit row-claiming (a `DELETE` is
//! naturally safe for multiple replicas to run concurrently); every
//! invocation-facing sweep below uses the same guarded-transition or
//! guarded-claim pattern for the same reason: race-free without a separate
//! lock, because `UPDATE ... WHERE ...` guards are inherently safe for
//! concurrent replicas.
//!
//! Scope note: failed-check-run-update retry is not attempted here — a
//! `CheckRunUpdate` call that itself fails (as opposed to the invocation's
//! GitHub-facing side never being called) is logged and dropped, matching
//! every other best-effort check-run write in this codebase.
//!
//! Split (R2 #29): each polling pass lives in its own submodule
//! (`stale_claimed`, `stale_dispatched`, `run_deadline`); this module holds
//! the config, the interval driver [`run`], the one-pass entry [`sweep_once`],
//! and the test suite.

mod run_deadline;
mod stale_claimed;
mod stale_dispatched;

pub(crate) use run_deadline::sweep_run_deadline;
pub(crate) use stale_claimed::sweep_stale_claimed;
pub(crate) use stale_dispatched::sweep_stale_dispatched;

use std::sync::Arc;
use std::time::Duration;

use slash_core::InvocationStatus;
use slash_github::GithubApp;
use sqlx::PgPool;

use crate::deliveries::{count_pending, delete_old_terminal, oldest_pending_age_seconds};
use crate::invocations;
use crate::metrics::Metrics;

#[derive(Debug, Clone, Copy)]
pub struct SweeperConfig {
    pub interval: Duration,
    pub delivery_retention: chrono::Duration,
    /// Spec §7.2/§7.3: invocations stuck in `claimed` past this long crashed
    /// before the write-ahead `dispatched` transition — before the POST.
    pub claimed_ttl: chrono::Duration,
    /// Spec §6.3: an ambiguous dispatch (timeout/5xx after the POST) leaves
    /// an invocation `dispatched` with no run id. Past this long, resolve it
    /// via the missing-run-id poll rather than waiting on a webhook that may
    /// never arrive.
    pub dispatch_timeout: chrono::Duration,
    /// Spec §6.3: a run that never sends a terminal `workflow_run` webhook
    /// (webhook outage, wedged run) is force-completed `timed_out` past this
    /// long — GitHub's own workflow runtime cap, by default.
    pub run_deadline: chrono::Duration,
    /// Overrides `https://api.github.com` for every `RepoClient` the sweeper
    /// constructs; used in tests to point at a mock server.
    pub base_uri: Option<&'static str>,
}

impl Default for SweeperConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            delivery_retention: chrono::Duration::days(30),
            claimed_ttl: chrono::Duration::seconds(60),
            dispatch_timeout: chrono::Duration::minutes(10),
            run_deadline: chrono::Duration::hours(72),
            base_uri: None,
        }
    }
}

/// One sweep pass. Exposed separately from [`run`] so it can be driven
/// directly in tests instead of racing a background loop. Also refreshes
/// `slash_deliveries_pending` (spec §7.4) — the sweeper's interval is a
/// convenient, already-existing heartbeat for this, rather than a second
/// dedicated task.
pub async fn sweep_once(
    pool: &PgPool,
    app: &GithubApp,
    config: &SweeperConfig,
    metrics: &Metrics,
) -> u64 {
    let pending = match count_pending(pool).await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::error!(%error, "sweeper: failed to count pending deliveries");
            0
        }
    };
    metrics.deliveries_pending.set(pending);

    match oldest_pending_age_seconds(pool).await {
        Ok(age) => metrics
            .deliveries_oldest_pending_age_seconds
            .set(age.unwrap_or(0)),
        Err(error) => {
            tracing::error!(%error, "sweeper: failed to compute oldest pending delivery age")
        }
    }

    match invocations::count_by_status(pool).await {
        Ok(counts) => {
            // Zero every known status first so one that just emptied out
            // doesn't linger at a stale nonzero value.
            for status in InvocationStatus::ALL {
                metrics
                    .invocations
                    .with_label_values(&[status.as_str()])
                    .set(0);
            }
            for (status, count) in counts {
                metrics.invocations.with_label_values(&[&status]).set(count);
            }
        }
        Err(error) => tracing::error!(%error, "sweeper: failed to count invocations by status"),
    }

    match invocations::max_dispatched_age_seconds(pool).await {
        Ok(age) => metrics
            .invocations_max_dispatched_age_seconds
            .set(age.unwrap_or(0)),
        Err(error) => tracing::error!(%error, "sweeper: failed to compute max dispatched age"),
    }

    let deleted = match delete_old_terminal(pool, config.delivery_retention).await {
        Ok(deleted) => deleted,
        Err(error) => {
            tracing::error!(%error, "sweeper: failed to delete old terminal deliveries");
            0
        }
    };

    sweep_stale_claimed(pool, app, config).await;
    sweep_stale_dispatched(pool, app, config, metrics).await;
    sweep_run_deadline(pool, app, config).await;

    // Test Engine flaky reconcile (docs/design/1.0-test-engine.md §5): a
    // level-triggered pass over the durable execution record. It runs on the
    // sweeper's existing heartbeat (not a separate timer), so replicas need no
    // leader election and a crashed pass is re-run on the next interval. Failures
    // are logged and dropped — never fatal to the sweep.
    match crate::flaky::reconcile(pool).await {
        Ok(n) if n > 0 => tracing::info!(
            transitions = n,
            "test-engine flaky reconcile applied transitions"
        ),
        Ok(_) => {}
        Err(error) => tracing::error!(%error, "test-engine flaky reconcile failed"),
    }

    deleted
}

pub async fn run(pool: PgPool, app: Arc<GithubApp>, config: SweeperConfig, metrics: Arc<Metrics>) {
    loop {
        let deleted = sweep_once(&pool, &app, &config, &metrics).await;
        if deleted > 0 {
            tracing::info!(deleted, "sweeper: deleted old terminal deliveries");
        }
        tokio::time::sleep(config.interval).await;
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::db;
    use crate::deliveries::{claim_pending, insert_delivery};
    use crate::invocations::{self, ClaimOutcome, NewInvocation};
    use slash_core::InvocationStatus;
    use uuid::Uuid;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_KEY_PEM: &[u8] =
        include_bytes!("../../../slash-github/tests/fixtures/test-app-key.pem");

    fn test_app() -> GithubApp {
        GithubApp::new(123, TEST_KEY_PEM).unwrap()
    }

    /// `None` when `SLASH_TEST_DATABASE_URL` is unset — callers skip
    /// cleanly rather than failing (plan M4).
    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE deliveries")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("TRUNCATE invocations")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
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

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_once_deletes_terminal_deliveries_past_retention() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "sweep-guid-1", "ping", b"{}")
            .await
            .unwrap();
        claim_pending(&pool)
            .await
            .unwrap()
            .unwrap()
            .complete()
            .await
            .unwrap();

        let config = SweeperConfig {
            delivery_retention: chrono::Duration::zero(),
            ..SweeperConfig::default()
        };
        let metrics = Metrics::new().unwrap();
        let deleted = sweep_once(&pool, &test_app(), &config, &metrics).await;
        assert_eq!(deleted, 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_once_refreshes_the_pending_gauge() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "sweep-guid-2", "ping", b"{}")
            .await
            .unwrap();
        insert_delivery(&pool, "sweep-guid-3", "ping", b"{}")
            .await
            .unwrap();

        let config = SweeperConfig::default();
        let metrics = Metrics::new().unwrap();
        sweep_once(&pool, &test_app(), &config, &metrics).await;

        assert_eq!(metrics.deliveries_pending.get(), 2);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_once_refreshes_the_invocation_status_and_stuck_age_gauges() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let claimed_id = Uuid::new_v4();
        invocations::claim(&pool, &sample(claimed_id))
            .await
            .unwrap();

        let dispatched_id = Uuid::new_v4();
        let mut dispatched_sample = sample(dispatched_id);
        dispatched_sample.comment_id = 200;
        invocations::claim(&pool, &dispatched_sample).await.unwrap();
        invocations::transition_status(&pool, dispatched_id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE invocations SET dispatched_at = now() - interval '30 seconds' WHERE id = $1",
        )
        .bind(dispatched_id)
        .execute(&pool)
        .await
        .unwrap();

        let config = SweeperConfig::default();
        let metrics = Metrics::new().unwrap();
        sweep_once(&pool, &test_app(), &config, &metrics).await;

        assert_eq!(metrics.invocations.with_label_values(&["claimed"]).get(), 1);
        assert_eq!(
            metrics.invocations.with_label_values(&["dispatched"]).get(),
            1
        );
        assert_eq!(
            metrics.invocations.with_label_values(&["completed"]).get(),
            0
        );
        assert!(metrics.invocations_max_dispatched_age_seconds.get() >= 25);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_stale_claimed_aborts_a_row_stranded_past_the_ttl() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        let ClaimOutcome::Claimed(_) = invocations::claim(&pool, &sample(id)).await.unwrap() else {
            panic!("expected a fresh claim");
        };

        let config = SweeperConfig {
            claimed_ttl: chrono::Duration::zero(),
            ..SweeperConfig::default()
        };
        sweep_stale_claimed(&pool, &test_app(), &config).await;

        let status: (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "aborted");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_stale_claimed_leaves_fresh_claims_alone() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();

        let config = SweeperConfig {
            claimed_ttl: chrono::Duration::hours(1),
            ..SweeperConfig::default()
        };
        sweep_stale_claimed(&pool, &test_app(), &config).await;

        let status: (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "claimed");
    }

    /// Two-worker concurrency test (plan M6/Testing Strategy): two replicas'
    /// sweepers racing on the very same stranded `claimed` row at the same
    /// instant. The guarded `transition_status` must let exactly one of
    /// them win and complete the check run.
    #[serial_test::serial(db)]
    #[tokio::test]
    async fn two_concurrent_sweeps_abort_a_stale_claimed_row_exactly_once() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        invocations::claim(&pool, &sample(id)).await.unwrap();
        invocations::set_check_run_id(&pool, id, 55).await.unwrap();

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/widgets/check-runs/55"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 55, "node_id": "n1", "head_sha": "deadbeef",
                "url": "https://api.github.com/repos/acme/widgets/check-runs/55",
                "html_url": null, "details_url": null, "conclusion": "neutral",
                "output": {"title": null, "summary": null, "text": null, "annotations_count": 0, "annotations_url": "https://api.github.com/x"},
                "started_at": null, "completed_at": null, "name": "slash/echo", "pull_requests": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let config = SweeperConfig {
            claimed_ttl: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        let app = test_app_against(&server);

        // `.expect(1)` above would fail this test if both concurrent sweeps
        // completed the check run instead of exactly one.
        tokio::join!(
            sweep_stale_claimed(&pool, &app, &config),
            sweep_stale_claimed(&pool, &app, &config),
        );

        let status: (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "aborted");
    }

    fn test_app_against(server: &MockServer) -> GithubApp {
        GithubApp::with_base_uri(123, TEST_KEY_PEM, Some(&server.uri())).unwrap()
    }

    fn app_response_json() -> serde_json::Value {
        serde_json::json!({
            "id": 123, "slug": "slash", "node_id": "n",
            "owner": {
                "login": "acme", "id": 1, "node_id": "n", "avatar_url": "https://avatars.githubusercontent.com/u/1",
                "gravatar_id": "", "url": "https://api.github.com/users/acme", "html_url": "https://github.com/acme",
                "followers_url": "https://api.github.com/users/acme/followers",
                "following_url": "https://api.github.com/users/acme/following{/other_user}",
                "gists_url": "https://api.github.com/users/acme/gists{/gist_id}",
                "starred_url": "https://api.github.com/users/acme/starred{/owner}{/repo}",
                "subscriptions_url": "https://api.github.com/users/acme/subscriptions",
                "organizations_url": "https://api.github.com/users/acme/orgs",
                "repos_url": "https://api.github.com/users/acme/repos",
                "events_url": "https://api.github.com/users/acme/events{/privacy}",
                "received_events_url": "https://api.github.com/users/acme/received_events",
                "type": "Organization", "site_admin": false
            },
            "name": "Slash", "external_url": "https://slash.example.com", "html_url": "https://github.com/apps/slash",
            "permissions": {"push": true, "pull": true},
            "events": ["issue_comment", "workflow_run", "check_run", "pull_request"]
        })
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

    async fn dispatched_invocation(pool: &PgPool, id: Uuid) {
        invocations::claim(pool, &sample(id)).await.unwrap();
        invocations::transition_status(pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
    }

    async fn correlated_invocation(pool: &PgPool, id: Uuid, workflow_run_id: i64) {
        dispatched_invocation(pool, id).await;
        sqlx::query("UPDATE invocations SET workflow_run_id = $2 WHERE id = $1")
            .bind(id)
            .bind(workflow_run_id)
            .execute(pool)
            .await
            .unwrap();
        invocations::transition_status(pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_stale_dispatched_correlates_when_the_poll_finds_the_missing_run() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        dispatched_invocation(&pool, id).await;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(app_response_json()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/workflows/echo.yml/runs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 1,
                "workflow_runs": [workflow_run_json(999, "queued", None)]
            })))
            .mount(&server)
            .await;

        let config = SweeperConfig {
            dispatch_timeout: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        let metrics = Metrics::new().unwrap();
        sweep_stale_dispatched(&pool, &test_app_against(&server), &config, &metrics).await;

        let row: (String, Option<i64>) =
            sqlx::query_as("SELECT status, workflow_run_id FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "correlated");
        assert_eq!(row.1, Some(999));
        assert_eq!(
            metrics
                .correlation_total
                .with_label_values(&["polled"])
                .get(),
            1
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_stale_dispatched_never_claims_a_human_started_run() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        dispatched_invocation(&pool, id).await;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(app_response_json()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        // GitHub itself filters by `actor`, so a human-started run of the
        // same workflow on the same branch never appears in a response
        // scoped to the bot login. Requiring the query param here means a
        // regression that ever drops the actor filter fails this request
        // outright (wiremock 404s an unmatched request) rather than
        // silently widening the poll to any actor.
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/workflows/echo.yml/runs"))
            .and(query_param("actor", "slash[bot]"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 0,
                "workflow_runs": []
            })))
            .mount(&server)
            .await;

        let config = SweeperConfig {
            dispatch_timeout: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        let metrics = Metrics::new().unwrap();
        sweep_stale_dispatched(&pool, &test_app_against(&server), &config, &metrics).await;

        let row: (String, Option<i64>) =
            sqlx::query_as("SELECT status, workflow_run_id FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "dispatch_failed");
        assert_eq!(row.1, None, "a human-started run must never be claimed");
        assert_eq!(
            metrics
                .dispatch_failures_total
                .with_label_values(&["poll_no_match"])
                .get(),
            1
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_stale_dispatched_marks_dispatch_failed_when_no_run_is_found() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        dispatched_invocation(&pool, id).await;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(app_response_json()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/workflows/echo.yml/runs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 0,
                "workflow_runs": []
            })))
            .mount(&server)
            .await;

        let config = SweeperConfig {
            dispatch_timeout: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        let metrics = Metrics::new().unwrap();
        sweep_stale_dispatched(&pool, &test_app_against(&server), &config, &metrics).await;

        let status: (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "dispatch_failed");
        assert_eq!(
            metrics
                .correlation_total
                .with_label_values(&["timeout"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .dispatch_failures_total
                .with_label_values(&["poll_no_match"])
                .get(),
            1
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_stale_dispatched_marks_correlation_timeout_when_the_poll_errors() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        dispatched_invocation(&pool, id).await;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_json(app_response_json()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/workflows/echo.yml/runs"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let config = SweeperConfig {
            dispatch_timeout: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        let metrics = Metrics::new().unwrap();
        sweep_stale_dispatched(&pool, &test_app_against(&server), &config, &metrics).await;

        let status: (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "correlation_timeout");
        assert_eq!(
            metrics
                .correlation_total
                .with_label_values(&["timeout"])
                .get(),
            1
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn sweep_run_deadline_recovers_a_lost_completed_webhook() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        correlated_invocation(&pool, id, 999).await;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/runs/999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(workflow_run_json(
                999,
                "completed",
                Some("success"),
            )))
            .mount(&server)
            .await;

        let config = SweeperConfig {
            run_deadline: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        sweep_run_deadline(&pool, &test_app_against(&server), &config).await;

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
    async fn sweep_run_deadline_marks_timed_out_when_still_running() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        correlated_invocation(&pool, id, 999).await;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "tok_abc",
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/actions/runs/999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(workflow_run_json(
                999,
                "in_progress",
                None,
            )))
            .mount(&server)
            .await;

        let config = SweeperConfig {
            run_deadline: chrono::Duration::zero(),
            base_uri: Some(server.uri().leak()),
            ..SweeperConfig::default()
        };
        sweep_run_deadline(&pool, &test_app_against(&server), &config).await;

        let row: (String, Option<String>) =
            sqlx::query_as("SELECT status, conclusion FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "completed");
        assert_eq!(row.1.as_deref(), Some("timed_out"));
    }
}
