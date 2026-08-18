use octocrab::models::webhook_events::payload::{
    CheckRunWebhookEventAction, CheckRunWebhookEventPayload,
};
use octocrab::params::checks::{CheckRunConclusion, CheckRunStatus};
use slash_core::{InvocationStatus, ResolvedRole, messages};
use slash_github::{CheckRunUpdate, RepoClient, WebhookEvent};

use crate::catalog::{CatalogError, CatalogOutcome, load_catalog, resolve_default_branch};
use crate::invocations::{self, NewInvocation};
use crate::pipeline::{
    PipelineContext, PipelineError, TOKEN_PERMISSIONS, log_permission_api_failure, permission_name,
    record_catalog_load_metrics,
};

/// Handles `check_run.rerequested` (spec §6.5): re-resolve the
/// *rerequester's* permission — never the original invoker's — against the
/// command's current configuration, claim a fresh invocation row for the
/// same comment at the next attempt, and run the normal pipeline from the
/// claim onward.
pub async fn handle_check_run_rerequested(
    ctx: &PipelineContext<'_>,
    event: &WebhookEvent,
) -> Result<(), PipelineError> {
    let slash_github::WebhookEventPayload::CheckRun(payload) = &event.specific else {
        return Ok(());
    };
    let CheckRunWebhookEventPayload {
        action: CheckRunWebhookEventAction::Rerequested,
        check_run,
        ..
    } = payload.as_ref()
    else {
        return Ok(());
    };

    #[derive(serde::Deserialize)]
    struct MinimalCheckRun {
        id: u64,
        external_id: Option<String>,
    }
    let Ok(check_run) = serde_json::from_value::<MinimalCheckRun>(check_run.clone()) else {
        return Ok(());
    };

    let Some(original) = invocations::find_by_check_run_id(
        ctx.pool,
        ctx.installation_id as i64,
        &ctx.owner,
        &ctx.repo,
        check_run.id as i64,
    )
    .await?
    else {
        // A stale or unknown check_run_id: nothing to re-run.
        return Ok(());
    };
    let _ = check_run.external_id;

    let Some(rerequester) = &event.sender else {
        return Ok(());
    };

    let token = crate::installations::mint_installation_token(
        ctx.pool,
        ctx.app,
        ctx.installation_id,
        ctx.repository_id,
        TOKEN_PERMISSIONS,
    )
    .await?;
    let client =
        RepoClient::with_base_uri(&token, ctx.owner.clone(), ctx.repo.clone(), ctx.base_uri)?;

    let permission = match client.get_collaborator_permission(&rerequester.login).await {
        Ok(permission) => permission,
        Err(error) => {
            log_permission_api_failure(
                "rerequest_lookup",
                &ctx.owner,
                &ctx.repo,
                &rerequester.login,
                original.comment_id as u64,
                &error,
            );
            let _ = client
                .update_check_run(
                    check_run.id,
                    CheckRunUpdate {
                        status: Some(CheckRunStatus::Completed),
                        conclusion: Some(CheckRunConclusion::ActionRequired),
                        details_url: None,
                        output: Some((
                            "Re-run denied",
                            "Slash could not verify the re-requester's current repository access.",
                        )),
                    },
                )
                .await;
            return Ok(());
        }
    };
    let role = ResolvedRole::from_role_name(&permission.role_name).unwrap_or_else(|| {
        ResolvedRole::from_permission_booleans(
            permission.user.permissions.admin,
            permission.user.permissions.maintain,
            permission.user.permissions.push,
            permission.user.permissions.triage,
            permission.user.permissions.pull,
        )
    });

    // Re-capture the PR head and re-check the command's *current*
    // permission requirement (config may have changed since the original
    // invocation).
    let pr = client.get_pull_request(original.pr_number as u64).await?;
    let hinted_default_branch = pr
        .base
        .repo
        .as_ref()
        .and_then(|repo| repo.default_branch.as_deref());
    let resolved = match resolve_default_branch(&client, hinted_default_branch).await {
        Ok(resolved) => resolved,
        Err(error) => {
            record_catalog_load_metrics(ctx, &error, "rerequest command catalog resolution failed");
            let body = messages::command_catalog_unavailable();
            let _ = client
                .update_check_run(
                    check_run.id,
                    CheckRunUpdate {
                        status: Some(CheckRunStatus::Completed),
                        conclusion: Some(CheckRunConclusion::ActionRequired),
                        details_url: None,
                        output: Some(("Re-run unavailable", &body)),
                    },
                )
                .await;
            return Ok(());
        }
    };
    tracing::debug!(
        owner = %ctx.owner,
        repo = %ctx.repo,
        default_branch = %resolved.name,
        config_sha = %resolved.sha,
        "resolved rerequest command catalog snapshot"
    );
    let catalog = match load_catalog(&client, &resolved.sha).await {
        Ok(CatalogOutcome::Loaded(catalog)) => {
            ctx.metrics
                .command_catalog_loads_total
                .with_label_values(&["loaded", "complete"])
                .inc();
            catalog
        }
        Ok(CatalogOutcome::NotConfigured) => {
            ctx.metrics
                .command_catalog_loads_total
                .with_label_values(&["not_configured", "complete"])
                .inc();
            let body = messages::installed_but_not_configured();
            let _ = client
                .update_check_run(
                    check_run.id,
                    CheckRunUpdate {
                        status: Some(CheckRunStatus::Completed),
                        conclusion: Some(CheckRunConclusion::ActionRequired),
                        details_url: None,
                        output: Some(("Re-run denied", &body)),
                    },
                )
                .await;
            return Ok(());
        }
        Err(error) => {
            record_catalog_load_metrics(ctx, &error, "rerequest command catalog load failed");
            let (title, body) = match &error {
                CatalogError::Invalid { details } => {
                    ("Re-run denied", messages::config_error(details))
                }
                CatalogError::Unavailable { .. } => (
                    "Re-run unavailable",
                    messages::command_catalog_unavailable(),
                ),
            };
            let _ = client
                .update_check_run(
                    check_run.id,
                    CheckRunUpdate {
                        status: Some(CheckRunStatus::Completed),
                        conclusion: Some(CheckRunConclusion::ActionRequired),
                        details_url: None,
                        output: Some((title, &body)),
                    },
                )
                .await;
            return Ok(());
        }
    };
    let Some(validated) = catalog.find(&original.command) else {
        return Ok(());
    };

    let authorized =
        slash_core::authorizes_command(ctx.repository_is_private, role, validated.permission);
    let required = permission_name(validated.permission);
    tracing::info!(
        owner = %ctx.owner,
        repo = %ctx.repo,
        repository_private = ctx.repository_is_private,
        policy = if ctx.repository_is_private { "private_collaborators" } else { "public_role" },
        actor = %rerequester.login,
        resolved_role = ?role,
        required_permission = required,
        command = %original.command,
        authorized,
        "evaluated command rerequest authorization"
    );
    if !authorized {
        // No comment surface for a denied re-run (spec §6.5); the check
        // run itself communicates the denial.
        let denial = if ctx.repository_is_private {
            messages::rerequest_private_collaborator_denied(&original.command)
        } else {
            messages::rerequest_permission_denied(&original.command, required)
        };
        let _ = client
            .update_check_run(
                check_run.id,
                CheckRunUpdate {
                    status: Some(CheckRunStatus::Completed),
                    conclusion: Some(CheckRunConclusion::ActionRequired),
                    details_url: None,
                    output: Some(("Re-run denied", &denial)),
                },
            )
            .await;
        return Ok(());
    }

    let new_id = uuid::Uuid::new_v4();
    let new_invocation = NewInvocation {
        id: new_id,
        installation_id: ctx.installation_id as i64,
        repository_id: ctx.repository_id as i64,
        owner: &ctx.owner,
        repo: &ctx.repo,
        comment_id: original.comment_id,
        attempt: original.attempt + 1,
        pr_number: original.pr_number,
        head_sha: &pr.head.sha,
        head_branch: &pr.head.ref_field,
        actor: &rerequester.login,
        actor_id: rerequester.id.0 as i64,
        command: &original.command,
        raw_comment_line: &original.raw_comment_line,
        args: serde_json::Value::Object(serde_json::Map::new()),
        workflow_file: &original.workflow_file,
        delivery_guid: ctx.delivery_guid,
    };

    let claim_outcome = invocations::claim(ctx.pool, &new_invocation).await?;
    let invocations::ClaimOutcome::Claimed(id) = claim_outcome else {
        return Ok(());
    };

    let check_run_name = format!("slash/{}", original.command);
    let new_check_run = client
        .create_check_run(&check_run_name, &pr.head.sha, &id.to_string())
        .await?;
    invocations::set_check_run_id(ctx.pool, id, new_check_run.id.0 as i64).await?;
    invocations::transition_status(ctx.pool, id, InvocationStatus::Dispatched).await?;

    let dispatch_ref = format!("refs/heads/{}", pr.head.ref_field);
    let inputs = serde_json::json!({
        "slash_run_id": id.to_string(),
        "slash_pr_number": original.pr_number.to_string(),
        "slash_head_sha": pr.head.sha,
        "slash_actor": rerequester.login,
        "slash_actor_id": rerequester.id.0.to_string(),
    });

    if let Ok(outcome) = client
        .dispatch_workflow(&original.workflow_file, &dispatch_ref, inputs)
        .await
    {
        invocations::set_workflow_run_id(ctx.pool, id, outcome.workflow_run_id as i64).await?;
        invocations::transition_status(ctx.pool, id, InvocationStatus::Correlated).await?;
        ctx.metrics
            .correlation_total
            .with_label_values(&["dispatch_response"])
            .inc();
    }

    Ok(())
}
