use octocrab::models::webhook_events::payload::WorkflowRunWebhookEventAction;
use octocrab::models::webhook_events::payload::WorkflowRunWebhookEventPayload;
use octocrab::params::checks::CheckRunStatus;
use slash_core::{InvocationStatus, messages};
use slash_github::{CheckRunUpdate, RepoClient, WorkflowRun};
use sqlx::PgPool;

use crate::invocations::{self, Invocation};
use crate::pipeline::{PipelineContext, PipelineError, TOKEN_PERMISSIONS};

use super::to_octocrab_conclusion;

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
