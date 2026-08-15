//! Maintenance of the `installations` table (spec §7.2, §7.5): tracking each
//! GitHub App installation's lifecycle state (`active` / `suspended` /
//! `deleted`) so the server can tell when an installation has disappeared and
//! stop dispatching work to it (instead of looping on token-mint failures).
//!
//! The `installation` webhook is the primary signal (created → `active`,
//! suspend → `suspended`, unsuspend → `active`, deleted → `deleted`); the 401
//! path in `auth` marks `deleted` as a fallback when no webhook ever arrived.

use octocrab::models::webhook_events::WebhookEventPayload;
use octocrab::models::webhook_events::payload::InstallationWebhookEventAction;
use slash_github::{AppAuthError, GithubApp, WebhookEvent};
use sqlx::PgPool;

/// Upserts the installation's lifecycle state from an `installation` webhook.
/// No-op for `new_permissions_accepted` (permissions don't change lifecycle).
/// Returns `Ok(false)` when the event carries no installation identity.
pub async fn handle_installation_event(
    pool: &PgPool,
    event: &WebhookEvent,
) -> Result<bool, sqlx::Error> {
    let WebhookEventPayload::Installation(payload) = &event.specific else {
        return Ok(false);
    };

    let Some(state) = installation_state(&payload.action) else {
        return Ok(false);
    };
    let Some((installation_id, account)) = installation_identity(event) else {
        return Ok(false);
    };

    upsert(pool, installation_id, &account, state).await?;
    Ok(true)
}

fn installation_state(action: &InstallationWebhookEventAction) -> Option<&'static str> {
    match action {
        InstallationWebhookEventAction::Created | InstallationWebhookEventAction::Unsuspend => {
            Some("active")
        }
        InstallationWebhookEventAction::Suspend => Some("suspended"),
        InstallationWebhookEventAction::Deleted => Some("deleted"),
        // New permissions don't change lifecycle state; octocrab may also add
        // future actions, which we safely ignore.
        _ => None,
    }
}

/// Extracts `(installation_id, account_login)` from the event's `installation`
/// object. `installation` webhooks carry the full installation (with the
/// account); other events only a minimal id — so `Full` is expected here.
fn installation_identity(event: &WebhookEvent) -> Option<(i64, String)> {
    let installation = event.installation.as_ref()?;
    let octocrab::models::webhook_events::EventInstallation::Full(installation) = installation
    else {
        return None;
    };
    Some((installation.id.0 as i64, installation.account.login.clone()))
}

async fn upsert(
    pool: &PgPool,
    installation_id: i64,
    account: &str,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO installations (installation_id, account, state)
         VALUES ($1, $2, $3)
         ON CONFLICT (installation_id) DO UPDATE
         SET account = EXCLUDED.account, state = EXCLUDED.state, updated_at = now()",
    )
    .bind(installation_id)
    .bind(account)
    .bind(state)
    .execute(pool)
    .await?;
    Ok(())
}

/// Marks an installation `deleted` — the 401 fallback path: GitHub answered
/// `401` on token mint, meaning the installation was uninstalled (or the App
/// lost access to it) without a `deleted` webhook ever being delivered. Idempotent.
pub async fn mark_deleted(pool: &PgPool, installation_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO installations (installation_id, account, state)
         VALUES ($1, '', 'deleted')
         ON CONFLICT (installation_id) DO UPDATE
         SET state = 'deleted', updated_at = now()",
    )
    .bind(installation_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mints an installation token, and when GitHub answers `401` (the
/// installation is gone) marks it `deleted` so callers can stop retrying
/// instead of looping on mint failures (spec §7.5). Passes through any other
/// mint error untouched.
pub async fn mint_installation_token(
    pool: &PgPool,
    app: &GithubApp,
    installation_id: u64,
    repository_id: u64,
    permissions: &[(&str, &str)],
) -> Result<String, AppAuthError> {
    match app
        .installation_token(installation_id, repository_id, permissions)
        .await
    {
        Ok(token) => Ok(token),
        Err(AppAuthError::InstallationGone { installation_id }) => {
            if let Err(db_error) = mark_deleted(pool, installation_id as i64).await {
                tracing::error!(
                    %db_error,
                    installation_id,
                    "failed to mark deleted installation in the 401 path"
                );
            }
            Err(AppAuthError::InstallationGone { installation_id })
        }
        Err(other) => Err(other),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::db;
    use sqlx::PgPool;

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE installations")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    fn installation_event(action: &str) -> WebhookEvent {
        // Full installation object as GitHub sends it for `installation`
        // events (account + permissions + events are all required by
        // octocrab's `EventInstallation::Full` deserialization).
        let json = serde_json::json!({
            "action": action,
            "installation": {
                "id": 39593433,
                "account": {
                    "login": "gagbo",
                    "id": 10496163,
                    "node_id": "MDQ6VXNlcjEwNDk2MTYz",
                    "avatar_url": "https://avatars.githubusercontent.com/u/10496163?v=4",
                    "gravatar_id": "",
                    "url": "https://api.github.com/users/gagbo",
                    "html_url": "https://github.com/gagbo",
                    "followers_url": "https://api.github.com/users/gagbo/followers",
                    "following_url": "https://api.github.com/users/gagbo/following{/other_user}",
                    "gists_url": "https://api.github.com/users/gagbo/gists{/gist_id}",
                    "starred_url": "https://api.github.com/users/gagbo/starred{/owner}{/repo}",
                    "subscriptions_url": "https://api.github.com/users/gagbo/subscriptions",
                    "organizations_url": "https://api.github.com/users/gagbo/orgs",
                    "repos_url": "https://api.github.com/users/gagbo/repos",
                    "events_url": "https://api.github.com/users/gagbo/events{/privacy}",
                    "received_events_url": "https://api.github.com/users/gagbo/received_events",
                    "type": "User",
                    "site_admin": false
                },
                "repository_selection": "all",
                "access_tokens_url": "https://api.github.com/app/installations/39593433/access_tokens",
                "repositories_url": "https://api.github.com/installation/repositories",
                "html_url": "https://github.com/settings/installations/39593433",
                "app_id": 360617,
                "target_id": 10496163,
                "target_type": "User",
                "permissions": {
                    "issues": "write",
                    "actions": "write",
                    "metadata": "read",
                    "pull_requests": "write"
                },
                "events": ["issues", "issue_comment", "pull_request"],
                "created_at": "2023-07-13T11:33:20.000+02:00",
                "updated_at": "2023-07-13T11:33:21.000+02:00",
                "single_file_name": null,
                "has_multiple_single_files": false,
                "single_file_paths": [],
                "suspended_by": null,
                "suspended_at": null
            }
        });
        slash_github::parse_webhook_event("installation", json.to_string().as_bytes()).unwrap()
    }

    async fn state_of(pool: &PgPool, installation_id: i64) -> String {
        sqlx::query_scalar("SELECT state FROM installations WHERE installation_id = $1")
            .bind(installation_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn created_sets_active() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let handled = handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();
        assert!(handled);
        assert_eq!(state_of(&pool, 39593433).await, "active");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn suspend_then_unsuspend_flips_state() {
        let Some(pool) = test_pool().await else {
            return;
        };
        handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();
        handle_installation_event(&pool, &installation_event("suspend"))
            .await
            .unwrap();
        assert_eq!(state_of(&pool, 39593433).await, "suspended");
        handle_installation_event(&pool, &installation_event("unsuspend"))
            .await
            .unwrap();
        assert_eq!(state_of(&pool, 39593433).await, "active");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn deleted_marks_deleted() {
        let Some(pool) = test_pool().await else {
            return;
        };
        handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();
        handle_installation_event(&pool, &installation_event("deleted"))
            .await
            .unwrap();
        assert_eq!(state_of(&pool, 39593433).await, "deleted");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn new_permissions_accepted_is_a_no_op() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let handled =
            handle_installation_event(&pool, &installation_event("new_permissions_accepted"))
                .await
                .unwrap();
        assert!(!handled);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM installations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn upsert_overwrites_state_and_account() {
        let Some(pool) = test_pool().await else {
            return;
        };
        handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();
        // Same installation, suspend event: account + state are overwritten.
        handle_installation_event(&pool, &installation_event("suspend"))
            .await
            .unwrap();
        let row: (String, String) =
            sqlx::query_as("SELECT account, state FROM installations WHERE installation_id = $1")
                .bind(39593433i64)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "gagbo");
        assert_eq!(row.1, "suspended");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn mark_deleted_sets_deleted_and_is_idempotent() {
        let Some(pool) = test_pool().await else {
            return;
        };
        handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();
        mark_deleted(&pool, 39593433).await.unwrap();
        assert_eq!(state_of(&pool, 39593433).await, "deleted");
        // Second call overwrites state again — still deleted, no error.
        mark_deleted(&pool, 39593433).await.unwrap();
        assert_eq!(state_of(&pool, 39593433).await, "deleted");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn mark_deleted_creates_the_row_when_absent() {
        let Some(pool) = test_pool().await else {
            return;
        };
        // No prior webhook: the 401 fallback still records the row so the
        // installation is known to be gone.
        mark_deleted(&pool, 999).await.unwrap();
        assert_eq!(state_of(&pool, 999).await, "deleted");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn mint_401_marks_the_installation_deleted() {
        let Some(pool) = test_pool().await else {
            return;
        };
        handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/app/installations/39593433/access_tokens",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({ "message": "Bad credentials" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let app = slash_github::GithubApp::with_base_uri(
            123,
            include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem"),
            Some(&server.uri()),
        )
        .unwrap();
        let result =
            mint_installation_token(&pool, &app, 39593433, 99, &[("checks", "write")]).await;
        assert!(matches!(
            result,
            Err(AppAuthError::InstallationGone {
                installation_id: 39593433
            })
        ));
        assert_eq!(state_of(&pool, 39593433).await, "deleted");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn mint_other_error_passes_through_without_touching_state() {
        let Some(pool) = test_pool().await else {
            return;
        };
        handle_installation_event(&pool, &installation_event("created"))
            .await
            .unwrap();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/app/installations/39593433/access_tokens",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(403)
                    .set_body_json(serde_json::json!({ "message": "Forbidden" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let app = slash_github::GithubApp::with_base_uri(
            123,
            include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem"),
            Some(&server.uri()),
        )
        .unwrap();
        let result =
            mint_installation_token(&pool, &app, 39593433, 99, &[("checks", "write")]).await;
        assert!(matches!(result, Err(AppAuthError::Mint(_))));
        assert_eq!(state_of(&pool, 39593433).await, "active");
    }
}
