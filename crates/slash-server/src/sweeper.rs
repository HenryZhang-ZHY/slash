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

use std::sync::Arc;
use std::time::Duration;

use slash_core::InvocationStatus;
use slash_github::{CheckRunUpdate, GithubApp, ListWorkflowRunsFilter, RepoClient};
use sqlx::PgPool;

use crate::correlation::apply_completed_run;
use crate::deliveries::{count_pending, delete_old_terminal, oldest_pending_age_seconds};
use crate::invocations::{self, ClaimRunIdOutcome};
use crate::metrics::Metrics;
use crate::pipeline::TOKEN_PERMISSIONS;

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

    deleted
}

/// Collects invocations stranded in `claimed` into `aborted` (spec §7.2), and
/// completes their check run with "re-issue the command" (spec §6.1's
/// head-SHA lookup is the caller's job when `check_run_id` was lost before
/// being persisted; this sweep only handles the common case where it was).
async fn sweep_stale_claimed(pool: &PgPool, app: &GithubApp, config: &SweeperConfig) {
    let stale = match invocations::find_stale_claimed(pool, config.claimed_ttl).await {
        Ok(stale) => stale,
        Err(error) => {
            tracing::error!(%error, "sweeper: failed to query stale claimed invocations");
            return;
        }
    };

    for invocation in stale {
        let transitioned = match invocations::transition_status(
            pool,
            invocation.id,
            InvocationStatus::Aborted,
        )
        .await
        {
            Ok(transitioned) => transitioned,
            Err(error) => {
                tracing::error!(%error, invocation_id = %invocation.id, "sweeper: failed to abort stale invocation");
                continue;
            }
        };
        if !transitioned {
            continue; // raced with another replica's sweep or a supersede
        }
        tracing::info!(invocation_id = %invocation.id, "sweeper: aborted a stale claimed invocation");

        let Some(check_run_id) = invocation.check_run_id else {
            continue; // no check run was ever created; nothing to complete
        };
        let token = match app
            .installation_token(
                invocation.installation_id as u64,
                invocation.repository_id as u64,
                TOKEN_PERMISSIONS,
            )
            .await
        {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, invocation_id = %invocation.id, "sweeper: failed to mint token to complete check run");
                continue;
            }
        };
        let Ok(client) = RepoClient::with_base_uri(
            &token,
            invocation.owner.clone(),
            invocation.repo.clone(),
            config.base_uri,
        ) else {
            continue;
        };
        let _ = client
            .update_check_run(
                check_run_id as u64,
                CheckRunUpdate {
                    status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                    conclusion: Some(octocrab::params::checks::CheckRunConclusion::Neutral),
                    details_url: None,
                    output: Some(("Aborted", "Please re-issue the command.")),
                },
            )
            .await;
    }
}

/// Resolves invocations `dispatched` past `dispatch_timeout` with no run id
/// (spec §6.3): an ambiguous dispatch outcome that a webhook alone can never
/// resolve, since there is no run id to match on yet. The `triggering_actor`
/// predicate (the App's own bot login) is what keeps this from ever
/// claiming a human-started run of the same workflow on the same branch.
async fn sweep_stale_dispatched(
    pool: &PgPool,
    app: &GithubApp,
    config: &SweeperConfig,
    metrics: &Metrics,
) {
    let stale =
        match invocations::find_stale_dispatched_unresolved(pool, config.dispatch_timeout).await {
            Ok(stale) => stale,
            Err(error) => {
                tracing::error!(%error, "sweeper: failed to query stale dispatched invocations");
                return;
            }
        };
    if stale.is_empty() {
        return;
    }

    let bot_login = match app.bot_login().await {
        Ok(login) => login,
        Err(error) => {
            tracing::warn!(%error, "sweeper: failed to resolve the app's bot login; skipping the missing-run-id poll");
            return;
        }
    };

    for invocation in stale {
        let token = match app
            .installation_token(
                invocation.installation_id as u64,
                invocation.repository_id as u64,
                TOKEN_PERMISSIONS,
            )
            .await
        {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, invocation_id = %invocation.id, "sweeper: failed to mint token for the missing-run-id poll");
                continue;
            }
        };
        let Ok(client) = RepoClient::with_base_uri(
            &token,
            invocation.owner.clone(),
            invocation.repo.clone(),
            config.base_uri,
        ) else {
            continue;
        };

        // A small backward skew (spec §6.3: `created_at >= dispatched_at -
        // skew`) absorbs clock drift between Slash and GitHub's own run
        // creation timestamp.
        let created_floor = invocation
            .dispatched_at
            .map(|at| format!(">={}", (at - chrono::Duration::minutes(2)).to_rfc3339()));
        let filter = ListWorkflowRunsFilter {
            event: Some("workflow_dispatch"),
            branch: Some(&invocation.head_branch),
            actor: Some(&bot_login),
            created: created_floor.as_deref(),
        };

        match client
            .list_workflow_runs(&invocation.workflow_file, filter)
            .await
        {
            Ok(candidates) if candidates.is_empty() => {
                // The poll succeeded and confirmed no run exists (spec
                // §7.2): a permanent, terminal outcome.
                match invocations::transition_status(
                    pool,
                    invocation.id,
                    InvocationStatus::DispatchFailed,
                )
                .await
                {
                    Ok(true) => {
                        metrics
                            .correlation_total
                            .with_label_values(&["timeout"])
                            .inc();
                        metrics
                            .dispatch_failures_total
                            .with_label_values(&["poll_no_match"])
                            .inc();
                        let _ = invocations::set_failure_reason(
                            pool,
                            invocation.id,
                            "no matching workflow run was found after the missing-run-id poll",
                        )
                        .await;
                        if let Some(check_run_id) = invocation.check_run_id {
                            let _ = client
                                .update_check_run(
                                    check_run_id as u64,
                                    CheckRunUpdate {
                                        status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                                        conclusion: Some(octocrab::params::checks::CheckRunConclusion::Failure),
                                        details_url: None,
                                        output: Some((
                                            "Dispatch failed",
                                            "No matching workflow run was found; the command may not have started. Please re-issue the command.",
                                        )),
                                    },
                                )
                                .await;
                        }
                    }
                    Ok(false) => {} // raced with another replica or a supersede
                    Err(error) => {
                        tracing::error!(%error, invocation_id = %invocation.id, "sweeper: failed to mark dispatch_failed");
                    }
                }
            }
            Ok(candidates) => {
                for candidate in candidates {
                    match invocations::claim_workflow_run_id_if_unresolved(
                        pool,
                        invocation.id,
                        candidate.id as i64,
                    )
                    .await
                    {
                        Ok(ClaimRunIdOutcome::Claimed) => {
                            let _ = invocations::transition_status(
                                pool,
                                invocation.id,
                                InvocationStatus::Correlated,
                            )
                            .await;
                            metrics
                                .correlation_total
                                .with_label_values(&["polled"])
                                .inc();
                            if let Some(check_run_id) = invocation.check_run_id {
                                let _ = client
                                    .update_check_run(
                                        check_run_id as u64,
                                        CheckRunUpdate {
                                            status: None,
                                            conclusion: None,
                                            details_url: Some(&candidate.html_url),
                                            output: None,
                                        },
                                    )
                                    .await;
                            }
                            break;
                        }
                        Ok(ClaimRunIdOutcome::AlreadyResolved) => break,
                        Ok(ClaimRunIdOutcome::RunIdTaken) => continue,
                        Err(error) => {
                            tracing::error!(%error, invocation_id = %invocation.id, "sweeper: failed to claim a candidate run id");
                            continue;
                        }
                    }
                }
            }
            Err(error) => {
                // The run's existence is still undeterminable (spec §7.2):
                // a distinct terminal outcome from a confirmed no-match.
                tracing::warn!(%error, invocation_id = %invocation.id, "sweeper: missing-run-id poll failed");
                metrics
                    .correlation_total
                    .with_label_values(&["timeout"])
                    .inc();
                if let Ok(true) = invocations::transition_status(
                    pool,
                    invocation.id,
                    InvocationStatus::CorrelationTimeout,
                )
                .await
                    && let Some(check_run_id) = invocation.check_run_id
                {
                    let _ = client
                        .update_check_run(
                            check_run_id as u64,
                            CheckRunUpdate {
                                status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                                conclusion: Some(octocrab::params::checks::CheckRunConclusion::Neutral),
                                details_url: None,
                                output: Some((
                                    "Could not confirm the run",
                                    "Could not determine whether the workflow started. Please re-issue the command.",
                                )),
                            },
                        )
                        .await;
                }
            }
        }
    }
}

/// Resolves invocations `correlated` past `run_deadline` (spec §6.3): either
/// the terminal `workflow_run` webhook was lost — re-fetching resolves it
/// exactly like the normal webhook path via [`apply_completed_run`] — or the
/// run is genuinely wedged, force-completed `timed_out` so the check run and
/// invocation row are never immortal.
async fn sweep_run_deadline(pool: &PgPool, app: &GithubApp, config: &SweeperConfig) {
    let stale = match invocations::find_stale_correlated(pool, config.run_deadline).await {
        Ok(stale) => stale,
        Err(error) => {
            tracing::error!(%error, "sweeper: failed to query stale correlated invocations");
            return;
        }
    };

    for invocation in stale {
        let Some(workflow_run_id) = invocation.workflow_run_id else {
            continue;
        };
        let token = match app
            .installation_token(
                invocation.installation_id as u64,
                invocation.repository_id as u64,
                TOKEN_PERMISSIONS,
            )
            .await
        {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, invocation_id = %invocation.id, "sweeper: failed to mint token for the run-deadline poll");
                continue;
            }
        };
        let Ok(client) = RepoClient::with_base_uri(
            &token,
            invocation.owner.clone(),
            invocation.repo.clone(),
            config.base_uri,
        ) else {
            continue;
        };

        match client.get_workflow_run(workflow_run_id as u64).await {
            Ok(fresh) if fresh.status == "completed" => {
                // The webhook was lost, not the run: resolve it exactly like
                // the normal `workflow_run.completed` path would have.
                if let Err(error) = apply_completed_run(pool, &client, &invocation, &fresh).await {
                    tracing::error!(%error, invocation_id = %invocation.id, "sweeper: failed to apply a recovered completed run");
                }
            }
            Ok(_) => {
                // Still running past the deadline: genuinely wedged.
                match invocations::transition_status(
                    pool,
                    invocation.id,
                    InvocationStatus::Completed,
                )
                .await
                {
                    Ok(true) => {
                        let _ = invocations::set_conclusion(pool, invocation.id, "timed_out").await;
                        let _ =
                            invocations::set_last_reported_status(pool, invocation.id, "completed")
                                .await;
                        if let Some(check_run_id) = invocation.check_run_id {
                            let _ = client
                                .update_check_run(
                                    check_run_id as u64,
                                    CheckRunUpdate {
                                        status: Some(octocrab::params::checks::CheckRunStatus::Completed),
                                        conclusion: Some(octocrab::params::checks::CheckRunConclusion::TimedOut),
                                        details_url: None,
                                        output: Some((
                                            "Timed out",
                                            "The workflow run exceeded the maximum runtime and was marked timed out.",
                                        )),
                                    },
                                )
                                .await;
                        }
                    }
                    Ok(false) => {} // raced with the normal webhook path
                    Err(error) => {
                        tracing::error!(%error, invocation_id = %invocation.id, "sweeper: failed to mark timed_out");
                    }
                }
            }
            Err(error) => {
                // Transient poll failure: leave it `correlated` and retry
                // next tick, rather than guessing at a terminal state.
                tracing::warn!(%error, invocation_id = %invocation.id, "sweeper: run-deadline poll failed; will retry");
            }
        }
    }
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
    use uuid::Uuid;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_KEY_PEM: &[u8] =
        include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem");

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
