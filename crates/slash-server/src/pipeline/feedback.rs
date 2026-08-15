use slash_core::messages;
use slash_github::RepoClient;
use slash_github::octocrab_types::ReactionContent;
use uuid::Uuid;

use crate::catalog::CatalogError;
use crate::invocations;
use crate::pipeline::{PipelineContext, PipelineError};

/// Logs a failed GitHub collaborator-permission API call. Shared by the
/// pipeline's permission guard; kept in `feedback` so the guard sequence
/// stays readable.
pub(crate) fn log_permission_api_failure(
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

pub(crate) async fn report_catalog_error(
    ctx: &PipelineContext<'_>,
    client: &RepoClient,
    issue_number: u64,
    comment_id: u64,
    can_comment: bool,
    error: &CatalogError,
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
        "command catalog load failed"
    );

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

pub(crate) async fn supersede_older_invocations(
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
