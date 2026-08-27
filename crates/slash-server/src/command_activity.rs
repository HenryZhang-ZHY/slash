//! Repository discovery and user-visible command invocation history.
//!
//! GitHub user credentials discover the intersection of App and user access.
//! History reads independently re-check live collaborator access before any
//! tenant data is returned. See `docs/design/command-invocation-history.md`.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header::{COOKIE, HeaderValue, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use slash_core::{InvocationStatus, ResolvedRole};
use slash_github::RepoClient;
use sqlx::FromRow;
use uuid::Uuid;

use crate::AppState;
use crate::github_user_access;
use crate::userapi::SessionUserId;

const DEFAULT_PAGE_SIZE: u8 = 50;
const MAX_PAGE_SIZE: u8 = 100;
const GITHUB_API_VERSION: &str = "2026-03-10";

type ActivityResult<T> = Result<T, Box<Response>>;
const GITHUB_CONNECTION_ID: Uuid = Uuid::from_u128(1);

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    cursor: Option<String>,
    limit: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubInstallationPage {
    total_count: u64,
    installations: Vec<GithubInstallation>,
}

#[derive(Debug, Deserialize)]
struct GithubInstallation {
    id: u64,
    account: GithubAccount,
    target_type: String,
}

#[derive(Debug, Deserialize)]
struct GithubAccount {
    login: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct InstallationView {
    id: String,
    account: String,
    target_type: String,
}

#[derive(Debug, Deserialize)]
struct GithubRepositoryPage {
    total_count: u64,
    repositories: Vec<GithubRepository>,
}

#[derive(Debug, Deserialize)]
struct GithubRepository {
    id: u64,
    name: String,
    full_name: String,
    private: bool,
    owner: GithubAccount,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RepositoryView {
    id: String,
    name: String,
    full_name: String,
    owner: String,
    private: bool,
}

#[derive(Debug, Deserialize)]
pub struct InvocationQuery {
    installation_id: u64,
    repository_id: u64,
    owner: String,
    repo: String,
    status: Option<String>,
    command: Option<String>,
    cursor: Option<String>,
    limit: Option<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
struct InvocationCursor {
    created_at: String,
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct InvocationRow {
    id: Uuid,
    comment_id: i64,
    attempt: i32,
    pr_number: i64,
    head_sha: String,
    actor: String,
    command: String,
    check_run_id: Option<i64>,
    workflow_run_id: Option<i64>,
    status: String,
    conclusion: Option<String>,
    created_at: DateTime<Utc>,
    dispatched_at: Option<DateTime<Utc>>,
    correlated_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct InvocationView {
    id: Uuid,
    attempt: i32,
    pr_number: i64,
    head_sha: String,
    actor: String,
    command: String,
    status: String,
    conclusion: Option<String>,
    created_at: DateTime<Utc>,
    dispatched_at: Option<DateTime<Utc>>,
    correlated_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    pull_url: String,
    comment_url: String,
    check_url: Option<String>,
    workflow_run_url: Option<String>,
}

pub async fn list_github_installations(
    State(state): State<AppState>,
    SessionUserId(user_id): SessionUserId,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Response {
    let credential = match discovery_credential(&state, user_id, &headers).await {
        Ok(credential) => credential,
        Err(response) => return response,
    };
    let (page_number, limit) = match normalize_page(&page) {
        Ok(page) => page,
        Err(response) => return *response,
    };
    let url = format!("{}/user/installations", api_base_url(&state));
    let response = github_get(&url, &credential.access_token, page_number, limit).await;
    let response = match checked_github_response(response, true) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    let payload: GithubInstallationPage = match response.json().await {
        Ok(payload) => payload,
        Err(_) => return github_unavailable(),
    };
    let next_cursor = next_cursor(page_number, limit, payload.total_count);
    Json(Page {
        items: payload
            .installations
            .into_iter()
            .map(|installation| InstallationView {
                id: installation.id.to_string(),
                account: installation.account.login,
                target_type: installation.target_type,
            })
            .collect(),
        next_cursor,
    })
    .into_response()
}

pub async fn list_github_repositories(
    State(state): State<AppState>,
    SessionUserId(user_id): SessionUserId,
    headers: HeaderMap,
    Path(installation_id): Path<u64>,
    Query(page): Query<PageQuery>,
) -> Response {
    let credential = match discovery_credential(&state, user_id, &headers).await {
        Ok(credential) => credential,
        Err(response) => return response,
    };
    let (page_number, limit) = match normalize_page(&page) {
        Ok(page) => page,
        Err(response) => return *response,
    };
    let url = format!(
        "{}/user/installations/{installation_id}/repositories",
        api_base_url(&state)
    );
    let response = github_get(&url, &credential.access_token, page_number, limit).await;
    let response = match checked_github_response(response, true) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    let payload: GithubRepositoryPage = match response.json().await {
        Ok(payload) => payload,
        Err(_) => return github_unavailable(),
    };
    let next_cursor = next_cursor(page_number, limit, payload.total_count);
    Json(Page {
        items: payload
            .repositories
            .into_iter()
            .map(|repository| RepositoryView {
                id: repository.id.to_string(),
                name: repository.name,
                full_name: repository.full_name,
                owner: repository.owner.login,
                private: repository.private,
            })
            .collect(),
        next_cursor,
    })
    .into_response()
}

pub async fn list_invocations(
    State(state): State<AppState>,
    SessionUserId(user_id): SessionUserId,
    Query(query): Query<InvocationQuery>,
) -> Response {
    let query = match normalize_invocation_query(query) {
        Ok(query) => query,
        Err(response) => return *response,
    };
    if let Err(response) = authorize_repository(&state, user_id, &query).await {
        return response;
    }
    let cursor = match query.cursor.as_deref().map(decode_invocation_cursor) {
        Some(Some(cursor)) => Some(cursor),
        Some(None) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_cursor",
                "invalid cursor",
            );
        }
        None => None,
    };
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let cursor_at = cursor.as_ref().map(|cursor| cursor.0);
    let cursor_id = cursor.as_ref().map(|cursor| cursor.1);
    let rows = sqlx::query_as::<_, InvocationRow>(
        "SELECT id, comment_id, attempt, pr_number, head_sha, actor, command,
                check_run_id, workflow_run_id, status, conclusion, created_at,
                dispatched_at, correlated_at, completed_at
         FROM invocations
         WHERE installation_id = $1 AND repository_id = $2
           AND ($3::text IS NULL OR status = $3)
           AND ($4::text IS NULL OR command = $4)
           AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6))
         ORDER BY created_at DESC, id DESC
         LIMIT $7",
    )
    .bind(query.installation_id as i64)
    .bind(query.repository_id as i64)
    .bind(query.status.as_deref())
    .bind(query.command.as_deref())
    .bind(cursor_at)
    .bind(cursor_id)
    .bind(i64::from(limit) + 1)
    .fetch_all(&state.pool)
    .await;
    let mut rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "invocation activity query failed");
            return internal_error();
        }
    };
    let has_more = rows.len() > usize::from(limit);
    rows.truncate(usize::from(limit));
    let next_cursor = has_more
        .then(|| {
            rows.last()
                .map(|row| encode_invocation_cursor(row.created_at, row.id))
        })
        .flatten();
    Json(Page {
        items: rows
            .into_iter()
            .map(|row| invocation_view(row, &query.owner, &query.repo))
            .collect(),
        next_cursor,
    })
    .into_response()
}

fn normalize_invocation_query(mut query: InvocationQuery) -> ActivityResult<InvocationQuery> {
    if !valid_github_name(&query.owner) || !valid_github_name(&query.repo) {
        return Err(Box::new(error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_repository",
            "invalid repository",
        )));
    }
    if query.installation_id > i64::MAX as u64 || query.repository_id > i64::MAX as u64 {
        return Err(Box::new(error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_repository",
            "invalid repository",
        )));
    }
    if query
        .status
        .as_deref()
        .is_some_and(|status| InvocationStatus::parse(status).is_none())
    {
        return Err(Box::new(error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_status",
            "invalid invocation status",
        )));
    }
    query.command = query
        .command
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty());
    if query.command.as_ref().is_some_and(|command| {
        command.len() > 64
            || !command
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) {
        return Err(Box::new(error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_command",
            "invalid command",
        )));
    }
    Ok(query)
}

fn valid_github_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

async fn authorize_repository(
    state: &AppState,
    user_id: Uuid,
    query: &InvocationQuery,
) -> Result<(), Response> {
    let username: Option<String> = sqlx::query_scalar(
        "SELECT ui.username
         FROM user_identities ui
         JOIN users u ON u.id = ui.user_id
         WHERE ui.user_id = $1 AND ui.connection_id = $2 AND u.status = 'active'",
    )
    .bind(user_id)
    .bind(GITHUB_CONNECTION_ID)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, user_id = %user_id, "github identity lookup failed");
        internal_error()
    })?;
    let username = username.ok_or_else(not_found)?;
    let app = state.github_app.as_ref().ok_or_else(github_unavailable)?;
    let token = crate::installations::mint_installation_token(
        &state.pool,
        app,
        query.installation_id,
        query.repository_id,
        &[("metadata", "read")],
    )
    .await
    .map_err(|error| match error {
        slash_github::AppAuthError::InstallationGone { .. } => not_found(),
        other => {
            tracing::warn!(error = %other, "command activity installation token mint failed");
            github_unavailable()
        }
    })?;
    let client =
        RepoClient::with_base_uri(&token, &query.owner, &query.repo, Some(api_base_url(state)))
            .map_err(|error| {
                tracing::error!(%error, "command activity GitHub client build failed");
                internal_error()
            })?;
    let permission = client
        .get_collaborator_permission(&username)
        .await
        .map_err(|error| {
            if error.status_code() == Some(404) {
                not_found()
            } else {
                tracing::warn!(%error, "command activity collaborator lookup failed");
                github_unavailable()
            }
        })?;
    let role = ResolvedRole::from_role_name(&permission.role_name).unwrap_or_else(|| {
        ResolvedRole::from_permission_booleans(
            permission.user.permissions.admin,
            permission.user.permissions.maintain,
            permission.user.permissions.push,
            permission.user.permissions.triage,
            permission.user.permissions.pull,
        )
    });
    if role < ResolvedRole::Read {
        return Err(not_found());
    }
    Ok(())
}

fn encode_invocation_cursor(created_at: DateTime<Utc>, id: Uuid) -> String {
    let cursor = InvocationCursor {
        created_at: created_at.to_rfc3339(),
        id,
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap_or_default())
}

fn decode_invocation_cursor(cursor: &str) -> Option<(DateTime<Utc>, Uuid)> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let cursor: InvocationCursor = serde_json::from_slice(&bytes).ok()?;
    let created_at = DateTime::parse_from_rfc3339(&cursor.created_at)
        .ok()?
        .with_timezone(&Utc);
    Some((created_at, cursor.id))
}

fn invocation_view(row: InvocationRow, owner: &str, repo: &str) -> InvocationView {
    let root = format!("https://github.com/{owner}/{repo}");
    InvocationView {
        id: row.id,
        attempt: row.attempt,
        pr_number: row.pr_number,
        head_sha: row.head_sha,
        actor: row.actor,
        command: row.command,
        status: row.status,
        conclusion: row.conclusion,
        created_at: row.created_at,
        dispatched_at: row.dispatched_at,
        correlated_at: row.correlated_at,
        completed_at: row.completed_at,
        pull_url: format!("{root}/pull/{}", row.pr_number),
        comment_url: format!(
            "{root}/pull/{}#issuecomment-{}",
            row.pr_number, row.comment_id
        ),
        check_url: row
            .check_run_id
            .map(|check_run_id| format!("{root}/runs/{check_run_id}")),
        workflow_run_url: row
            .workflow_run_id
            .map(|run_id| format!("{root}/actions/runs/{run_id}")),
    }
}

async fn discovery_credential(
    state: &AppState,
    user_id: Uuid,
    headers: &HeaderMap,
) -> Result<github_user_access::Credential, Response> {
    let token = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| github_user_access::token_from_header(Some(header)))
        .ok_or_else(reauthorization_required)?;
    let credential = github_user_access::open(&state.auth_secret, &token)
        .map_err(|_| reauthorization_required_with_clear())?;
    if credential.user_id != user_id {
        return Err(reauthorization_required_with_clear());
    }
    let subject: Option<String> = sqlx::query_scalar(
        "SELECT subject FROM user_identities
         WHERE user_id = $1 AND connection_id = $2",
    )
    .bind(user_id)
    .bind(GITHUB_CONNECTION_ID)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, user_id = %user_id, "github identity lookup failed");
        internal_error()
    })?;
    if subject.as_deref() != Some(credential.github_subject.as_str()) {
        return Err(reauthorization_required_with_clear());
    }
    Ok(credential)
}

fn normalize_page(page: &PageQuery) -> ActivityResult<(u32, u8)> {
    let limit = page
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let page_number = match page.cursor.as_deref() {
        Some(cursor) => decode_cursor(cursor).ok_or_else(|| {
            Box::new(error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_cursor",
                "invalid cursor",
            ))
        })?,
        None => 1,
    };
    Ok((page_number, limit))
}

fn encode_cursor(page: u32) -> String {
    URL_SAFE_NO_PAD.encode(page.to_be_bytes())
}

fn decode_cursor(cursor: &str) -> Option<u32> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let bytes: [u8; 4] = bytes.try_into().ok()?;
    let page = u32::from_be_bytes(bytes);
    (page > 0).then_some(page)
}

fn next_cursor(page: u32, limit: u8, total: u64) -> Option<String> {
    (u64::from(page) * u64::from(limit) < total).then(|| encode_cursor(page + 1))
}

async fn github_get(
    url: &str,
    access_token: &str,
    page: u32,
    limit: u8,
) -> Result<reqwest::Response, reqwest::Error> {
    let url = format!("{url}?page={page}&per_page={limit}");
    reqwest::Client::new()
        .get(url)
        .bearer_auth(access_token)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", GITHUB_API_VERSION)
        .header("user-agent", "slash-server")
        .send()
        .await
}

fn checked_github_response(
    response: Result<reqwest::Response, reqwest::Error>,
    clear_on_unauthorized: bool,
) -> ActivityResult<reqwest::Response> {
    let response = response.map_err(|_| Box::new(github_unavailable()))?;
    match response.status() {
        status if status.is_success() => Ok(response),
        reqwest::StatusCode::UNAUTHORIZED if clear_on_unauthorized => {
            Err(Box::new(reauthorization_required_with_clear()))
        }
        reqwest::StatusCode::NOT_FOUND => Err(Box::new(not_found())),
        _ => Err(Box::new(github_unavailable())),
    }
}

fn api_base_url(state: &AppState) -> &str {
    state
        .github_oauth
        .as_ref()
        .map_or("https://api.github.com", |oauth| oauth.api_base_url())
        .trim_end_matches('/')
}

fn reauthorization_required() -> Response {
    error_response(
        StatusCode::FORBIDDEN,
        "github_reauthorization_required",
        "GitHub authorization is required",
    )
}

fn reauthorization_required_with_clear() -> Response {
    let mut response = reauthorization_required();
    #[allow(clippy::expect_used)]
    let cookie = HeaderValue::from_str(&github_user_access::clear_cookie_value())
        .expect("generated cookie is valid ASCII");
    response.headers_mut().append(SET_COOKIE, cookie);
    response
}

fn not_found() -> Response {
    error_response(StatusCode::NOT_FOUND, "not_found", "not found")
}

fn github_unavailable() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "github_unavailable",
        "GitHub is unavailable",
    )
}

fn internal_error() -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "could not load command activity",
    )
}

fn error_response(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": code, "message": message})),
    )
        .into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use axum::body::to_bytes;
    use chrono::Duration;
    use sqlx::PgPool;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_KEY_PEM: &[u8] =
        include_bytes!("../../slash-github/tests/fixtures/test-app-key.pem");

    fn invocation_query() -> InvocationQuery {
        InvocationQuery {
            installation_id: 1,
            repository_id: 2,
            owner: "acme".into(),
            repo: "widgets".into(),
            status: None,
            command: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn pagination_cursor_is_opaque_bounded_and_round_trips() {
        let cursor = encode_cursor(42);
        assert_ne!(cursor, "42");
        assert_eq!(decode_cursor(&cursor), Some(42));
        assert_eq!(decode_cursor("not-a-cursor"), None);
        assert_eq!(next_cursor(1, 50, 51), Some(encode_cursor(2)));
        assert_eq!(next_cursor(2, 50, 100), None);
    }

    #[test]
    fn page_size_defaults_and_is_bounded() {
        assert_eq!(
            normalize_page(&PageQuery {
                cursor: None,
                limit: None,
            })
            .unwrap(),
            (1, 50)
        );
        assert_eq!(
            normalize_page(&PageQuery {
                cursor: None,
                limit: Some(0),
            })
            .unwrap(),
            (1, 1)
        );
    }

    #[tokio::test]
    async fn github_discovery_request_is_user_authenticated_and_paginated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/installations"))
            .and(header("authorization", "Bearer ghu_discovery"))
            .and(query_param("page", "2"))
            .and(query_param("per_page", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "total_count": 0,
                "installations": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = github_get(
            &format!("{}/user/installations", server.uri()),
            "ghu_discovery",
            2,
            50,
        )
        .await
        .unwrap();

        assert!(response.status().is_success());
    }

    #[test]
    fn invocation_cursor_round_trips_exact_ordering_key() {
        let created_at = Utc::now();
        let id = Uuid::new_v4();
        let cursor = encode_invocation_cursor(created_at, id);
        assert_eq!(decode_invocation_cursor(&cursor), Some((created_at, id)));
        assert_eq!(decode_invocation_cursor("invalid"), None);
    }

    #[test]
    fn invocation_query_rejects_untrusted_repository_and_status_values() {
        let mut invalid_repository = invocation_query();
        invalid_repository.owner = "../secret".into();
        assert!(normalize_invocation_query(invalid_repository).is_err());

        let mut invalid_status = invocation_query();
        invalid_status.status = Some("stuck".into());
        assert!(normalize_invocation_query(invalid_status).is_err());
    }

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = crate::db::connect(&url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE invocations, users CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    async fn mount_repository_authorization(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "installation-token",
                "expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339()
            })))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/widgets/collaborators/alice/permission"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "permission": "pull",
                "role_name": "read",
                "user": {
                    "login": "alice", "id": 42, "node_id": "n",
                    "avatar_url": "https://example.test/avatar", "gravatar_id": "",
                    "url": "https://example.test/users/alice",
                    "html_url": "https://example.test/alice",
                    "followers_url": "https://example.test/followers",
                    "following_url": "https://example.test/following{/other_user}",
                    "gists_url": "https://example.test/gists{/gist_id}",
                    "starred_url": "https://example.test/starred{/owner}{/repo}",
                    "subscriptions_url": "https://example.test/subscriptions",
                    "organizations_url": "https://example.test/orgs",
                    "repos_url": "https://example.test/repos",
                    "events_url": "https://example.test/events{/privacy}",
                    "received_events_url": "https://example.test/received_events",
                    "type": "User", "site_admin": false,
                    "permissions": {"admin": false, "push": false, "pull": true}
                }
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn authorized_history_returns_safe_repository_scoped_rows() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let server = MockServer::start().await;
        mount_repository_authorization(&server).await;
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'Alice')")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO user_identities
                (id, user_id, connection_id, subject, username)
             VALUES ($1, $2, $3, '42', 'alice')",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(GITHUB_CONNECTION_ID)
        .execute(&pool)
        .await
        .unwrap();
        let invocation_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO invocations (
                id, installation_id, repository_id, owner, repo, comment_id,
                pr_number, head_sha, head_branch, actor, actor_id, command,
                raw_comment_line, args, workflow_file, status, failure_reason
             ) VALUES (
                $1, 1, 2, 'acme', 'widgets', 99, 7, 'deadbeef', 'feature',
                'bob', 9, 'deploy', '/deploy secret', '{\"token\":\"secret\"}',
                'deploy.yml', 'completed', 'private upstream detail'
             )",
        )
        .bind(invocation_id)
        .execute(&pool)
        .await
        .unwrap();
        let oauth = crate::github_oauth::OauthState::new(
            Arc::from("client"),
            Arc::from("secret"),
            Arc::from("https://slash.example"),
            crate::auth::AuthSecret(Arc::from("auth-secret")),
        )
        .with_api_base_url(server.uri());
        let state = AppState {
            pool,
            metrics: Arc::new(crate::metrics::Metrics::new().unwrap()),
            webhook_secret: Arc::from("webhook"),
            auth_secret: crate::auth::AuthSecret(Arc::from("auth-secret")),
            admin_secret: None,
            github_app: Some(Arc::new(
                slash_github::GithubApp::with_base_uri(1, TEST_KEY_PEM, Some(&server.uri()))
                    .unwrap(),
            )),
            web_dir: Arc::from(""),
            github_oauth: Some(oauth),
        };

        let response = list_invocations(
            State(state),
            SessionUserId(user_id),
            Query(invocation_query()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let item = json
            .get("items")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.first())
            .unwrap();
        assert_eq!(
            item.get("id").and_then(serde_json::Value::as_str),
            Some(invocation_id.to_string().as_str())
        );
        assert_eq!(
            item.get("command").and_then(serde_json::Value::as_str),
            Some("deploy")
        );
        let rendered = String::from_utf8(body.to_vec()).unwrap();
        assert!(!rendered.contains("/deploy secret"));
        assert!(!rendered.contains("private upstream detail"));
        assert!(!rendered.contains("token"));
    }
}
