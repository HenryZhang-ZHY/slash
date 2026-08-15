//! User onboarding HTTP API (org/user management lane, 1.0 MVP).
//!
//! account/password register/login + create-first-team onboarding, served by
//! the same axum server as the GitHub webhook control plane. Sessions are
//! stateless HMAC tokens in an HttpOnly cookie (see [`crate::auth`]).

use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::{HeaderValue, SET_COOKIE};
use axum::response::{IntoResponse, Response};
use axum::Json;
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
const DUMMY_HASH_FOR_TIMING: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$AQBxsd/1YH74zla8A5ymdQ$k+VQwhXnpFIX1lsK0a/Aqb1fEWcywY/Lci0Ytn4nCM4";

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
    pub email: String,
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

/// `{ user, teams }` — the onboarding /auth/me surface the Web App consumes.
#[derive(Debug, Serialize)]
pub struct MePayload {
    pub user: UserView,
    pub teams: Vec<TeamView>,
}

/// `{ user }` — the register/login surface.
#[derive(Debug, Serialize)]
pub struct AuthPayload {
    pub user: UserView,
}

// ---- response helpers -----------------------------------------------------

fn set_token_cookie(resp: &mut axum::response::Response, token: &str) {
    // Our Set-Cookie values are fixed, ASCII, attacker-non-influenced.
    #[allow(clippy::expect_used)]
    let value = HeaderValue::from_str(&auth::set_cookie_value(token))
        .expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, value);
}

fn clear_token_cookie(resp: &mut axum::response::Response) {
    #[allow(clippy::expect_used)]
    let value = HeaderValue::from_str(&auth::clear_cookie_value())
        .expect("Set-Cookie value is ASCII");
    resp.headers_mut().insert(SET_COOKIE, value);
}

fn user_view(id: Uuid, email: &str, display_name: &str) -> UserView {
    UserView {
        id,
        email: email.to_string(),
        display_name: display_name.to_string(),
    }
}

// ---- Handlers --------------------------------------------------------------

pub async fn register(
    State(state): State<crate::AppState>,
    Json(body): Json<RegisterRequest>,
) -> Response {
    if body.password.len() < 8 {
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
    let insert = sqlx::query(
        "INSERT INTO users (id, email, password_hash, display_name, status)
         VALUES ($1, $2, $3, $4, 'active')",
    )
    .bind(id)
    .bind(&email)
    .bind(&password_hash)
    .bind(body.display_name.trim());
    let result = insert.execute(&state.pool).await;
    match result {
        Ok(_) => {}
        Err(e) if is_unique_violation(&e) => {
            return api_error(StatusCode::CONFLICT, "an account with this email already exists")
        }
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create account"),
    }
    // Registration logs the user in.
    let token = match auth::sign_token(&state.auth_secret, id) {
        Ok(t) => t,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create session"),
    };
    let mut resp = Json(AuthPayload {
        user: user_view(id, &email, body.display_name.trim()),
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
        "SELECT id, password_hash, display_name FROM users WHERE email = $1 AND status = 'active'",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await;

    // Timing-safe login: when no row matches we still run an Argon2 verify
    // against a fixed dummy hash, so a non-existent email costs the same as
    // a valid email with a wrong password. This closes the user-enumeration
    // side channel (SlashLead review note) without changing the behavior.
    let (id, phc_hash, display_name): (Uuid, String, String) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = auth::verify_password(&body.password, DUMMY_HASH_FOR_TIMING);
            return api_error(StatusCode::UNAUTHORIZED, "invalid credentials");
        }
        Err(_) => return api_error(StatusCode::UNAUTHORIZED, "invalid credentials"),
    };
    if !auth::verify_password(&body.password, &phc_hash) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid credentials");
    }
    let token = match auth::sign_token(&state.auth_secret, id) {
        Ok(t) => t,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create session"),
    };
    let mut resp = Json(AuthPayload {
        user: user_view(id, &email, &display_name),
    })
    .into_response();
    set_token_cookie(&mut resp, &token);
    resp
}

pub async fn logout() -> Response {
    let mut resp = (StatusCode::NO_CONTENT).into_response();
    clear_token_cookie(&mut resp);
    resp
}

pub async fn me(
    State(state): State<crate::AppState>,
    auth_user: UserId,
) -> Response {
    let id = auth_user.0;
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT email, display_name FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;
    let (email, display_name) = match row {
        Ok(Some(r)) => r,
        _ => return api_error(StatusCode::UNAUTHORIZED, "unknown user"),
    };
    let teams = load_teams(&state.pool, id).await.unwrap_or_default();
    Json(MePayload {
        user: user_view(id, &email, &display_name),
        teams,
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
            )
        }
        None => slugify(name),
    };
    if slug.is_empty() {
        return api_error(StatusCode::UNPROCESSABLE_ENTITY, "could not derive a valid slug");
    }

    let team_id = Uuid::new_v4();
    let result = create_team_tx(&state.pool, user_id, team_id, name, &slug).await;
    match result {
        Ok(_org_id) => {
            let teams = load_teams(&state.pool, user_id).await.unwrap_or_default();
            match teams.iter().find(|t| t.id == team_id) {
                Some(team) => Json(json!({ "team": team })).into_response(),
                None => {
                    api_error(StatusCode::INTERNAL_SERVER_ERROR, "team created but not readable")
                }
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

    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, $3)",
    )
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

// ---- auth extractor -----------------------------------------------------------

/// Extracted authenticated user id from the session cookie.
#[derive(Clone, Copy)]
pub struct UserId(pub Uuid);

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
            let cookie = parts
                .headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok());
            let token = auth::session_token_from_header(cookie)
                .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "not signed in"))?;
            let user_id = match auth::verify_token(&state.auth_secret, &token) {
                Ok(id) => id,
                Err(AuthError::ExpiredToken) => {
                    return Err(api_error(StatusCode::UNAUTHORIZED, "session expired"))
                }
                Err(_) => return Err(api_error(StatusCode::UNAUTHORIZED, "invalid session")),
            };
            Ok(UserId(user_id))
        }
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

fn api_error(status: StatusCode, msg: &str) -> Response {
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
            web_dir: std::sync::Arc::from(""),
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

        // The user row exists with the normalized (lowercased/trimmed) email.
        let (id, email, hashed): (uuid::Uuid, String, String) = sqlx::query_as(
            "SELECT id, email, password_hash FROM users WHERE email = 'alice@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(email, "alice@example.com");
        // password is stored as an Argon2 PHC hash ("$argon2id$...").
        assert!(hashed.starts_with("$argon2id$"));
        assert!(crate::auth::verify_password("supersecure1", &hashed));
        let _ = id;
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
        let (uid,): (uuid::Uuid,) =
            sqlx::query_as("SELECT id FROM users WHERE email = 'frank@example.com'")
                .fetch_one(&pool)
                .await
                .unwrap();

        let resp = create_team(State(state.clone()), UserId(uid), Json(CreateTeamRequest {
            name: "Acme".into(),
            slug: None,
        }))
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
        let uid: uuid::Uuid =
            sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM users WHERE email = 'gina@example.com'")
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

    #[test]
    fn dummy_timing_hash_is_a_valid_argon2_phc() {
        // The timing-equalization dummy must be a parseable Argon2id hash so
        // `verify_password` actually runs Argon2 (matching the cost of a real
        // account) instead of failing fast and reintroducing the side channel.
        let parsed = argon2::PasswordHash::new(DUMMY_HASH_FOR_TIMING);
        assert!(parsed.is_ok(), "dummy hash must be valid PHC");
        // Any password fails against it, but the *cost* is what matters.
        assert!(!crate::auth::verify_password("anything", DUMMY_HASH_FOR_TIMING));
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
        let view = user_view(id, "a@b.com", "Alice");
        assert_eq!(view.id, id);
        assert_eq!(view.email, "a@b.com");
        assert_eq!(view.display_name, "Alice");
    }
}
