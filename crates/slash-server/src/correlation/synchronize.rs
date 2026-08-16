use octocrab::models::webhook_events::payload::{
    PullRequestWebhookEventAction, PullRequestWebhookEventPayload,
};

use crate::invocations;
use crate::pipeline::{PipelineContext, PipelineError};

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
