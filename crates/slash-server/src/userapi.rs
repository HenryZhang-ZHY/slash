//! User onboarding HTTP API (org/user management lane, 1.0 MVP).
//!
//! account/password register/login + create-first-team onboarding, served by
//! the same axum server as the GitHub webhook control plane. Sessions are
//! stateless HMAC tokens in an HttpOnly cookie (see [`crate::auth`]).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::{HeaderValue, SET_COOKIE};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth;
use crate::auth::AuthError;

/// Named placeholder in the transaction/flow for the onboarding org the first
/// team is scoped to when the user has none yet.
const DEFAULT_MEMBER_ROLE: &str = "maintainer";

/// A fixed Argon2id PHC hash used to equalize login timing when no account
/// matches, so a non-existent email costs the same as a wrong password on a
/// real account (closes the user-enumeration side channel). The password it
/// encodes is random and never issued to anyone; it's only the *cost* that
/// matters, never the content.
const DUMMY_HASH_FOR_TIMING: &str = "$argon2id$v=19$m=19456,t=2,p=1$AQBxsd/1YH74zla8A5ymdQ$k+VQwhXnpFIX1lsK0a/Aqb1fEWcywY/Lci0Ytn4nCM4";

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePasswordRequest {
    /// Required only when the authenticated account has no password
    /// credential yet. Existing password users keep their current login email.
    pub email: Option<String>,
    /// Required when replacing an existing password; omitted for accounts
    /// whose external identity is currently their only login method.
    pub current_password: Option<String>,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    /// Optional org-scoped slug; derives from `name` when absent.
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserView {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamView {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub slug: String,
    /// The viewer's role in this team (`member`/`maintainer`).
    pub role: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubConnectionView {
    pub login: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectionsView {
    pub github: Option<GithubConnectionView>,
}

/// `{ user, teams }` — the onboarding /auth/me surface the Web App consumes.
#[derive(Debug, Serialize)]
pub struct MePayload {
    pub user: UserView,
    pub teams: Vec<TeamView>,
    pub connections: ConnectionsView,
}

/// `{ user }` — the register/login surface.
#[derive(Debug, Serialize)]
pub struct AuthPayload {
    pub user: UserView,
}

// ---- response helpers -----------------------------------------------------

pub fn set_token_cookie(resp: &mut axum::response::Response, token: &str) {
    // Our Set-Cookie values are fixed, ASCII, attacker-non-influenced.
    #[allow(clippy::expect_used)]
    let value =
        HeaderValue::from_str(&auth::set_cookie_value(token)).expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, value);
}

fn clear_token_cookie(resp: &mut axum::response::Response) {
    #[allow(clippy::expect_used)]
    let value =
        HeaderValue::from_str(&auth::clear_cookie_value()).expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, value);
}

fn user_view(id: Uuid, email: Option<&str>, display_name: &str) -> UserView {
    UserView {
        id,
        email: email.map(str::to_string),
        display_name: display_name.to_string(),
    }
}

fn is_valid_password(password: &str) -> bool {
    password.chars().count() >= 8
}

// ---- Handlers --------------------------------------------------------------

pub async fn register(
    State(state): State<crate::AppState>,
    Json(body): Json<RegisterRequest>,
) -> Response {
    if !is_valid_password(&body.password) {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "password must be at least 8 characters",
        );
    }
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return api_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid email");
    }
    let password_hash = match auth::hash_password(&body.password) {
        Ok(h) => h,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "auth setup failed"),
    };
    let id = Uuid::new_v4();
    let result = async {
        let mut tx = state.pool.begin().await?;
        sqlx::query(
            "INSERT INTO users (id, display_name, status)
             VALUES ($1, $2, 'active')",
        )
        .bind(id)
        .bind(body.display_name.trim())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO password_credentials (user_id, normalized_email, password_hash)
             VALUES ($1, $2, $3)",
        )
        .bind(id)
        .bind(&email)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO user_emails
                (id, user_id, normalized_email, purpose, is_primary)
             VALUES ($1, $2, $3, 'contact', true)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(&email)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }
    .await;
    match result {
        Ok(_) => {}
        Err(e) if is_unique_violation(&e) => {
            return api_error(
                StatusCode::CONFLICT,
                "an account with this email already exists",
            );
        }
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create account",
            );
        }
    }
    // Registration logs the user in.
    let token = match auth::sign_token(&state.auth_secret, id) {
        Ok(t) => t,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create session",
            );
        }
    };
    let mut resp = Json(AuthPayload {
        user: user_view(id, Some(&email), body.display_name.trim()),
    })
    .into_response();
    set_token_cookie(&mut resp, &token);
    resp
}

pub async fn login(
    State(state): State<crate::AppState>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let email = body.email.trim().to_lowercase();
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT u.id, pc.password_hash, u.display_name
         FROM password_credentials pc
         JOIN users u ON u.id = pc.user_id
         WHERE pc.normalized_email = $1 AND u.status = 'active'",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await;

    // Timing-safe login: when no row matches we still run an Argon2 verify
    // against a fixed dummy hash, so a non-existent email costs the same as
    // a valid email with a wrong password. This closes the user-enumeration
    // side channel without changing the behavior.
    let (id, phc_hash, display_name): (Uuid, String, String) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = auth::verify_password(&body.password, DUMMY_HASH_FOR_TIMING);
            return api_error(StatusCode::UNAUTHORIZED, "invalid credentials");
        }
        Err(error) => {
            tracing::error!(%error, "password login database query failed");
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication service unavailable",
            );
        }
    };
    if !auth::verify_password(&body.password, &phc_hash) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid credentials");
    }
    let token = match auth::sign_token(&state.auth_secret, id) {
        Ok(t) => t,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create session",
            );
        }
    };
    let mut resp = Json(AuthPayload {
        user: user_view(id, Some(&email), &display_name),
    })
    .into_response();
    set_token_cookie(&mut resp, &token);
    resp
}

pub async fn logout() -> Response {
    let mut resp = (StatusCode::NO_CONTENT).into_response();
    clear_token_cookie(&mut resp);
    #[allow(clippy::expect_used)]
    let github_cookie = HeaderValue::from_str(&crate::github_user_access::clear_cookie_value())
        .expect("generated cookie is valid ASCII");
    resp.headers_mut().append(SET_COOKIE, github_cookie);
    resp
}

/// Creates or replaces the authenticated user's password credential.
///
/// Credential management is browser-session-only: callers reach this handler
/// through [`SessionUserId`], so a personal access token cannot create a new
/// long-lived login method. Existing password users must prove the current
/// password. External-identity-only users instead provide the email that will
/// become their normalized password-login name.
pub async fn update_password(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Json(body): Json<UpdatePasswordRequest>,
) -> Response {
    if !is_valid_password(&body.new_password) {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "new password must be at least 8 characters",
        );
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, user_id = %user_id, "password update transaction failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not update password",
            );
        }
    };
    let credential = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
        "SELECT u.status, pc.normalized_email, pc.password_hash
         FROM users u
         LEFT JOIN password_credentials pc ON pc.user_id = u.id
         WHERE u.id = $1
         FOR UPDATE OF u",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await;
    let (_status, existing_email, existing_hash) = match credential {
        Ok(Some(row)) if row.0 == "active" => row,
        Ok(Some(_) | None) => {
            return api_error(StatusCode::UNAUTHORIZED, "account unavailable");
        }
        Err(error) => {
            tracing::error!(%error, user_id = %user_id, "password credential lookup failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not update password",
            );
        }
    };
    if let (Some(_), Some(current_hash)) = (existing_email, existing_hash) {
        let Some(current_password) = body.current_password.as_deref() else {
            return api_error(StatusCode::UNAUTHORIZED, "current password required");
        };
        if !auth::verify_password(current_password, &current_hash) {
            return api_error(StatusCode::UNAUTHORIZED, "current password is incorrect");
        }
        let password_hash = match auth::hash_password(&body.new_password) {
            Ok(hash) => hash,
            Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "auth setup failed"),
        };
        if let Err(error) = sqlx::query(
            "UPDATE password_credentials
             SET password_hash = $2, updated_at = now()
             WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, user_id = %user_id, "password credential update failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not update password",
            );
        }
    } else {
        let email = body
            .email
            .as_deref()
            .map(str::trim)
            .map(str::to_lowercase)
            .filter(|email| !email.is_empty() && email.contains('@'));
        let Some(email) = email else {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "email is required to create a password credential",
            );
        };
        let password_hash = match auth::hash_password(&body.new_password) {
            Ok(hash) => hash,
            Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "auth setup failed"),
        };
        let inserted = sqlx::query(
            "INSERT INTO password_credentials (user_id, normalized_email, password_hash)
             VALUES ($1, $2, $3)",
        )
        .bind(user_id)
        .bind(&email)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(error) if is_unique_violation(&error) => {
                return api_error(
                    StatusCode::CONFLICT,
                    "an account with this email already exists",
                );
            }
            Err(error) => {
                tracing::error!(%error, user_id = %user_id, "password credential creation failed");
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not update password",
                );
            }
        }
        if let Err(error) = sqlx::query(
            "INSERT INTO user_emails
                (id, user_id, normalized_email, purpose, is_primary)
             VALUES (
                $1, $2, $3, 'contact',
                NOT EXISTS (
                    SELECT 1 FROM user_emails WHERE user_id = $2 AND is_primary
                )
             )
             ON CONFLICT (user_id, normalized_email) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(&email)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, user_id = %user_id, "password contact email creation failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not update password",
            );
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, user_id = %user_id, "password update commit failed");
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not update password",
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn me(State(state): State<crate::AppState>, auth_user: UserId) -> Response {
    let id = auth_user.0;
    let row = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT pc.normalized_email, u.display_name
         FROM users u
         LEFT JOIN password_credentials pc ON pc.user_id = u.id
         WHERE u.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;
    let (email, display_name) = match row {
        Ok(Some(r)) => r,
        _ => return api_error(StatusCode::UNAUTHORIZED, "unknown user"),
    };
    let teams = load_teams(&state.pool, id).await.unwrap_or_default();
    let github = match load_github_connection(&state.pool, id).await {
        Ok(connection) => connection,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not load account"),
    };
    Json(MePayload {
        user: user_view(id, email.as_deref(), &display_name),
        teams,
        connections: ConnectionsView { github },
    })
    .into_response()
}

pub async fn create_team(
    State(state): State<crate::AppState>,
    auth_user: UserId,
    Json(body): Json<CreateTeamRequest>,
) -> Response {
    let user_id = auth_user.0;
    let name = body.name.trim();
    if name.is_empty() {
        return api_error(StatusCode::UNPROCESSABLE_ENTITY, "team name required");
    }
    let slug = match body.slug {
        Some(s) if is_valid_slug(&s) => s.to_lowercase(),
        Some(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "slug must be lowercase letters/digits/hyphens",
            );
        }
        None => slugify(name),
    };
    if slug.is_empty() {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "could not derive a valid slug",
        );
    }

    let team_id = Uuid::new_v4();
    let result = create_team_tx(&state.pool, user_id, team_id, name, &slug).await;
    match result {
        Ok(_org_id) => {
            let teams = load_teams(&state.pool, user_id).await.unwrap_or_default();
            match teams.iter().find(|t| t.id == team_id) {
                Some(team) => Json(json!({ "team": team })).into_response(),
                None => api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "team created but not readable",
                ),
            }
        }
        Err(CreateTeamError::SlugTaken) => {
            api_error(StatusCode::CONFLICT, "a team with this slug already exists")
        }
        Err(CreateTeamError::Other) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create team")
        }
    }
}

// ---- internal data access ------------------------------------------------------

#[derive(Debug, thiserror::Error)]
enum CreateTeamError {
    #[error("slug taken")]
    SlugTaken,
    #[error("other")]
    Other,
}

/// Create (or reuse) the user's organization and create a team inside it,
/// adding the user as maintainer. Runs in one transaction.
async fn create_team_tx(
    pool: &PgPool,
    user_id: Uuid,
    team_id: Uuid,
    name: &str,
    slug: &str,
) -> Result<Uuid, CreateTeamError> {
    let mut tx = pool.begin().await.map_err(|_| CreateTeamError::Other)?;

    let org_id = first_org_of_user(pool, user_id).await;
    let org_id = match org_id {
        Some(o) => o,
        None => {
            // Onboarding: create the user's home organization (tenant).
            let new_org = Uuid::new_v4();
            let org_slug = format!("org-{}", slug);
            let inserted = sqlx::query(
                "INSERT INTO organizations (id, slug, name, state) VALUES ($1, $2, $3, 'active')
                 ON CONFLICT (slug) DO NOTHING",
            )
            .bind(new_org)
            .bind(&org_slug)
            .bind(name)
            .execute(&mut *tx)
            .await;
            match inserted {
                Ok(r) if r.rows_affected() == 1 => new_org,
                Ok(_) => return Err(CreateTeamError::Other), // slug clash on org
                Err(_) => return Err(CreateTeamError::Other),
            }
        }
    };

    let inserted = sqlx::query(
        "INSERT INTO teams (id, organization_id, name, slug, is_default_team, default_member_role)
         VALUES ($1, $2, $3, $4, false, 'member')
         ON CONFLICT (organization_id, slug) DO NOTHING",
    )
    .bind(team_id)
    .bind(org_id)
    .bind(name)
    .bind(slug)
    .execute(&mut *tx)
    .await;
    match inserted {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => return Err(CreateTeamError::SlugTaken),
        Err(_) => return Err(CreateTeamError::Other),
    }

    sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(user_id)
        .bind(DEFAULT_MEMBER_ROLE)
        .execute(&mut *tx)
        .await
        .map_err(|_| CreateTeamError::Other)?;

    // De-specialized org lifecycle (M3 #22): the user who creates their org
    // becomes its **owner** (in addition to their first team's maintainer).
    // The home org is a fully standard org with explicit ownership.
    sqlx::query(
        "INSERT INTO org_members (organization_id, user_id, role) VALUES ($1, $2, 'owner')
         ON CONFLICT (organization_id, user_id) DO NOTHING",
    )
    .bind(org_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| CreateTeamError::Other)?;

    tx.commit().await.map_err(|_| CreateTeamError::Other)?;
    Ok(org_id)
}

async fn first_org_of_user(pool: &PgPool, user_id: Uuid) -> Option<Uuid> {
    // The user's orgs are those of any team they belong to; take the first.
    sqlx::query_scalar::<_, Uuid>(
        "SELECT t.organization_id FROM teams t
         JOIN team_members tm ON tm.team_id = t.id
         WHERE tm.user_id = $1
         ORDER BY t.created_at LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

async fn load_teams(pool: &PgPool, user_id: Uuid) -> Result<Vec<TeamView>, sqlx::Error> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT t.id, t.organization_id, t.name, t.slug, tm.role
         FROM teams t JOIN team_members tm ON tm.team_id = t.id
         WHERE tm.user_id = $1 ORDER BY t.created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| TeamView {
            id: r.get("id"),
            organization_id: r.get("organization_id"),
            name: r.get("name"),
            slug: r.get("slug"),
            role: r.get("role"),
        })
        .collect())
}

async fn load_github_connection(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<GithubConnectionView>, sqlx::Error> {
    sqlx::query_as::<_, (String,)>(
        "SELECT ui.username
         FROM user_identities ui
         JOIN auth_connections ac ON ac.id = ui.connection_id
         WHERE ui.user_id = $1 AND ac.connection_key = 'github'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map(|row| row.map(|(login,)| GithubConnectionView { login }))
}

// ---- auth extractor -----------------------------------------------------------

/// Extracted authenticated user id from either a personal Bearer token or the
/// browser session cookie. An explicit Authorization header always wins and
/// fails closed instead of falling back to a cookie.
#[derive(Clone, Copy)]
pub struct UserId(pub Uuid);

/// Browser-session-only identity for credential-management endpoints. A
/// personal access token cannot mint or revoke other credentials.
#[derive(Clone, Copy)]
pub struct SessionUserId(pub Uuid);

#[derive(Debug)]
enum RequestAuthError {
    MissingSession,
    ExpiredSession,
    InvalidSession,
    InvalidAccessToken,
    AccessTokenDatabase,
}

impl RequestAuthError {
    fn into_response(self) -> Response {
        match self {
            Self::MissingSession => api_error(StatusCode::UNAUTHORIZED, "not signed in"),
            Self::ExpiredSession => api_error(StatusCode::UNAUTHORIZED, "session expired"),
            Self::InvalidSession => api_error(StatusCode::UNAUTHORIZED, "invalid session"),
            Self::InvalidAccessToken => api_error(StatusCode::UNAUTHORIZED, "invalid access token"),
            Self::AccessTokenDatabase => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not authenticate access token",
            ),
        }
    }
}

// The axum `FromRequestParts` trait requires an `impl Future` return, which
// clippy's `manual_async_fn` would otherwise suggest rewriting as `async fn`.
// That rewrite cannot satisfy the trait signature here, so the lint is
// suppressed for this impl block.
#[allow(clippy::manual_async_fn)]
impl axum::extract::FromRequestParts<crate::AppState> for UserId {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            resolve_user_id(&parts.headers, state)
                .await
                .map(UserId)
                .map_err(RequestAuthError::into_response)
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl axum::extract::FromRequestParts<crate::AppState> for SessionUserId {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            resolve_session_user_id(&parts.headers, state)
                .map(SessionUserId)
                .map_err(RequestAuthError::into_response)
        }
    }
}

async fn resolve_user_id(
    headers: &axum::http::HeaderMap,
    state: &crate::AppState,
) -> Result<Uuid, RequestAuthError> {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        let token = value
            .to_str()
            .ok()
            .and_then(|header| header.strip_prefix("Bearer "))
            .filter(|token| !token.is_empty())
            .ok_or(RequestAuthError::InvalidAccessToken)?;
        return match crate::access_tokens::authenticate(&state.pool, &state.auth_secret, token)
            .await
        {
            Ok(user_id) => Ok(user_id),
            Err(crate::access_tokens::AccessTokenError::Database(_)) => {
                Err(RequestAuthError::AccessTokenDatabase)
            }
            Err(_) => Err(RequestAuthError::InvalidAccessToken),
        };
    }
    resolve_session_user_id(headers, state)
}

fn resolve_session_user_id(
    headers: &axum::http::HeaderMap,
    state: &crate::AppState,
) -> Result<Uuid, RequestAuthError> {
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok());
    let token = auth::session_token_from_header(cookie).ok_or(RequestAuthError::MissingSession)?;
    match auth::verify_token(&state.auth_secret, &token) {
        Ok(id) => Ok(id),
        Err(AuthError::ExpiredToken) => Err(RequestAuthError::ExpiredSession),
        Err(_) => Err(RequestAuthError::InvalidSession),
    }
}

// ---- SQL / misc helpers ---------------------------------------------------------

fn is_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db) => db.is_unique_violation(),
        _ => false,
    }
}

fn is_valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub(crate) fn api_error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::db;
    use axum::extract::State;
    use axum::http::StatusCode;

    /// DB-backed tests skip cleanly when `SLASH_TEST_DATABASE_URL` is unset
    /// (matches the repo-wide plan M4 convention).
    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE org_members, team_members, teams, organizations, users CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    fn app_state(pool: PgPool) -> crate::AppState {
        crate::AppState {
            pool,
            metrics: std::sync::Arc::new(crate::metrics::Metrics::new().unwrap()),
            webhook_secret: std::sync::Arc::from("test-webhook-secret"),
            auth_secret: crate::auth::AuthSecret(std::sync::Arc::from("test-auth-secret")),
            admin_secret: None,
            github_app: None,
            web_dir: std::sync::Arc::from(""),
            github_oauth: None,
        }
    }

    fn response_status(res: &Response) -> StatusCode {
        res.status()
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn register_creates_the_user_and_returns_the_auth_payload() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let body = Json(RegisterRequest {
            email: "  Alice@Example.com ".into(),
            password: "supersecure1".into(),
            display_name: "Alice".into(),
        });
        let resp = register(State(state), body).await;
        assert_eq!(response_status(&resp), StatusCode::OK);

        // Identity, password credential, and verified contact data are stored
        // separately. The core user record does not own either login method.
        let (id, email, hashed): (uuid::Uuid, String, String) = sqlx::query_as(
            "SELECT u.id, pc.normalized_email, pc.password_hash
             FROM users u
             JOIN password_credentials pc ON pc.user_id = u.id
             WHERE pc.normalized_email = 'alice@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(email, "alice@example.com");
        // password is stored as an Argon2 PHC hash ("$argon2id$...").
        assert!(hashed.starts_with("$argon2id$"));
        assert!(crate::auth::verify_password("supersecure1", &hashed));
        let verified_contact: bool = sqlx::query_scalar(
            "SELECT verified_at IS NOT NULL
             FROM user_emails
             WHERE user_id = $1 AND normalized_email = 'alice@example.com'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!verified_contact);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn register_duplicate_email_is_conflict() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "bob@example.com".into(),
                password: "supersecure1".into(),
                display_name: "Bob".into(),
            }),
        )
        .await;
        let resp = register(
            State(state),
            Json(RegisterRequest {
                email: "bob@example.com".into(),
                password: "supersecure1".into(),
                display_name: "Bob2".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&resp), StatusCode::CONFLICT);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn register_rejects_short_password() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let resp = register(
            State(state),
            Json(RegisterRequest {
                email: "c@example.com".into(),
                password: "short".into(),
                display_name: "C".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&resp), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn password_length_counts_unicode_characters() {
        assert!(!is_valid_password("密码密"));
        assert!(is_valid_password("密码密码密码密码"));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn login_happy_path_verifies_password_and_returns_user() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "dana@example.com".into(),
                password: "supersecure1".into(),
                display_name: "Dana".into(),
            }),
        )
        .await;

        let resp = login(
            State(state),
            Json(LoginRequest {
                email: "dana@example.com".into(),
                password: "supersecure1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&resp), StatusCode::OK);

        // Cookie set on login.
        assert!(
            resp.headers()
                .get(axum::http::header::SET_COOKIE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("slash_session=") && v.contains("HttpOnly"))
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn login_wrong_password_is_unauthorized() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "e@example.com".into(),
                password: "supersecure1".into(),
                display_name: "E".into(),
            }),
        )
        .await;
        let resp = login(
            State(state),
            Json(LoginRequest {
                email: "e@example.com".into(),
                password: "wrong-password".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&resp), StatusCode::UNAUTHORIZED);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn login_unknown_email_is_unauthorized() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let resp = login(
            State(state),
            Json(LoginRequest {
                email: "nobody@example.com".into(),
                password: "whatever1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&resp), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_database_failure_is_service_unavailable() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect_lazy("postgres://slash:slash@127.0.0.1:1/slash")
            .unwrap();
        let state = app_state(pool);

        let resp = login(
            State(state),
            Json(LoginRequest {
                email: "dana@example.com".into(),
                password: "supersecure1".into(),
            }),
        )
        .await;

        assert_eq!(response_status(&resp), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn passwordless_user_can_create_a_password_credential() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'OIDC user')")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        let state = app_state(pool.clone());

        let response = update_password(
            State(state.clone()),
            SessionUserId(user_id),
            Json(UpdatePasswordRequest {
                email: Some("  OIDC@Example.com ".into()),
                current_password: None,
                new_password: "new-password-1".into(),
            }),
        )
        .await;

        assert_eq!(response_status(&response), StatusCode::NO_CONTENT);
        let (email, hash): (String, String) = sqlx::query_as(
            "SELECT normalized_email, password_hash
             FROM password_credentials WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(email, "oidc@example.com");
        assert!(crate::auth::verify_password("new-password-1", &hash));

        let login_response = login(
            State(state),
            Json(LoginRequest {
                email: "oidc@example.com".into(),
                password: "new-password-1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&login_response), StatusCode::OK);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn password_user_must_verify_current_password_before_changing_it() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "change@example.com".into(),
                password: "old-password-1".into(),
                display_name: "Change".into(),
            }),
        )
        .await;
        let user_id: Uuid = sqlx::query_scalar(
            "SELECT user_id FROM password_credentials WHERE normalized_email = 'change@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let wrong = update_password(
            State(state.clone()),
            SessionUserId(user_id),
            Json(UpdatePasswordRequest {
                email: None,
                current_password: Some("wrong-password".into()),
                new_password: "new-password-1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&wrong), StatusCode::UNAUTHORIZED);

        let updated = update_password(
            State(state.clone()),
            SessionUserId(user_id),
            Json(UpdatePasswordRequest {
                email: None,
                current_password: Some("old-password-1".into()),
                new_password: "new-password-1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&updated), StatusCode::NO_CONTENT);

        let old_login = login(
            State(state.clone()),
            Json(LoginRequest {
                email: "change@example.com".into(),
                password: "old-password-1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&old_login), StatusCode::UNAUTHORIZED);
        let new_login = login(
            State(state),
            Json(LoginRequest {
                email: "change@example.com".into(),
                password: "new-password-1".into(),
            }),
        )
        .await;
        assert_eq!(response_status(&new_login), StatusCode::OK);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn passwordless_user_cannot_claim_an_existing_login_email() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "owned@example.com".into(),
                password: "owner-password".into(),
                display_name: "Owner".into(),
            }),
        )
        .await;
        let passwordless_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'OIDC user')")
            .bind(passwordless_id)
            .execute(&pool)
            .await
            .unwrap();

        let response = update_password(
            State(state),
            SessionUserId(passwordless_id),
            Json(UpdatePasswordRequest {
                email: Some("OWNED@example.com".into()),
                current_password: None,
                new_password: "new-password-1".into(),
            }),
        )
        .await;

        assert_eq!(response_status(&response), StatusCode::CONFLICT);
        let has_credential: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM password_credentials WHERE user_id = $1)",
        )
        .bind(passwordless_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!has_credential);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn create_team_onboards_org_team_and_maintainer_membership() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "frank@example.com".into(),
                password: "supersecure1".into(),
                display_name: "Frank".into(),
            }),
        )
        .await;
        let (uid,): (uuid::Uuid,) = sqlx::query_as(
            "SELECT user_id FROM password_credentials
             WHERE normalized_email = 'frank@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let resp = create_team(
            State(state.clone()),
            UserId(uid),
            Json(CreateTeamRequest {
                name: "Acme".into(),
                slug: None,
            }),
        )
        .await;
        assert_eq!(response_status(&resp), StatusCode::OK);

        // An org was created, a team in it, and Frank is its maintainer.
        let row: (uuid::Uuid, String) = sqlx::query_as(
            "SELECT t.id, tm.role FROM teams t
             JOIN team_members tm ON tm.team_id = t.id
             WHERE tm.user_id = $1",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.1, "maintainer");

        // De-specialized org lifecycle (M3 #22): Frank is also the org owner.
        let org_owner: Option<String> = sqlx::query_scalar(
            "SELECT om.role FROM org_members om
             JOIN teams t ON t.organization_id = om.organization_id
             JOIN team_members tm ON tm.team_id = t.id
             WHERE tm.user_id = $1 AND om.user_id = $1",
        )
        .bind(uid)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(org_owner.as_deref(), Some("owner"));

        // /me now lists the team.
        let me_resp = me(State(state.clone()), UserId(uid)).await;
        assert_eq!(response_status(&me_resp), StatusCode::OK);
        let _ = row;
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn create_team_slug_conflict_is_conflict() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let _ = register(
            State(state.clone()),
            Json(RegisterRequest {
                email: "gina@example.com".into(),
                password: "supersecure1".into(),
                display_name: "Gina".into(),
            }),
        )
        .await;
        let uid: uuid::Uuid = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT user_id FROM password_credentials
             WHERE normalized_email = 'gina@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // First create succeeds and onboards an org; reusing the same user
        // keeps the org stable so the second same-slug insert conflicts on
        // `UNIQUE (organization_id, slug)`.
        let first = create_team(
            State(state.clone()),
            UserId(uid),
            Json(CreateTeamRequest {
                name: "Acme".into(),
                slug: Some("acme".into()),
            }),
        )
        .await;
        assert_eq!(response_status(&first), StatusCode::OK);

        let second = create_team(
            State(state.clone()),
            UserId(uid),
            Json(CreateTeamRequest {
                name: "Acme Security".into(),
                slug: Some("acme".into()),
            }),
        )
        .await;
        assert_eq!(response_status(&second), StatusCode::CONFLICT);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn me_for_unknown_user_is_unauthorized() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = app_state(pool.clone());
        let resp = me(State(state), UserId(uuid::Uuid::new_v4())).await;
        assert_eq!(response_status(&resp), StatusCode::UNAUTHORIZED);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn me_connection_state_comes_from_the_identity_record() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, display_name, status)
             VALUES ($1, 'Connected', 'active')",
        )
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            load_github_connection(&pool, user_id)
                .await
                .unwrap()
                .is_none()
        );

        sqlx::query(
            "INSERT INTO user_identities
                (id, user_id, connection_id, subject, username, display_name)
             VALUES (
                $1, $2, '00000000-0000-0000-0000-000000000001',
                '42', 'octocat', 'The Octocat'
             )",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

        let github = load_github_connection(&pool, user_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(github.login, "octocat");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn bearer_access_token_authenticates_across_replicas() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'API user')")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        let issuer = app_state(pool.clone());
        let issued =
            crate::access_tokens::issue(&pool, &issuer.auth_secret, user_id, "Agent token", None)
                .await
                .unwrap();
        let verifier = app_state(pool.clone());
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {}", issued.token).parse().unwrap(),
        );

        assert_eq!(resolve_user_id(&headers, &verifier).await.unwrap(), user_id);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn invalid_authorization_header_does_not_fall_back_to_cookie() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'Browser user')")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        let state = app_state(pool);
        let session = crate::auth::sign_token(&state.auth_secret, user_id).unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer invalid".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::COOKIE,
            format!("slash_session={session}").parse().unwrap(),
        );

        assert!(resolve_user_id(&headers, &state).await.is_err());
    }

    #[tokio::test]
    async fn session_only_auth_does_not_accept_a_bearer_token() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://slash:slash@127.0.0.1/slash")
            .unwrap();
        let state = app_state(pool);
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer slash_pat_not-a-browser-session".parse().unwrap(),
        );

        assert!(matches!(
            resolve_session_user_id(&headers, &state),
            Err(RequestAuthError::MissingSession)
        ));
    }

    #[test]
    fn dummy_timing_hash_is_a_valid_argon2_phc() {
        // The timing-equalization dummy must be a parseable Argon2id hash so
        // `verify_password` actually runs Argon2 (matching the cost of a real
        // account) instead of failing fast and reintroducing the side channel.
        let parsed = argon2::PasswordHash::new(DUMMY_HASH_FOR_TIMING);
        assert!(parsed.is_ok(), "dummy hash must be valid PHC");
        // Any password fails against it, but the *cost* is what matters.
        assert!(!crate::auth::verify_password(
            "anything",
            DUMMY_HASH_FOR_TIMING
        ));
    }

    #[test]
    fn is_valid_slug_accepts_lowercase_digits_and_dashes() {
        assert!(is_valid_slug("acme"));
        assert!(is_valid_slug("acme-123"));
        assert!(is_valid_slug("a1-b2"));
    }

    #[test]
    fn is_valid_slug_rejects_empty_long_uppercase_and_special_chars() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("a".repeat(64).as_str()));
        assert!(is_valid_slug("a".repeat(63).as_str()));
        assert!(!is_valid_slug("Acme"));
        assert!(!is_valid_slug("ac me"));
        assert!(!is_valid_slug("acme!"));
        assert!(!is_valid_slug("acme_1"));
    }

    #[test]
    fn slugify_lowercases_and_dash_separates_non_alphanumerics() {
        assert_eq!(slugify("Acme Corp"), "acme-corp");
        assert_eq!(slugify("  hello  world  "), "hello-world");
        assert_eq!(slugify("Version 1.2.3"), "version-1-2-3");
    }

    #[test]
    fn slugify_collapses_and_trims_trailing_dashes() {
        assert_eq!(slugify("a---b"), "a-b");
        assert_eq!(slugify("alpha-"), "alpha");
        assert_eq!(slugify("---"), "");
    }

    #[test]
    fn is_unique_violation_rejects_non_unique_errors() {
        // The true (unique-violation) branch is covered by the DB-backed
        // `register_duplicate_email_is_conflict`; PgDatabaseError has no
        // public constructor, so here we pin the fail-closed half: only a
        // real PG unique violation must map to true.
        assert!(!is_unique_violation(&sqlx::Error::RowNotFound));
        assert!(!is_unique_violation(&sqlx::Error::Io(
            std::io::Error::other("network down")
        )));
    }

    #[test]
    fn user_view_round_trips_fields() {
        let id = uuid::Uuid::new_v4();
        let view = user_view(id, Some("a@b.com"), "Alice");
        assert_eq!(view.id, id);
        assert_eq!(view.email.as_deref(), Some("a@b.com"));
        assert_eq!(view.display_name, "Alice");
    }
}
