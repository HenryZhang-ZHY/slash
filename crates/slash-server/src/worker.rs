//! The worker pool consuming `deliveries` (spec §7.3). `issue_comment` is
//! the M5 dispatch pipeline; `workflow_run`, `check_run`, and
//! `pull_request` are M6's correlation handlers. `installation` /
//! `installation_repositories` events maintain the `installations` table
//! (`installations.rs`) so a removed/suspended install is recognized.

use std::time::Duration;

use slash_github::{GithubApp, WebhookEvent, WebhookEventPayload, WebhookEventType};
use sqlx::PgPool;

use crate::correlation;
use crate::deliveries::claim_pending;
use crate::metrics::Metrics;
use crate::pipeline::{self, PipelineContext};

const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(20);
const MAX_DELIVERY_ATTEMPTS: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessOutcome {
    Processed,
    Idle,
}

/// One claim-process-complete cycle. Exposed separately from [`run`] so it
/// can be driven directly in tests instead of racing a background loop.
pub async fn process_one(
    pool: &PgPool,
    app: &GithubApp,
    metrics: &Metrics,
) -> Result<ProcessOutcome, sqlx::Error> {
    let Some(claimed) = claim_pending(pool).await? else {
        return Ok(ProcessOutcome::Idle);
    };
    let started = std::time::Instant::now();
    if claimed.was_recovered() {
        metrics.delivery_lease_recoveries_total.inc();
    }

    let event_name = claimed.delivery.event.clone();
    let guid = claimed.delivery.delivery_guid.clone();

    let parsed = match slash_github::parse_webhook_event(&event_name, &claimed.delivery.payload) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(delivery_guid = %guid, event = %event_name, %error, "failed to parse webhook payload");
            claimed.fail(&error.to_string()).await?;
            observe_processing(metrics, started, "failed");
            return Ok(ProcessOutcome::Processed);
        }
    };

    let needs_context = matches!(
        parsed.kind,
        WebhookEventType::IssueComment
            | WebhookEventType::WorkflowRun
            | WebhookEventType::CheckRun
            | WebhookEventType::PullRequest
    );
    if !needs_context {
        // Installation lifecycle events (and anything else we don't dispatch
        // on) are still processed — installation events maintain the
        // `installations` table so a removed/suspended install is recognized
        // instead of looping on token-mint failures.
        if matches!(
            parsed.kind,
            WebhookEventType::Installation | WebhookEventType::InstallationRepositories
        ) && let Err(error) =
            crate::installations::handle_installation_event(pool, &parsed).await
        {
            tracing::error!(delivery_guid = %guid, event = %event_name, %error, "installation handler failed");
            claimed.fail(&error.to_string()).await?;
            observe_processing(metrics, started, "failed");
            return Ok(ProcessOutcome::Processed);
        }
        tracing::info!(delivery_guid = %guid, event = %event_name, kind = ?parsed.kind, "processed delivery");
        claimed.complete().await?;
        observe_processing(metrics, started, "completed");
        return Ok(ProcessOutcome::Processed);
    }

    let Some(ctx) = build_context(pool, app, metrics, &parsed, &guid) else {
        tracing::warn!(delivery_guid = %guid, event = %event_name, "event missing installation/repository");
        claimed
            .fail("missing installation/repository on webhook event")
            .await?;
        observe_processing(metrics, started, "failed");
        return Ok(ProcessOutcome::Processed);
    };

    let result = match run_with_lease_renewal(&claimed, dispatch(&ctx, &parsed)).await {
        Ok(result) => result,
        Err(error) => {
            observe_processing(metrics, started, "lease_lost");
            return Err(error);
        }
    };
    match result {
        Ok(()) => {
            claimed.complete().await?;
            observe_processing(metrics, started, "completed");
        }
        Err(error) => {
            tracing::error!(delivery_guid = %guid, %error, "handler failed");
            if is_safe_transient_auth_failure(&error)
                && claimed.delivery.attempts < MAX_DELIVERY_ATTEMPTS
            {
                let delay = retry_delay(claimed.delivery.attempts);
                claimed
                    .retry_after(
                        &error.to_string(),
                        chrono::Duration::from_std(delay).unwrap_or_default(),
                    )
                    .await?;
                metrics
                    .delivery_retries_total
                    .with_label_values(&["token_mint"])
                    .inc();
                observe_processing(metrics, started, "retried");
            } else {
                claimed.fail(&error.to_string()).await?;
                observe_processing(metrics, started, "failed");
            }
        }
    }

    Ok(ProcessOutcome::Processed)
}

async fn run_with_lease_renewal<T, E>(
    claimed: &crate::deliveries::ClaimedDelivery,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<Result<T, E>, sqlx::Error> {
    run_with_lease_renewal_interval(claimed, future, LEASE_RENEW_INTERVAL).await
}

async fn run_with_lease_renewal_interval<T, E>(
    claimed: &crate::deliveries::ClaimedDelivery,
    future: impl std::future::Future<Output = Result<T, E>>,
    renew_interval: Duration,
) -> Result<Result<T, E>, sqlx::Error> {
    tokio::pin!(future);
    let mut interval = tokio::time::interval(renew_interval);
    interval.tick().await;
    loop {
        tokio::select! {
            result = &mut future => return Ok(result),
            _ = interval.tick() => claimed.renew(crate::deliveries::DEFAULT_LEASE_DURATION).await?,
        }
    }
}

fn is_safe_transient_auth_failure(error: &pipeline::PipelineError) -> bool {
    matches!(
        error,
        pipeline::PipelineError::Auth(slash_github::AppAuthError::Mint(_))
    )
}

fn retry_delay(attempt: i32) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0).min(6);
    Duration::from_secs(1u64 << exponent)
}

fn observe_processing(metrics: &Metrics, started: std::time::Instant, outcome: &str) {
    metrics
        .delivery_processing_seconds
        .with_label_values(&[outcome])
        .observe(started.elapsed().as_secs_f64());
}

async fn dispatch(
    ctx: &PipelineContext<'_>,
    event: &WebhookEvent,
) -> Result<(), pipeline::PipelineError> {
    match &event.specific {
        WebhookEventPayload::IssueComment(_) => {
            pipeline::handle_issue_comment(ctx, &event.specific).await
        }
        WebhookEventPayload::WorkflowRun(payload) => {
            correlation::handle_workflow_run(ctx, payload).await
        }
        WebhookEventPayload::CheckRun(_) => {
            correlation::handle_check_run_rerequested(ctx, event).await
        }
        WebhookEventPayload::PullRequest(payload) => {
            correlation::handle_pull_request_synchronize(ctx, payload).await
        }
        _ => Ok(()),
    }
}

fn build_context<'a>(
    pool: &'a PgPool,
    app: &'a GithubApp,
    metrics: &'a Metrics,
    event: &slash_github::WebhookEvent,
    delivery_guid: &'a str,
) -> Option<PipelineContext<'a>> {
    let installation_id = event.installation.as_ref()?.id().0;
    let repository = event.repository.as_ref()?;
    let repository_id = repository.id.0;
    let repository_is_private = repository.private?;
    let owner = repository.owner.as_ref()?.login.clone();
    let repo = repository.name.clone();

    Some(PipelineContext {
        app,
        pool,
        installation_id,
        repository_id,
        repository_is_private,
        owner,
        repo,
        delivery_guid: Some(delivery_guid),
        base_uri: None,
        metrics,
    })
}

pub fn spawn_pool(
    pool: PgPool,
    app: std::sync::Arc<GithubApp>,
    metrics: std::sync::Arc<Metrics>,
    worker_count: usize,
    poll_interval: Duration,
) {
    for worker_id in 0..worker_count {
        tokio::spawn(run(
            pool.clone(),
            app.clone(),
            metrics.clone(),
            worker_id,
            poll_interval,
        ));
    }
}

pub async fn run(
    pool: PgPool,
    app: std::sync::Arc<GithubApp>,
    metrics: std::sync::Arc<Metrics>,
    worker_id: usize,
    poll_interval: Duration,
) {
    let max_idle_delay = Duration::from_secs(5);
    let mut idle_delay = poll_interval;
    loop {
        match process_one(&pool, &app, &metrics).await {
            Ok(ProcessOutcome::Processed) => idle_delay = poll_interval,
            Ok(ProcessOutcome::Idle) => {
                tokio::time::sleep(idle_delay).await;
                idle_delay = idle_delay.saturating_mul(2).min(max_idle_delay);
            }
            Err(error) => {
                tracing::error!(worker_id, %error, "worker iteration failed");
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db;
    use crate::deliveries::{insert_delivery, state_of};
    use crate::metrics::Metrics;

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
        sqlx::query("TRUNCATE deliveries, invocations")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn processes_a_pending_delivery_and_marks_it_done() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(
            &pool,
            "worker-guid-1",
            "ping",
            br#"{"zen":"x","hook_id":1}"#,
        )
        .await
        .unwrap();

        let metrics = Metrics::new().unwrap();
        let outcome = process_one(&pool, &test_app(), &metrics).await.unwrap();
        assert_eq!(outcome, ProcessOutcome::Processed);
        assert_eq!(
            state_of(&pool, "worker-guid-1").await.unwrap().as_deref(),
            Some("done")
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn returns_idle_when_nothing_is_pending() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let metrics = Metrics::new().unwrap();
        let outcome = process_one(&pool, &test_app(), &metrics).await.unwrap();
        assert_eq!(outcome, ProcessOutcome::Idle);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn an_unparseable_payload_is_marked_failed_not_lost() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "worker-guid-2", "ping", b"not json")
            .await
            .unwrap();

        let metrics = Metrics::new().unwrap();
        let outcome = process_one(&pool, &test_app(), &metrics).await.unwrap();
        assert_eq!(outcome, ProcessOutcome::Processed);
        assert_eq!(
            state_of(&pool, "worker-guid-2").await.unwrap().as_deref(),
            Some("failed")
        );
    }

    #[test]
    fn retry_backoff_is_exponential_and_bounded_by_the_attempt_limit() {
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(MAX_DELIVERY_ATTEMPTS), Duration::from_secs(16));
    }

    #[test]
    fn only_token_mint_failures_are_replayed_as_whole_deliveries() {
        assert!(is_safe_transient_auth_failure(
            &pipeline::PipelineError::Auth(slash_github::AppAuthError::Mint(
                "temporary".to_string()
            ))
        ));
        assert!(!is_safe_transient_auth_failure(
            &pipeline::PipelineError::Auth(slash_github::AppAuthError::InstallationGone {
                installation_id: 1
            })
        ));
        assert!(!is_safe_transient_auth_failure(
            &pipeline::PipelineError::GitHub(slash_github::ClientError::InvalidResponse(
                "ambiguous stage".to_string()
            ))
        ));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn lease_renewal_keeps_a_long_running_future_alive() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "worker-guid-renew", "ping", br#"{"zen":"x"}"#)
            .await
            .unwrap();
        let claimed =
            crate::deliveries::claim_pending_for(&pool, chrono::Duration::milliseconds(10))
                .await
                .unwrap()
                .unwrap();

        let result = run_with_lease_renewal_interval(
            &claimed,
            async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                Ok::<_, ()>("done")
            },
            Duration::from_millis(5),
        )
        .await
        .unwrap();
        assert_eq!(result, Ok("done"));
        let remaining: i64 = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM (lease_expires_at - now()))::bigint \
             FROM deliveries WHERE delivery_guid = 'worker-guid-renew'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(remaining >= 55);
        claimed.complete().await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn eight_workers_drain_sixty_four_deliveries_without_duplicates() {
        let Some(pool) = test_pool().await else {
            return;
        };
        for index in 0..64 {
            insert_delivery(
                &pool,
                &format!("burst-guid-{index}"),
                "ping",
                br#"{"zen":"x","hook_id":1}"#,
            )
            .await
            .unwrap();
        }

        let app = std::sync::Arc::new(test_app());
        let metrics = std::sync::Arc::new(Metrics::new().unwrap());
        let mut workers = Vec::new();
        for _ in 0..8 {
            let pool = pool.clone();
            let app = app.clone();
            let metrics = metrics.clone();
            workers.push(tokio::spawn(async move {
                for _ in 0..8 {
                    assert_eq!(
                        process_one(&pool, &app, &metrics).await.unwrap(),
                        ProcessOutcome::Processed
                    );
                }
            }));
        }
        for worker in workers {
            worker.await.unwrap();
        }

        let completed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM deliveries WHERE state = 'done' AND attempts = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(completed, 64);
    }
}
