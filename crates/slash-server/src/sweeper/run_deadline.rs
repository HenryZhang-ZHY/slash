use slash_core::InvocationStatus;
use slash_github::{CheckRunUpdate, GithubApp, RepoClient};
use sqlx::PgPool;

use crate::correlation::apply_completed_run;
use crate::invocations;
use crate::pipeline::TOKEN_PERMISSIONS;

use super::SweeperConfig;

/// Resolves invocations `correlated` past `run_deadline` (spec §6.3): either
/// the terminal `workflow_run` webhook was lost — re-fetching resolves it
/// exactly like the normal webhook path via [`apply_completed_run`] — or the
/// run is genuinely wedged, force-completed `timed_out` so the check run and
/// invocation row are never immortal.
pub(crate) async fn sweep_run_deadline(pool: &PgPool, app: &GithubApp, config: &SweeperConfig) {
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
        let token = match crate::installations::mint_installation_token(
            pool,
            app,
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
