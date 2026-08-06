//! User-owned access tokens for REST API authentication.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthSecret;
use crate::userapi::SessionUserId;

const TOKEN_PREFIX: &str = "slash_pat_";
const MAX_EXPIRY_DAYS: u16 = 365;

type HmacSha256 = Hmac<Sha256>;
type AccessTokenRow = (
    Uuid,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessTokenView {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedAccessToken {
    pub access_token: AccessTokenView,
    pub token: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenError {
    #[error("token name must be between 1 and 100 characters")]
    InvalidName,
    #[error("token expiry must be between 1 and 365 days")]
    InvalidExpiry,
    #[error("invalid access token")]
    InvalidToken,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueAccessTokenRequest {
    pub name: String,
    pub expires_in_days: Option<u16>,
}

pub async fn list_tokens(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
) -> Response {
    match list(&state.pool, user_id).await {
        Ok(tokens) => Json(tokens).into_response(),
        Err(_) => crate::userapi::api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not load access tokens",
        ),
    }
}

pub async fn issue_token(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Json(body): Json<IssueAccessTokenRequest>,
) -> Response {
    match issue(
        &state.pool,
        &state.auth_secret,
        user_id,
        &body.name,
        body.expires_in_days,
    )
    .await
    {
        Ok(token) => (StatusCode::CREATED, Json(token)).into_response(),
        Err(AccessTokenError::InvalidName | AccessTokenError::InvalidExpiry) => {
            crate::userapi::api_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid token settings")
        }
        Err(_) => crate::userapi::api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not create access token",
        ),
    }
}

pub async fn revoke_token(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Path(id): Path<Uuid>,
) -> Response {
    match revoke(&state.pool, user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => crate::userapi::api_error(StatusCode::NOT_FOUND, "access token not found"),
        Err(_) => crate::userapi::api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not revoke access token",
        ),
    }
}

pub async fn issue(
    pool: &PgPool,
    auth_secret: &AuthSecret,
    user_id: Uuid,
    name: &str,
    expires_in_days: Option<u16>,
) -> Result<IssuedAccessToken, AccessTokenError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AccessTokenError::InvalidName);
    }
    if expires_in_days.is_some_and(|days| days == 0 || days > MAX_EXPIRY_DAYS) {
        return Err(AccessTokenError::InvalidExpiry);
    }

    let id = Uuid::new_v4();
    let token = generate_token(id);
    let token_hash = token_digest(auth_secret, &token);
    let expires_at = expires_in_days.map(|days| Utc::now() + Duration::days(i64::from(days)));
    let row = sqlx::query_as::<_, AccessTokenRow>(
        "INSERT INTO user_access_tokens (id, user_id, name, token_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, name, created_at, last_used_at, expires_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;

    Ok(IssuedAccessToken {
        access_token: view_from_row(row),
        token,
    })
}

pub async fn list(pool: &PgPool, user_id: Uuid) -> Result<Vec<AccessTokenView>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AccessTokenRow>(
        "SELECT id, name, created_at, last_used_at, expires_at
         FROM user_access_tokens
         WHERE user_id = $1 AND revoked_at IS NULL
         ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(view_from_row).collect())
}

pub async fn revoke(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE user_access_tokens
         SET revoked_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn authenticate(
    pool: &PgPool,
    auth_secret: &AuthSecret,
    token: &str,
) -> Result<Uuid, AccessTokenError> {
    let id = parse_token_id(token).ok_or(AccessTokenError::InvalidToken)?;
    let row = sqlx::query_as::<_, (Uuid, Vec<u8>)>(
        "SELECT t.user_id, t.token_hash
         FROM user_access_tokens t
         JOIN users u ON u.id = t.user_id
         WHERE t.id = $1
           AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > now())
           AND u.status = 'active'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(AccessTokenError::InvalidToken)?;

    let mut mac = keyed_mac(auth_secret);
    mac.update(token.as_bytes());
    mac.verify_slice(&row.1)
        .map_err(|_| AccessTokenError::InvalidToken)?;

    // Keep operational visibility without turning every API call into a write.
    sqlx::query(
        "UPDATE user_access_tokens
         SET last_used_at = now()
         WHERE id = $1
           AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(row.0)
}

fn generate_token(id: Uuid) -> String {
    let random = [
        Uuid::new_v4().as_bytes().as_slice(),
        Uuid::new_v4().as_bytes().as_slice(),
    ]
    .concat();
    format!(
        "{TOKEN_PREFIX}{}_{}",
        id.simple(),
        URL_SAFE_NO_PAD.encode(random)
    )
}

fn parse_token_id(token: &str) -> Option<Uuid> {
    let (id, secret) = token.strip_prefix(TOKEN_PREFIX)?.split_once('_')?;
    if secret.len() != 43 || URL_SAFE_NO_PAD.decode(secret).ok()?.len() != 32 {
        return None;
    }
    Uuid::parse_str(id).ok()
}

fn token_digest(auth_secret: &AuthSecret, token: &str) -> Vec<u8> {
    let mut mac = keyed_mac(auth_secret);
    mac.update(token.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn keyed_mac(auth_secret: &AuthSecret) -> HmacSha256 {
    #[allow(clippy::expect_used)]
    HmacSha256::new_from_slice(auth_secret.0.as_bytes()).expect("HMAC accepts any key")
}

fn view_from_row(row: AccessTokenRow) -> AccessTokenView {
    AccessTokenView {
        id: row.0,
        name: row.1,
        created_at: row.2,
        last_used_at: row.3,
        expires_at: row.4,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db;

    fn secret() -> AuthSecret {
        AuthSecret(Arc::from("test-auth-secret"))
    }

    async fn test_pool() -> Option<sqlx::PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE users CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    async fn create_user(pool: &sqlx::PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'Agent user')")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[test]
    fn generated_token_round_trips_its_public_id() {
        let id = Uuid::new_v4();
        let token = generate_token(id);
        assert_eq!(parse_token_id(&token), Some(id));
        assert!(token.starts_with("slash_pat_"));
        assert!(token.len() >= 80);
    }

    #[test]
    fn parser_rejects_malformed_and_session_tokens() {
        assert_eq!(parse_token_id("not-a-token"), None);
        assert_eq!(parse_token_id("slash_pat_not-a-uuid_secret"), None);
        assert_eq!(
            parse_token_id("slash_pat_00000000000000000000000000000000_"),
            None
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn issued_token_authenticates_then_revocation_invalidates_it() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = create_user(&pool).await;

        let issued = issue(&pool, &secret(), user_id, "Claude agent", Some(90))
            .await
            .unwrap();
        assert_eq!(issued.access_token.name, "Claude agent");
        assert!(issued.token.starts_with("slash_pat_"));
        let stored_hash: Vec<u8> =
            sqlx::query_scalar("SELECT token_hash FROM user_access_tokens WHERE id = $1")
                .bind(issued.access_token.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored_hash.len(), 32);
        assert_ne!(stored_hash, issued.token.as_bytes());
        assert_eq!(
            authenticate(&pool, &secret(), &issued.token).await.unwrap(),
            user_id
        );

        let rows = list(&pool, user_id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows.first().is_some_and(|row| row.last_used_at.is_some()));

        assert!(
            revoke(&pool, user_id, issued.access_token.id)
                .await
                .unwrap()
        );
        assert!(authenticate(&pool, &secret(), &issued.token).await.is_err());
        assert!(list(&pool, user_id).await.unwrap().is_empty());
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn another_user_cannot_revoke_a_token() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let owner = create_user(&pool).await;
        let other = create_user(&pool).await;
        let issued = issue(&pool, &secret(), owner, "Owner token", None)
            .await
            .unwrap();

        assert!(!revoke(&pool, other, issued.access_token.id).await.unwrap());
        assert_eq!(
            authenticate(&pool, &secret(), &issued.token).await.unwrap(),
            owner
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn expired_tokens_and_tokens_for_disabled_users_are_rejected() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = create_user(&pool).await;
        let issued = issue(&pool, &secret(), user_id, "Short lived", Some(1))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE user_access_tokens
             SET created_at = now() - interval '2 days',
                 expires_at = now() - interval '1 day'
             WHERE id = $1",
        )
        .bind(issued.access_token.id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(authenticate(&pool, &secret(), &issued.token).await.is_err());

        let active = issue(&pool, &secret(), user_id, "Disabled owner", None)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(authenticate(&pool, &secret(), &active.token).await.is_err());
    }
}
