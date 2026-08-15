use slash_core::InvocationStatus;
use slash_github::{CheckRunUpdate, GithubApp, ListWorkflowRunsFilter, RepoClient};
use sqlx::PgPool;

use crate::invocations::{self, ClaimRunIdOutcome};
use crate::metrics::Metrics;
use crate::pipeline::TOKEN_PERMISSIONS;

use super::SweeperConfig;

/// Resolves invocations `dispatched` past `dispatch_timeout` with no run id
/// (spec §6.3): an ambiguous dispatch outcome that a webhook alone can never
/// resolve, since there is no run id to match on yet. The `triggering_actor`
/// predicate (the App's own bot login) is what keeps this from ever
/// claiming a human-started run of the same workflow on the same branch.
pub(crate) async fn sweep_stale_dispatched(
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
