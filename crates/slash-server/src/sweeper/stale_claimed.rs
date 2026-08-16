use slash_core::InvocationStatus;
use slash_github::{CheckRunUpdate, GithubApp, RepoClient};
use sqlx::PgPool;

use crate::invocations;
use crate::pipeline::TOKEN_PERMISSIONS;

use super::SweeperConfig;

/// Collects invocations stranded in `claimed` into `aborted` (spec §7.2), and
/// completes their check run with "re-issue the command" (spec §6.1's
/// head-SHA lookup is the caller's job when `check_run_id` was lost before
/// being persisted; this sweep only handles the common case where it was).
pub(crate) async fn sweep_stale_claimed(pool: &PgPool, app: &GithubApp, config: &SweeperConfig) {
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
