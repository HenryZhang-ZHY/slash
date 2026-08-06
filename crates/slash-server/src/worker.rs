//! The worker pool consuming `deliveries` (spec §7.3). `issue_comment` is
//! the M5 dispatch pipeline; `workflow_run`, `check_run`, and
//! `pull_request` are M6's correlation handlers. Anything else subscribed
//! (`installation`, `installation_repositories`) is parsed and logged for
//! now — `installations` table maintenance is a documented gap, not yet
//! implemented.

use std::time::Duration;

use slash_github::{GithubApp, WebhookEvent, WebhookEventPayload, WebhookEventType};
use sqlx::PgPool;

use crate::correlation;
use crate::deliveries::claim_pending;
use crate::metrics::Metrics;
use crate::pipeline::{self, PipelineContext};

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

    let event_name = claimed.delivery.event.clone();
    let guid = claimed.delivery.delivery_guid.clone();

    let parsed = match slash_github::parse_webhook_event(&event_name, &claimed.delivery.payload) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(delivery_guid = %guid, event = %event_name, %error, "failed to parse webhook payload");
            claimed.fail(&error.to_string()).await?;
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
        tracing::info!(delivery_guid = %guid, event = %event_name, kind = ?parsed.kind, "processed delivery");
        claimed.complete().await?;
        return Ok(ProcessOutcome::Processed);
    }

    let Some(ctx) = build_context(pool, app, metrics, &parsed) else {
        tracing::warn!(delivery_guid = %guid, event = %event_name, "event missing installation/repository");
        claimed
            .fail("missing installation/repository on webhook event")
            .await?;
        return Ok(ProcessOutcome::Processed);
    };

    let result = dispatch(&ctx, &parsed).await;
    match result {
        Ok(()) => claimed.complete().await?,
        Err(error) => {
            tracing::error!(delivery_guid = %guid, %error, "handler failed");
            claimed.fail(&error.to_string()).await?;
        }
    }

    Ok(ProcessOutcome::Processed)
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
) -> Option<PipelineContext<'a>> {
    let installation_id = event.installation.as_ref()?.id().0;
    let repository = event.repository.as_ref()?;
    let repository_id = repository.id.0;
    let owner = repository.owner.as_ref()?.login.clone();
    let repo = repository.name.clone();

    Some(PipelineContext {
        app,
        pool,
        installation_id,
        repository_id,
        owner,
        repo,
        base_uri: None,
        metrics,
    })
}

pub async fn run(
    pool: PgPool,
    app: std::sync::Arc<GithubApp>,
    metrics: std::sync::Arc<Metrics>,
    poll_interval: Duration,
) {
    loop {
        match process_one(&pool, &app, &metrics).await {
            Ok(ProcessOutcome::Processed) => {}
            Ok(ProcessOutcome::Idle) => tokio::time::sleep(poll_interval).await,
            Err(error) => {
                tracing::error!(%error, "worker iteration failed");
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
        sqlx::query("TRUNCATE deliveries")
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
}
