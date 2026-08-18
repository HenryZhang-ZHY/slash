//! Instance-admin authentication and read-only operations console API.
//!
//! Admin access is deliberately separate from Slash user accounts. The
//! optional file-backed secret enables the surface; when it is absent every
//! `/admin` and `/api/admin` route returns 404.

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

const ADMIN_COOKIE: &str = "slash_admin_session";
const ADMIN_SESSION_TTL_SECS: u64 = 20 * 60;
const INSTALLATION_REFRESH_COOLDOWN_SECS: i64 = 5 * 60;
const INSTALLATION_REFRESH_LOCK: i64 = 8_215_501_052;

#[derive(Clone)]
pub struct AdminSecret(pub Arc<str>);

#[derive(Serialize, Deserialize)]
struct AdminClaims {
    exp: u64,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    secret: String,
}

pub async fn login(State(state): State<AppState>, Json(request): Json<LoginRequest>) -> Response {
    let Some(secret) = state.admin_secret.as_ref() else {
        return not_found();
    };
    if !constant_time_eq(secret.0.as_bytes(), request.secret.as_bytes()) {
        return api_error(StatusCode::UNAUTHORIZED, "invalid admin secret");
    }

    let token = match sign_token(secret) {
        Ok(token) => token,
        Err(()) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal auth failure"),
    };
    let mut response = Json(json!({ "authenticated": true })).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&set_cookie_value(&token)) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
        response
    } else {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal auth failure")
    }
}

pub async fn logout(admin: AdminSession) -> Response {
    let _ = admin;
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(cookie) = HeaderValue::from_str(&clear_cookie_value()) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

pub async fn session(admin: AdminSession) -> Json<serde_json::Value> {
    let _ = admin;
    Json(json!({ "authenticated": true }))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct OverviewResponse {
    active_installations: i64,
    personal_installations: i64,
    organization_installations: i64,
    suspended_installations: i64,
    registered_users: i64,
    deliveries_24h: i64,
    failed_deliveries_24h: i64,
    pending_deliveries: i64,
    oldest_pending_seconds: Option<i64>,
    invocations_24h: i64,
    failed_invocations_24h: i64,
    running_invocations: i64,
    last_installation_sync_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn overview(
    admin: AdminSession,
    State(state): State<AppState>,
) -> Result<Json<OverviewResponse>, Response> {
    let _ = admin;
    let overview = sqlx::query_as::<_, OverviewResponse>(
        "SELECT
            (SELECT count(*) FROM installations WHERE state = 'active')::bigint AS active_installations,
            (SELECT count(*) FROM installations WHERE state = 'active' AND lower(target_type) = 'user')::bigint AS personal_installations,
            (SELECT count(*) FROM installations WHERE state = 'active' AND lower(target_type) = 'organization')::bigint AS organization_installations,
            (SELECT count(*) FROM installations WHERE state = 'suspended')::bigint AS suspended_installations,
            (SELECT count(*) FROM users)::bigint AS registered_users,
            (SELECT count(*) FROM deliveries WHERE received_at >= now() - interval '24 hours')::bigint AS deliveries_24h,
            (SELECT count(*) FROM deliveries WHERE state = 'failed' AND received_at >= now() - interval '24 hours')::bigint AS failed_deliveries_24h,
            (SELECT count(*) FROM deliveries WHERE state = 'pending')::bigint AS pending_deliveries,
            (SELECT EXTRACT(EPOCH FROM (now() - min(received_at)))::bigint FROM deliveries WHERE state = 'pending') AS oldest_pending_seconds,
            (SELECT count(*) FROM invocations WHERE created_at >= now() - interval '24 hours')::bigint AS invocations_24h,
            (SELECT count(*) FROM invocations WHERE status IN ('aborted', 'dispatch_failed', 'correlation_timeout') AND created_at >= now() - interval '24 hours')::bigint AS failed_invocations_24h,
            (SELECT count(*) FROM invocations WHERE status IN ('claimed', 'dispatched', 'correlated'))::bigint AS running_invocations,
            (SELECT last_success_at FROM installation_sync_state WHERE singleton = true) AS last_installation_sync_at",
    )
    .fetch_one(&state.pool)
    .await
    .map_err(internal_error)?;
    Ok(Json(overview))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct InstallationView {
    installation_id: i64,
    account: String,
    target_type: String,
    state: String,
    installed_at: Option<chrono::DateTime<chrono::Utc>>,
    updated_at: chrono::DateTime<chrono::Utc>,
    last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn installations(
    admin: AdminSession,
    State(state): State<AppState>,
) -> Result<Json<Vec<InstallationView>>, Response> {
    let _ = admin;
    let rows = sqlx::query_as::<_, InstallationView>(
        "SELECT installation_id, account, target_type, state, installed_at, updated_at, last_synced_at
         FROM installations ORDER BY
            CASE state WHEN 'active' THEN 0 WHEN 'suspended' THEN 1 ELSE 2 END,
            account",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(internal_error)?;
    Ok(Json(rows))
}

#[derive(Serialize)]
pub struct RefreshResponse {
    refreshed: bool,
    installation_count: i64,
    last_success_at: chrono::DateTime<chrono::Utc>,
}

/// Explicit, cross-replica rate-limited installation refresh. There is no
/// timer or page polling: GitHub is contacted only after an admin click, and
/// at most once per five minutes for the whole database-backed deployment.
pub async fn refresh_installations(
    admin: AdminSession,
    State(state): State<AppState>,
) -> Result<Json<RefreshResponse>, Response> {
    let _ = admin;
    let Some(app) = state.github_app.as_ref() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub App client unavailable",
        ));
    };
    let mut tx = state.pool.begin().await.map_err(internal_error)?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(INSTALLATION_REFRESH_LOCK)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_error)?;
    if !acquired {
        return Err(api_error(
            StatusCode::CONFLICT,
            "installation refresh already in progress",
        ));
    }

    let current: Option<(chrono::DateTime<chrono::Utc>, i64)> = sqlx::query_as(
        "SELECT last_success_at, installation_count
         FROM installation_sync_state WHERE singleton = true",
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal_error)?;
    if let Some((last_success_at, installation_count)) = current
        && last_success_at
            > chrono::Utc::now() - chrono::Duration::seconds(INSTALLATION_REFRESH_COOLDOWN_SECS)
    {
        tx.commit().await.map_err(internal_error)?;
        return Ok(Json(RefreshResponse {
            refreshed: false,
            installation_count,
            last_success_at,
        }));
    }

    let snapshot = app.list_installations().await.map_err(|error| {
        tracing::warn!(%error, "admin installation refresh failed");
        api_error(
            StatusCode::BAD_GATEWAY,
            "GitHub installation refresh failed",
        )
    })?;
    crate::installations::apply_snapshot(&mut tx, &snapshot)
        .await
        .map_err(internal_error)?;
    let (last_success_at, installation_count) = sqlx::query_as(
        "SELECT last_success_at, installation_count
         FROM installation_sync_state WHERE singleton = true",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    Ok(Json(RefreshResponse {
        refreshed: true,
        installation_count,
        last_success_at,
    }))
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    delivery_guid: String,
    event: String,
    payload: Vec<u8>,
    received_at: chrono::DateTime<chrono::Utc>,
    processed_at: Option<chrono::DateTime<chrono::Utc>>,
    state: String,
    attempts: i32,
    last_error: Option<String>,
}

#[derive(Serialize)]
pub struct DeliveryView {
    delivery_guid: String,
    event: String,
    action: Option<String>,
    repository: Option<String>,
    received_at: chrono::DateTime<chrono::Utc>,
    processed_at: Option<chrono::DateTime<chrono::Utc>>,
    state: String,
    attempts: i32,
    last_error: Option<String>,
}

pub async fn deliveries(
    admin: AdminSession,
    State(state): State<AppState>,
) -> Result<Json<Vec<DeliveryView>>, Response> {
    let _ = admin;
    let rows = sqlx::query_as::<_, DeliveryRow>(
        "SELECT delivery_guid, event, payload, received_at, processed_at, state, attempts, last_error
         FROM deliveries ORDER BY received_at DESC LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(internal_error)?;
    Ok(Json(rows.into_iter().map(delivery_view).collect()))
}

#[derive(Serialize)]
pub struct DeliveryDetail {
    delivery: DeliveryView,
    payload: serde_json::Value,
    related_invocations: Vec<InvocationView>,
}

pub async fn delivery_detail(
    admin: AdminSession,
    State(state): State<AppState>,
    Path(guid): Path<String>,
) -> Result<Json<DeliveryDetail>, Response> {
    let _ = admin;
    let row = sqlx::query_as::<_, DeliveryRow>(
        "SELECT delivery_guid, event, payload, received_at, processed_at, state, attempts, last_error
         FROM deliveries WHERE delivery_guid = $1",
    )
    .bind(&guid)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal_error)?
    .ok_or_else(not_found)?;
    let payload = serde_json::from_slice(&row.payload).unwrap_or_else(
        |_| json!({ "unparseable_payload": String::from_utf8_lossy(&row.payload) }),
    );
    let related_invocations = related_invocations(&state.pool, &row, &payload)
        .await
        .map_err(internal_error)?;
    Ok(Json(DeliveryDetail {
        delivery: delivery_view(row),
        payload,
        related_invocations,
    }))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct InvocationView {
    id: uuid::Uuid,
    delivery_guid: Option<String>,
    owner: String,
    repo: String,
    pr_number: i64,
    actor: String,
    command: String,
    raw_comment_line: String,
    check_run_id: Option<i64>,
    workflow_run_id: Option<i64>,
    status: String,
    conclusion: Option<String>,
    failure_reason: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

const ADMIN_INVOCATION_COLUMNS: &str = "id, delivery_guid, owner, repo, pr_number, actor, command,
    raw_comment_line, check_run_id, workflow_run_id, status, conclusion, failure_reason,
    created_at, completed_at";

pub async fn invocations(
    admin: AdminSession,
    State(state): State<AppState>,
) -> Result<Json<Vec<InvocationView>>, Response> {
    let _ = admin;
    let rows = sqlx::query_as::<_, InvocationView>(&format!(
        "SELECT {ADMIN_INVOCATION_COLUMNS} FROM invocations ORDER BY created_at DESC LIMIT 200"
    ))
    .fetch_all(&state.pool)
    .await
    .map_err(internal_error)?;
    Ok(Json(rows))
}

async fn related_invocations(
    pool: &sqlx::PgPool,
    delivery: &DeliveryRow,
    payload: &serde_json::Value,
) -> Result<Vec<InvocationView>, sqlx::Error> {
    let comment_id = payload
        .pointer("/comment/id")
        .and_then(serde_json::Value::as_i64);
    let workflow_run_id = payload
        .pointer("/workflow_run/id")
        .and_then(serde_json::Value::as_i64);
    let check_run_id = payload
        .pointer("/check_run/id")
        .and_then(serde_json::Value::as_i64);
    sqlx::query_as::<_, InvocationView>(&format!(
        "SELECT {ADMIN_INVOCATION_COLUMNS} FROM invocations
         WHERE delivery_guid = $1
            OR ($2::bigint IS NOT NULL AND comment_id = $2)
            OR ($3::bigint IS NOT NULL AND workflow_run_id = $3)
            OR ($4::bigint IS NOT NULL AND check_run_id = $4)
         ORDER BY created_at DESC LIMIT 50"
    ))
    .bind(&delivery.delivery_guid)
    .bind(comment_id)
    .bind(workflow_run_id)
    .bind(check_run_id)
    .fetch_all(pool)
    .await
}

fn delivery_view(row: DeliveryRow) -> DeliveryView {
    let payload: Option<serde_json::Value> = serde_json::from_slice(&row.payload).ok();
    let action = payload
        .as_ref()
        .and_then(|value| value.get("action"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let repository = payload.as_ref().and_then(repository_name);
    DeliveryView {
        delivery_guid: row.delivery_guid,
        event: row.event,
        action,
        repository,
        received_at: row.received_at,
        processed_at: row.processed_at,
        state: row.state,
        attempts: row.attempts,
        last_error: row.last_error,
    }
}

fn repository_name(payload: &serde_json::Value) -> Option<String> {
    payload
        .pointer("/repository/full_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            let owner = payload
                .pointer("/repository/owner/login")
                .and_then(serde_json::Value::as_str)?;
            let repo = payload
                .pointer("/repository/name")
                .and_then(serde_json::Value::as_str)?;
            Some(format!("{owner}/{repo}"))
        })
}

#[derive(Clone, Copy)]
pub struct AdminSession;

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let cookie = parts
            .headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let secret = state.admin_secret.clone();
        async move {
            let Some(secret) = secret else {
                return Err(not_found());
            };
            let token = cookie_value(cookie.as_deref()).ok_or_else(|| {
                api_error(StatusCode::UNAUTHORIZED, "admin authentication required")
            })?;
            verify_token(&secret, token).map_err(|_| {
                api_error(StatusCode::UNAUTHORIZED, "admin session expired or invalid")
            })?;
            Ok(Self)
        }
    }
}

pub fn enabled(state: &AppState) -> bool {
    state.admin_secret.is_some()
}

fn sign_token(secret: &AdminSecret) -> Result<String, ()> {
    let claims = AdminClaims {
        exp: now_secs().saturating_add(ADMIN_SESSION_TTL_SECS),
    };
    let json = serde_json::to_vec(&claims).map_err(|_| ())?;
    let payload = URL_SAFE_NO_PAD.encode(json);
    let signature = hmac(secret, payload.as_bytes());
    Ok(format!("{payload}.{signature}"))
}

fn verify_token(secret: &AdminSecret, token: &str) -> Result<(), ()> {
    let (payload, signature) = token.split_once('.').ok_or(())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
    let expected = URL_SAFE_NO_PAD
        .decode(hmac(secret, payload.as_bytes()))
        .map_err(|_| ())?;
    if !constant_time_eq(&signature, &expected) {
        return Err(());
    }
    let claims: AdminClaims =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| ())?)
            .map_err(|_| ())?;
    if claims.exp < now_secs() {
        return Err(());
    }
    Ok(())
}

fn hmac(secret: &AdminSecret, value: &[u8]) -> String {
    #[allow(clippy::expect_used)]
    let mut mac = HmacSha256::new_from_slice(secret.0.as_bytes()).expect("HMAC accepts any key");
    mac.update(value);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn cookie_value(header_value: Option<&str>) -> Option<&str> {
    header_value?.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == ADMIN_COOKIE).then_some(value)
    })
}

fn set_cookie_value(token: &str) -> String {
    format!(
        "{ADMIN_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ADMIN_SESSION_TTL_SECS}"
    )
}

fn clear_cookie_value() -> String {
    format!("{ADMIN_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not found")
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn internal_error(error: sqlx::Error) -> Response {
    tracing::error!(%error, "admin database query failed");
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn secret(value: &str) -> AdminSecret {
        AdminSecret(Arc::from(value))
    }

    async fn test_state() -> Option<AppState> {
        let url = crate::test_support::test_database_url()?;
        let pool = crate::db::connect(&url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        sqlx::query(
            "TRUNCATE installations, installation_sync_state, deliveries, invocations, users CASCADE",
        )
        .execute(&pool)
        .await
        .unwrap();
        let github_app = slash_github::GithubApp::new(
            123,
            include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem"),
        )
        .unwrap();
        Some(AppState {
            pool,
            metrics: Arc::new(crate::metrics::Metrics::new().unwrap()),
            webhook_secret: Arc::from("test-webhook-secret"),
            auth_secret: crate::auth::AuthSecret(Arc::from("test-auth-secret")),
            admin_secret: Some(secret("test-admin-secret")),
            github_app: Some(Arc::new(github_app)),
            web_dir: Arc::from("."),
            github_oauth: None,
        })
    }

    #[test]
    fn token_round_trip_and_secret_rotation_invalidates_it() {
        let token = sign_token(&secret("first")).unwrap();
        assert!(verify_token(&secret("first"), &token).is_ok());
        assert!(verify_token(&secret("second"), &token).is_err());
    }

    #[test]
    fn cookie_is_http_only_strict_and_short_lived() {
        let cookie = set_cookie_value("token");
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=1200"));
        assert_eq!(
            cookie_value(Some("other=1; slash_admin_session=token")),
            Some("token")
        );
    }

    #[test]
    fn token_tampering_is_rejected() {
        let mut token = sign_token(&secret("secret")).unwrap();
        token.push('x');
        assert!(verify_token(&secret("secret"), &token).is_err());
    }

    #[test]
    fn extracts_repository_summary_without_trusting_html() {
        let payload = json!({
            "repository": {
                "full_name": "acme/widgets<script>"
            }
        });
        assert_eq!(
            repository_name(&payload).as_deref(),
            Some("acme/widgets<script>")
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn overview_reports_installations_queue_health_and_users() {
        let Some(state) = test_state().await else {
            return;
        };
        sqlx::query(
            "INSERT INTO installations (installation_id, account, target_type, state)
             VALUES (1, 'alice', 'User', 'active'), (2, 'acme', 'Organization', 'active')",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO deliveries (delivery_guid, event, payload) VALUES ('guid', 'ping', '{}'::bytea)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'Alice')")
            .bind(uuid::Uuid::new_v4())
            .execute(&state.pool)
            .await
            .unwrap();

        let response = overview(AdminSession, State(state)).await.unwrap().0;
        assert_eq!(response.active_installations, 2);
        assert_eq!(response.personal_installations, 1);
        assert_eq!(response.organization_installations, 1);
        assert_eq!(response.pending_deliveries, 1);
        assert_eq!(response.registered_users, 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn recent_installation_refresh_uses_database_snapshot_without_github_request() {
        let Some(state) = test_state().await else {
            return;
        };
        sqlx::query(
            "INSERT INTO installation_sync_state (singleton, last_success_at, installation_count)
             VALUES (true, now(), 7)",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        let response = refresh_installations(AdminSession, State(state))
            .await
            .unwrap()
            .0;
        assert!(!response.refreshed);
        assert_eq!(response.installation_count, 7);
    }
}
