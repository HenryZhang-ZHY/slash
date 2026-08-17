//! GitHub OAuth 2.0 login and account linking (Authorization Code flow).
//!
//! Activated only when `SLASH_GITHUB_CLIENT_ID` and
//! `SLASH_GITHUB_CLIENT_SECRET_PATH` are both set. The server starts normally
//! with email/password auth when OAuth is not configured.
//!
//! Two modes:
//!   - **Login** (`GET /api/auth/github`): unauthenticated; creates or
//!     reuses a user by GitHub identity.
//!   - **Link** (`POST /api/auth/github/link`): authenticated; binds the
//!     caller's existing account to their GitHub identity.
//!
//! Both flow through the same callback (`GET /api/auth/github/callback`);
//! the signed state token carries the mode and, for link, the user id.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::{HeaderValue, SET_COOKIE};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use hmac::{Hmac, Mac};
use hmac::digest::KeyInit;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::auth::{self, AuthSecret};
use crate::userapi::api_error;

type HmacSha256 = Hmac<Sha256>;

/// GitHub OAuth endpoints.
const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USERINFO_URL: &str = "https://api.github.com/user";

/// Cookie that carries the signed CSRF state token.
const STATE_COOKIE: &str = "slash_github_state";

/// How long the state token is valid (10 minutes — short-lived CSRF token).
const STATE_TTL_SECS: u64 = 10 * 60;

/// GitHub OAuth configuration. `None` when OAuth login is not configured.
#[derive(Clone)]
pub struct OauthState {
    pub client_id: Arc<str>,
    pub client_secret: Arc<str>,
    /// Pre-configured base URL for constructing the redirect URI. Derived
    /// from `SLASH_BASE_URL` at startup; `None` falls back to the request's
    /// `Host` header.
    pub base_url: Option<Arc<str>>,
    pub auth_secret: AuthSecret,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GithubTokenResponse {
    access_token: String,
    token_type: String,
    scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct GithubUser {
    id: i64,
    login: String,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StateClaims {
    /// `"login"` or `"link"`.
    #[serde(default)]
    mode: String,
    /// CSRF token (random UUID, verified against the `state` query param).
    csrf: String,
    /// Expiry (unix seconds).
    exp: u64,
    /// Where to redirect after the callback. Defaults to `/onboarding`.
    #[serde(default)]
    redirect: String,
    /// For link mode: the authenticated user's UUID.
    #[serde(default)]
    user_id: Option<Uuid>,
}

// ---- Handlers ---------------------------------------------------------------

/// `GET /api/auth/github` — Initiate GitHub OAuth login (unauthenticated).
///
/// Generates a signed state token, stores it in a cookie, and redirects
/// the browser to GitHub's authorization endpoint.
pub async fn start_github_login(
    State(state): State<crate::AppState>,
    parts: axum::http::request::Parts,
) -> Response {
    let redirect_to = "/onboarding";
    start_github_oauth(state, parts, "login", None, redirect_to).await
}

/// `POST /api/auth/github/link` — Initiate GitHub OAuth for account linking
/// (authenticated). Binds the caller's existing Slash account to their
/// GitHub identity.
pub async fn start_github_link(
    State(state): State<crate::AppState>,
    auth_user: crate::userapi::UserId,
    parts: axum::http::request::Parts,
) -> Response {
    let redirect_to = "/settings?github=linked";
    start_github_oauth(state, parts, "link", Some(auth_user.0), redirect_to).await
}

/// `GET /api/auth/github/callback` — Handle GitHub OAuth callback.
///
/// Validates the state token, exchanges the authorization code for an
/// access token, fetches the GitHub user profile, then either:
///   - **login mode**: upserts the user and sets a session cookie.
///   - **link mode**: binds the GitHub identity to the existing user.
pub async fn handle_github_callback(
    State(state): State<crate::AppState>,
    axum::extract::Query(params): axum::extract::Query<CallbackParams>,
    axum::http::request::Parts { headers, .. }: axum::http::request::Parts,
) -> Response {
    let oauth = match &state.github_oauth {
        Some(o) => o,
        None => return api_error(StatusCode::NOT_FOUND, "github login is not configured"),
    };

    // Extract and verify the state token from the cookie.
    let state_token = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_state_cookie);
    let state_token = match state_token {
        Some(t) => t,
        None => return api_error(StatusCode::UNAUTHORIZED, "missing state cookie"),
    };
    let state_claims = match verify_state(&oauth.auth_secret, &state_token) {
        Ok(c) => c,
        Err(_) => return api_error(StatusCode::UNAUTHORIZED, "invalid or expired state"),
    };
    if state_claims.csrf != params.state {
        return api_error(StatusCode::UNAUTHORIZED, "state mismatch");
    }

    // Exchange authorization code for access token.
    let http = reqwest::Client::new();
    let token_resp = http
        .post(GITHUB_TOKEN_URL)
        .header("accept", "application/json")
        .form(&[
            ("client_id", oauth.client_id.as_ref()),
            ("client_secret", oauth.client_secret.as_ref()),
            ("code", &params.code),
        ])
        .send()
        .await;
    let token_resp = match token_resp {
        Ok(r) => r,
        Err(_) => return api_error(StatusCode::BAD_GATEWAY, "failed to reach github"),
    };
    if !token_resp.status().is_success() {
        return api_error(StatusCode::BAD_GATEWAY, "github token exchange failed");
    }
    let token_data: GithubTokenResponse = match token_resp.json().await {
        Ok(d) => d,
        Err(_) => return api_error(StatusCode::BAD_GATEWAY, "invalid github token response"),
    };

    // Fetch the authenticated user's GitHub profile.
    let gh_user_resp = http
        .get(GITHUB_USERINFO_URL)
        .header("authorization", format!("Bearer {}", token_data.access_token))
        .header("user-agent", "slash-server")
        .send()
        .await;
    let gh_user: GithubUser = match gh_user_resp {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(_) => return api_error(StatusCode::BAD_GATEWAY, "invalid github user response"),
        },
        Err(_) => return api_error(StatusCode::BAD_GATEWAY, "failed to reach github"),
    };

    let github_id = gh_user.id;
    let github_login = gh_user.login;

    let redirect_to = if state_claims.redirect.is_empty() {
        "/onboarding".to_string()
    } else {
        state_claims.redirect
    };

    // Branch on mode.
    match state_claims.mode.as_str() {
        "link" => {
            // Link mode: bind GitHub identity to the existing user.
            let user_id = match state_claims.user_id {
                Some(id) => id,
                None => return api_error(StatusCode::UNAUTHORIZED, "missing user context"),
            };
            if link_github_user(&state.pool, user_id, github_id, &github_login).await.is_err() {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not link github account");
            }
            // Redirect without issuing a new session cookie (user is already
            // authenticated in the browser).
            let mut resp = axum::response::Redirect::to(&redirect_to).into_response();
            clear_state_cookie(&mut resp);
            resp
        }
        _ => {
            // Login mode (default): upsert user and set session cookie.
            let display_name = gh_user.name.unwrap_or_else(|| github_login.clone());
            let email = gh_user
                .email
                .unwrap_or_else(|| format!("{github_id}+{github_login}@users.noreply.github.com"));

            let user_id = match upsert_github_user(&state.pool, github_id, &github_login, &email, &display_name).await {
                Ok(id) => id,
                Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create account"),
            };

            let token = match auth::sign_token(&state.auth_secret, user_id) {
                Ok(t) => t,
                Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create session"),
            };

            let mut resp = axum::response::Redirect::to(&redirect_to).into_response();
            set_session_and_clear_state_cookie(&mut resp, &token);
            resp
        }
    }
}

// ---- Shared OAuth initiation ------------------------------------------------

/// Core OAuth initiation logic shared by login and link flows.
async fn start_github_oauth(
    state: crate::AppState,
    parts: axum::http::request::Parts,
    mode: &str,
    user_id: Option<Uuid>,
    redirect_to: &str,
) -> Response {
    let oauth = match &state.github_oauth {
        Some(o) => o,
        None => return api_error(StatusCode::NOT_FOUND, "github login is not configured"),
    };

    let axum::http::request::Parts { uri: _, headers, .. } = parts;

    // Determine the callback base URL: prefer the configured value, fall
    // back to the request's Host header.
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = "https";
    let base = match &oauth.base_url {
        Some(b) => b.to_string(),
        None => format!("{scheme}://{host}"),
    };
    let redirect_uri = format!("{base}/api/auth/github/callback");

    // CSRF: short-lived signed state token.
    let csrf = Uuid::new_v4().to_string();
    let state_token = match sign_state(&oauth.auth_secret, mode, &csrf, redirect_to, user_id) {
        Ok(t) => t,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "auth setup failed"),
    };

    let mut resp = axum::response::Redirect::to(&format!(
        "{GITHUB_AUTHORIZE_URL}?client_id={}&redirect_uri={}&state={}&scope=read:user user:email",
        oauth.client_id,
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&state_token),
    ))
    .into_response();

    // Set the state cookie (HttpOnly, SameSite=Lax, short-lived).
    #[allow(clippy::expect_used)]
    let cookie = HeaderValue::from_str(&format!(
        "{STATE_COOKIE}={state_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={STATE_TTL_SECS}"
    ))
    .expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, cookie);

    resp
}

// ---- State token (HMAC-signed CSRF) ----------------------------------------

fn sign_state(
    secret: &AuthSecret,
    mode: &str,
    csrf: &str,
    redirect: &str,
    user_id: Option<Uuid>,
) -> Result<String, auth::AuthError> {
    let claims = StateClaims {
        mode: mode.to_string(),
        csrf: csrf.to_string(),
        exp: now_secs() + STATE_TTL_SECS,
        redirect: redirect.to_string(),
        user_id,
    };
    let payload_json =
        serde_json::to_string(&claims).map_err(|_| auth::AuthError::Encode)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(payload_json.as_bytes());
    let mac = hmac(secret, payload.as_bytes());
    Ok(format!("{payload}.{mac}"))
}

fn verify_state(
    secret: &AuthSecret,
    token: &str,
) -> Result<StateClaims, auth::AuthError> {
    let (payload, mac_str) = token
        .split_once('.')
        .ok_or(auth::AuthError::InvalidToken)?;
    let expected = hmac(secret, payload.as_bytes());
    let actual_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(mac_str)
        .map_err(|_| auth::AuthError::InvalidToken)?;
    let expected_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&expected)
        .map_err(|_| auth::AuthError::Internal)?;
    if actual_bytes != expected_bytes {
        return Err(auth::AuthError::InvalidToken);
    }
    let payload_json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| auth::AuthError::InvalidToken)?;
    let claims: StateClaims = serde_json::from_slice(&payload_json)
        .map_err(|_| auth::AuthError::InvalidToken)?;
    if claims.exp < now_secs() {
        return Err(auth::AuthError::ExpiredToken);
    }
    Ok(claims)
}

fn hmac(secret: &AuthSecret, data: &[u8]) -> String {
    #[allow(clippy::expect_used)]
    let mut mac =
        HmacSha256::new_from_slice(secret.0.as_bytes()).expect("HMAC accepts any key");
    mac.update(data);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---- Cookie helpers ---------------------------------------------------------

fn set_session_and_clear_state_cookie(resp: &mut Response, token: &str) {
    let session_str = auth::set_cookie_value(token);
    let state_str = format!(
        "{STATE_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    );
    #[allow(clippy::expect_used)]
    let combined = HeaderValue::from_str(&format!("{session_str}, {state_str}"))
        .expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, combined);
}

fn clear_state_cookie(resp: &mut Response) {
    #[allow(clippy::expect_used)]
    let value = HeaderValue::from_str(&format!(
        "{STATE_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
    .expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, value);
}

// ---- Database ---------------------------------------------------------------

/// Find an existing user by `github_user_id`, or by email, or create a new
/// one. Returns the user's UUID.
async fn upsert_github_user(
    pool: &sqlx::PgPool,
    github_id: i64,
    github_login: &str,
    email: &str,
    display_name: &str,
) -> Result<Uuid, sqlx::Error> {
    // 1. Existing user linked to this GitHub account.
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE github_user_id = $1",
    )
    .bind(github_id)
    .fetch_optional(pool)
    .await?
    {
        sqlx::query("UPDATE users SET github_login = $1, updated_at = now() WHERE id = $2")
            .bind(github_login)
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(id);
    }

    // 2. Existing user with a matching email — link GitHub identity.
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?
    {
        sqlx::query(
            "UPDATE users SET github_user_id = $1, github_login = $2, updated_at = now()
             WHERE id = $3",
        )
        .bind(github_id)
        .bind(github_login)
        .bind(id)
        .execute(pool)
        .await?;
        return Ok(id);
    }

    // 3. Brand-new user from GitHub.
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, display_name, github_user_id, github_login, status)
         VALUES ($1, $2, '', $3, $4, $5, 'active')",
    )
    .bind(id)
    .bind(email)
    .bind(display_name)
    .bind(github_id)
    .bind(github_login)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Bind a GitHub identity to an existing user account (link mode).
/// Fails if the GitHub account is already linked to a different user.
async fn link_github_user(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    github_id: i64,
    github_login: &str,
) -> Result<(), sqlx::Error> {
    // Check that this GitHub account isn't already linked to someone else.
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE github_user_id = $1 AND id != $2",
    )
    .bind(github_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if existing.is_some() {
        // Return a unique-violation-like error; the caller maps it to 409.
        return Err(sqlx::Error::RowNotFound); // sentinel
    }

    sqlx::query(
        "UPDATE users SET github_user_id = $1, github_login = $2, updated_at = now()
         WHERE id = $3",
    )
    .bind(github_id)
    .bind(github_login)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- Helpers ----------------------------------------------------------------

fn extract_state_cookie(cookie_header: &str) -> Option<String> {
    for pair in cookie_header.split(';') {
        let (k, v) = pair.trim().split_once('=')?;
        if k.trim() == STATE_COOKIE {
            return Some(v.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn secret() -> AuthSecret {
        AuthSecret(Arc::from("test-oauth-secret"))
    }

    #[test]
    fn state_login_sign_and_verify_roundtrip() {
        let token = sign_state(&secret(), "login", "csrf-123", "/onboarding", None).unwrap();
        let claims = verify_state(&secret(), &token).unwrap();
        assert_eq!(claims.mode, "login");
        assert_eq!(claims.csrf, "csrf-123");
        assert_eq!(claims.redirect, "/onboarding");
        assert!(claims.user_id.is_none());
    }

    #[test]
    fn state_link_sign_and_verify_roundtrip() {
        let uid = Uuid::new_v4();
        let token = sign_state(&secret(), "link", "csrf-456", "/settings?github=linked", Some(uid)).unwrap();
        let claims = verify_state(&secret(), &token).unwrap();
        assert_eq!(claims.mode, "link");
        assert_eq!(claims.csrf, "csrf-456");
        assert_eq!(claims.redirect, "/settings?github=linked");
        assert_eq!(claims.user_id, Some(uid));
    }

    #[test]
    fn state_tamper_fails() {
        let token = sign_state(&secret(), "login", "csrf-123", "/onboarding", None).unwrap();
        let mut parts: Vec<&str> = token.split('.').collect();
        let mut payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .unwrap();
        payload_bytes.push(0xff);
        let reencoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload_bytes);
        parts[0] = &reencoded;
        let tampered = parts.join(".");
        assert!(verify_state(&secret(), &tampered).is_err());
    }

    #[test]
    fn state_wrong_key_fails() {
        let token = sign_state(&secret(), "login", "csrf-123", "/onboarding", None).unwrap();
        let other = AuthSecret(Arc::from("wrong-key"));
        assert!(verify_state(&other, &token).is_err());
    }

    #[test]
    fn state_expired_fails() {
        let claims = StateClaims {
            mode: "login".into(),
            csrf: "csrf".into(),
            exp: now_secs() - 1,
            redirect: "/".into(),
            user_id: None,
        };
        let json = serde_json::to_string(&claims).unwrap();
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        let mac = hmac(&secret(), payload.as_bytes());
        let token = format!("{payload}.{mac}");
        assert!(matches!(
            verify_state(&secret(), &token),
            Err(auth::AuthError::ExpiredToken)
        ));
    }

    #[test]
    fn extract_state_cookie_parses_correctly() {
        let header = "other=1; slash_github_state=thetoken; x=2";
        assert_eq!(
            extract_state_cookie(header).as_deref(),
            Some("thetoken")
        );
        assert_eq!(extract_state_cookie(""), None);
        assert_eq!(extract_state_cookie("no_state_here"), None);
    }
}
