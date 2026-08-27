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
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::github_user_access;
use crate::userapi::SessionUserId;

const DEFAULT_PAGE_SIZE: u8 = 50;
const MAX_PAGE_SIZE: u8 = 100;
const GITHUB_API_VERSION: &str = "2026-03-10";
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
    id: u64,
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
    id: u64,
    name: String,
    full_name: String,
    owner: String,
    private: bool,
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
        Err(response) => return response,
    };
    let url = format!("{}/user/installations", api_base_url(&state));
    let response = github_get(&url, &credential.access_token, page_number, limit).await;
    let response = match checked_github_response(response, true) {
        Ok(response) => response,
        Err(response) => return response,
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
                id: installation.id,
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
        Err(response) => return response,
    };
    let url = format!(
        "{}/user/installations/{installation_id}/repositories",
        api_base_url(&state)
    );
    let response = github_get(&url, &credential.access_token, page_number, limit).await;
    let response = match checked_github_response(response, true) {
        Ok(response) => response,
        Err(response) => return response,
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
                id: repository.id,
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

fn normalize_page(page: &PageQuery) -> Result<(u32, u8), Response> {
    let limit = page
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let page_number = match page.cursor.as_deref() {
        Some(cursor) => decode_cursor(cursor).ok_or_else(|| {
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_cursor",
                "invalid cursor",
            )
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
) -> Result<reqwest::Response, Response> {
    let response = response.map_err(|_| github_unavailable())?;
    match response.status() {
        status if status.is_success() => Ok(response),
        reqwest::StatusCode::UNAUTHORIZED if clear_on_unauthorized => {
            Err(reauthorization_required_with_clear())
        }
        reqwest::StatusCode::NOT_FOUND => Err(not_found()),
        _ => Err(github_unavailable()),
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
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
}
