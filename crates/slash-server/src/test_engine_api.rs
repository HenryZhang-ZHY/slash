//! Test Engine console read API (docs/design/1.0-test-engine.md §6 M2, UI).
//! A small set of authenticated read endpoints that back the manual-testing
//! console page (`/tests` in `web/`): list suites + tests with their current
//! disposition so a human can eyeball what's been ingested / quarantined.
//!
//! Auth: the same HttpOnly session (`UserId` extractor) as the org/user API.
//! Suite management and reads are scoped to the account that created the suite.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::AppState;
use crate::test_engine;
use crate::userapi::UserId;

/// The installation the manual-testing console shows (tenant scoping is a
/// follow-up once org/grants grants land). The first/primary installation the
/// instance is installed on.
const CONSOLE_INSTALLATION_ID: i64 = 1;

#[derive(Debug, Deserialize)]
pub struct CreateSuiteRequest {
    owner: String,
    repo: String,
    suite_key: String,
}

struct NormalizedCreateSuite {
    owner: String,
    repo: String,
    suite_key: String,
}

fn normalize_create_suite(request: CreateSuiteRequest) -> Option<NormalizedCreateSuite> {
    let normalized = NormalizedCreateSuite {
        owner: request.owner.trim().to_string(),
        repo: request.repo.trim().to_string(),
        suite_key: request.suite_key.trim().to_string(),
    };
    let fields = [&normalized.owner, &normalized.repo, &normalized.suite_key];
    fields
        .iter()
        .all(|field| !field.is_empty() && field.len() <= 255)
        .then_some(normalized)
}

/// `POST /api/test-engine/suites` — create (or reuse) a suite and issue its
/// first collection token so a new repository can bootstrap ingestion.
pub async fn create_suite(
    State(state): State<AppState>,
    auth_user: UserId,
    Json(request): Json<CreateSuiteRequest>,
) -> Response {
    let Some(suite) = normalize_create_suite(request) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorOut {
                message: "owner, repo, and suite_key are required",
            }),
        )
            .into_response();
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, "test-engine suite transaction failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let suite_id = match test_engine::upsert_owned_suite(
        &mut tx,
        &test_engine::NewSuite {
            installation_id: CONSOLE_INSTALLATION_ID,
            owner: &suite.owner,
            repo: &suite.repo,
            suite_key: &suite.suite_key,
        },
        auth_user.0,
    )
    .await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                Json(ErrorOut {
                    message: "suite is owned by another account",
                }),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(%error, "test-engine suite creation failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, "test-engine suite commit failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let token = match test_engine::issue_recoverable_collection_token(
        &state.pool,
        suite_id,
        &state.auth_secret,
    )
    .await
    {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, suite = %suite_id, "test-engine initial token issue failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    (
        StatusCode::CREATED,
        Json(SuiteCreated {
            suite: SuiteOut {
                id: suite_id.to_string(),
                suite_key: suite.suite_key,
                owner: suite.owner,
                repo: suite.repo,
                total_tests: 0,
                muted: 0,
                skipped: 0,
                run_count: 0,
                execution_count: 0,
                passed_executions: 0,
                failed_executions: 0,
                skipped_executions: 0,
                errored_executions: 0,
                average_duration_ms: None,
                last_captured: None,
            },
            token,
        }),
    )
        .into_response()
}

/// `GET /api/test-engine/suites` — suites for the console, each with test
/// counts by disposition.
pub async fn list_suites(
    State(state): State<AppState>,
    auth_user: UserId,
) -> Result<Json<Vec<SuiteOut>>, StatusCode> {
    match test_engine::list_suites(&state.pool, CONSOLE_INSTALLATION_ID, auth_user.0).await {
        Ok(suites) => Ok(Json(suites.into_iter().map(SuiteOut::from).collect())),
        Err(error) => {
            tracing::error!(%error, "test-engine suites listing failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// `GET /api/test-engine/suites/{id}/tests` — a suite's tests with disposition
/// and latest execution status.
pub async fn list_tests(
    State(state): State<AppState>,
    auth_user: UserId,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Vec<TestOut>>, StatusCode> {
    require_suite_owner(&state, id, auth_user.0).await?;
    match test_engine::list_tests(&state.pool, id, auth_user.0).await {
        Ok(tests) => Ok(Json(tests.into_iter().map(TestOut::from).collect())),
        Err(error) => {
            tracing::error!(%error, suite = %id, "test-engine suite tests listing failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// `GET /api/test-engine/tests/{id}/executions` — recent per-case execution
/// history, scoped to suites owned by the authenticated account.
pub async fn list_test_executions(
    State(state): State<AppState>,
    auth_user: UserId,
    Path(id): Path<uuid::Uuid>,
    Query(page): Query<ExecutionPageRequest>,
) -> Result<Json<TestExecutionPageOut>, StatusCode> {
    let limit = page.limit.clamp(1, 200);
    let offset = page.offset.max(0);
    match test_engine::list_test_executions(&state.pool, id, auth_user.0, limit, offset).await {
        Ok(page) => Ok(Json(TestExecutionPageOut {
            total: page.total,
            limit,
            offset,
            items: page
                .items
                .into_iter()
                .map(TestExecutionOut::from)
                .collect(),
        })),
        Err(error) => {
            tracing::error!(%error, test = %id, "test-engine execution history listing failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SetTestStateRequest {
    state: String,
}

/// `PATCH /api/test-engine/tests/{id}/state` — manually changes a case's
/// disposal state after verifying the case belongs to the authenticated user.
pub async fn set_test_state(
    State(state): State<AppState>,
    auth_user: UserId,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<SetTestStateRequest>,
) -> Result<Json<TestStateOut>, StatusCode> {
    let target = match request.state.as_str() {
        "enabled" => test_engine::TestState::Enabled,
        "muted" => test_engine::TestState::Muted,
        "skipped" => test_engine::TestState::Skipped,
        _ => return Err(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match test_engine::test_owned_by(&state.pool, id, auth_user.0).await {
        Ok(true) => {}
        Ok(false) => return Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, test = %id, "test-engine test ownership lookup failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    let from = [
        test_engine::TestState::Enabled,
        test_engine::TestState::Muted,
        test_engine::TestState::Skipped,
    ];
    match test_engine::set_test_state(&state.pool, id, &from, target).await {
        Ok(true) => Ok(Json(TestStateOut {
            state: target.as_str(),
        })),
        Ok(false) => Err(StatusCode::CONFLICT),
        Err(error) => {
            tracing::error!(%error, test = %id, "test-engine test state update failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// `POST /api/test-engine/suites/{id}/tokens` — issue a new per-suite
/// collection token (M2-4 token management). Its hash authenticates ingestion;
/// its encrypted value remains available to the owning account.
pub async fn issue_token(
    State(state): State<AppState>,
    auth_user: UserId,
    Path(id): Path<uuid::Uuid>,
) -> Result<(StatusCode, Json<TokenIssued>), StatusCode> {
    require_suite_owner(&state, id, auth_user.0).await?;
    match test_engine::issue_recoverable_collection_token(&state.pool, id, &state.auth_secret).await
    {
        Ok(raw) => Ok((StatusCode::CREATED, Json(TokenIssued { token: raw }))),
        Err(error) => {
            tracing::error!(%error, suite = %id, "test-engine token issue failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// `GET /api/test-engine/suites/{id}/tokens` — return the latest active token
/// to its owning account. Legacy hash-only tokens return `null` until rotated.
pub async fn get_token(
    State(state): State<AppState>,
    auth_user: UserId,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<CurrentToken>, StatusCode> {
    require_suite_owner(&state, id, auth_user.0).await?;
    match test_engine::latest_collection_token(&state.pool, id, auth_user.0, &state.auth_secret)
        .await
    {
        Ok(token) => Ok(Json(CurrentToken { token })),
        Err(error) => {
            tracing::error!(%error, suite = %id, "test-engine token read failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Body for token revocation (the raw token the caller still holds; it is
/// hashed and looked up to revoke).
#[derive(Debug, Deserialize)]
pub struct RevokeTokenRequest {
    token: String,
}

/// `POST /api/test-engine/suites/{id}/tokens/revoke` — revoke a collection
/// token. 204 on success; 404 if unknown or already revoked. Fail-closed:
/// revoked tokens no longer authenticate.
pub async fn revoke_token(
    State(state): State<AppState>,
    auth_user: UserId,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<RevokeTokenRequest>,
) -> Result<StatusCode, StatusCode> {
    require_suite_owner(&state, id, auth_user.0).await?;
    match test_engine::revoke_collection_token(&state.pool, id, &req.token).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, suite = %id, "test-engine token revoke failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SuiteOut {
    id: String,
    suite_key: String,
    owner: String,
    repo: String,
    total_tests: i64,
    muted: i64,
    skipped: i64,
    run_count: i64,
    execution_count: i64,
    passed_executions: i64,
    failed_executions: i64,
    skipped_executions: i64,
    errored_executions: i64,
    average_duration_ms: Option<f64>,
    last_captured: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct TestOut {
    id: String,
    name: String,
    state: String,
    file: Option<String>,
    line_no: Option<i32>,
    labels: Vec<String>,
    owner_team_ids: Vec<String>,
    created_at: String,
    updated_at: String,
    last_status: Option<String>,
    last_captured: Option<String>,
    last_run_ref: Option<String>,
    last_ci_provider: Option<String>,
    execution_count: i64,
    passed_count: i64,
    failed_count: i64,
    skipped_count: i64,
    errored_count: i64,
    average_duration_ms: Option<f64>,
}

#[derive(Debug, serde::Serialize)]
pub struct TestExecutionOut {
    id: String,
    status: String,
    duration_ms: i64,
    stack: Option<String>,
    captured_at: String,
    run_id: String,
    run_ref: String,
    ci_provider: String,
    started_at: String,
    finished_at: Option<String>,
    invocation_id: Option<String>,
}

impl From<test_engine::SuiteSummary> for SuiteOut {
    fn from(s: test_engine::SuiteSummary) -> Self {
        Self {
            id: s.id.to_string(),
            suite_key: s.suite_key,
            owner: s.owner,
            repo: s.repo,
            total_tests: s.total_tests,
            muted: s.muted,
            skipped: s.skipped,
            run_count: s.run_count,
            execution_count: s.execution_count,
            passed_executions: s.passed_executions,
            failed_executions: s.failed_executions,
            skipped_executions: s.skipped_executions,
            errored_executions: s.errored_executions,
            average_duration_ms: s.average_duration_ms,
            last_captured: s.last_captured.map(|captured| captured.to_rfc3339()),
        }
    }
}

impl From<test_engine::TestSummary> for TestOut {
    fn from(t: test_engine::TestSummary) -> Self {
        Self {
            id: t.id.to_string(),
            name: t.name,
            state: t.state,
            file: t.file,
            line_no: t.line_no,
            labels: t.labels,
            owner_team_ids: t
                .owner_team_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            created_at: t.created_at.to_rfc3339(),
            updated_at: t.updated_at.to_rfc3339(),
            last_status: t.last_status,
            last_captured: t.last_captured.map(|c| c.to_rfc3339()),
            last_run_ref: t.last_run_ref,
            last_ci_provider: t.last_ci_provider,
            execution_count: t.execution_count,
            passed_count: t.passed_count,
            failed_count: t.failed_count,
            skipped_count: t.skipped_count,
            errored_count: t.errored_count,
            average_duration_ms: t.average_duration_ms,
        }
    }
}

impl From<test_engine::TestExecutionSummary> for TestExecutionOut {
    fn from(e: test_engine::TestExecutionSummary) -> Self {
        Self {
            id: e.id.to_string(),
            status: e.status,
            duration_ms: e.duration_ms,
            stack: e.stack,
            captured_at: e.captured_at.to_rfc3339(),
            run_id: e.run_id.to_string(),
            run_ref: e.run_ref,
            ci_provider: e.ci_provider,
            started_at: e.started_at.to_rfc3339(),
            finished_at: e.finished_at.map(|finished| finished.to_rfc3339()),
            invocation_id: e.invocation_id.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExecutionPageRequest {
    #[serde(default = "default_execution_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_execution_limit() -> i64 {
    100
}

#[derive(Debug, serde::Serialize)]
pub struct TestExecutionPageOut {
    total: i64,
    limit: i64,
    offset: i64,
    items: Vec<TestExecutionOut>,
}

#[derive(Debug, serde::Serialize)]
pub struct TestStateOut {
    state: &'static str,
}

#[derive(Debug, serde::Serialize)]
pub struct TokenIssued {
    token: String,
}

#[derive(Debug, serde::Serialize)]
pub struct CurrentToken {
    token: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct SuiteCreated {
    suite: SuiteOut,
    token: String,
}

#[derive(Debug, serde::Serialize)]
struct ErrorOut {
    message: &'static str,
}

async fn require_suite_owner(
    state: &AppState,
    suite_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<(), StatusCode> {
    match test_engine::suite_owned_by(&state.pool, suite_id, user_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(error) => {
            tracing::error!(%error, suite = %suite_id, "test-engine suite ownership lookup failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_suite_fields_are_trimmed() {
        let request = CreateSuiteRequest {
            owner: " HenryZhang-ZHY ".to_string(),
            repo: " slash ".to_string(),
            suite_key: " ci-test ".to_string(),
        };

        let normalized = normalize_create_suite(request).expect("request should be valid");

        assert_eq!(normalized.owner, "HenryZhang-ZHY");
        assert_eq!(normalized.repo, "slash");
        assert_eq!(normalized.suite_key, "ci-test");
    }

    #[test]
    fn create_suite_rejects_blank_fields() {
        for request in [
            CreateSuiteRequest {
                owner: "".to_string(),
                repo: "slash".to_string(),
                suite_key: "ci-test".to_string(),
            },
            CreateSuiteRequest {
                owner: "HenryZhang-ZHY".to_string(),
                repo: "  ".to_string(),
                suite_key: "ci-test".to_string(),
            },
            CreateSuiteRequest {
                owner: "HenryZhang-ZHY".to_string(),
                repo: "slash".to_string(),
                suite_key: "\t".to_string(),
            },
        ] {
            assert!(normalize_create_suite(request).is_none());
        }
    }
}
