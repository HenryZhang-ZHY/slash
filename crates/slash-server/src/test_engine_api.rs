//! Test Engine console read API (docs/design/1.0-test-engine.md §6 M2, UI).
//! A small set of authenticated read endpoints that back the manual-testing
//! console page (`/tests` in `web/`): list suites + tests with their current
//! disposition so a human can eyeball what's been ingested / quarantined.
//!
//! Auth: the same HttpOnly session (`UserId` extractor) as the org/user API, so
//! the console page rides the existing login. Scope at MVP = all suites for the
//! configured primary installation (`installation_id` selectable in future when
//! granting/tenancy models land).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::AppState;
use crate::test_engine;
use crate::userapi::UserId;

/// The installation the manual-testing console shows (tenant scoping is a
/// follow-up once org/grants grants land). The first/primary installation the
/// instance is installed on.
const CONSOLE_INSTALLATION_ID: i64 = 1;

/// `GET /api/test-engine/suites` — suites for the console, each with test
/// counts by disposition.
pub async fn list_suites(
    State(state): State<AppState>,
    _auth: UserId,
) -> Result<Json<Vec<SuiteOut>>, StatusCode> {
    match test_engine::list_suites(&state.pool, CONSOLE_INSTALLATION_ID).await {
        Ok(suites) => Ok(Json(
            suites
                .into_iter()
                .map(|s| SuiteOut {
                    id: s.id.to_string(),
                    suite_key: s.suite_key,
                    owner: s.owner,
                    repo: s.repo,
                    total_tests: s.total_tests,
                    muted: s.muted,
                    skipped: s.skipped,
                })
                .collect(),
        )),
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
    _auth: UserId,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Vec<TestOut>>, StatusCode> {
    match test_engine::list_tests(&state.pool, id).await {
        Ok(tests) => Ok(Json(
            tests
                .into_iter()
                .map(|t| TestOut {
                    id: t.id.to_string(),
                    name: t.name,
                    state: t.state,
                    last_status: t.last_status,
                    last_captured: t.last_captured.map(|c| c.to_rfc3339()),
                })
                .collect(),
        )),
        Err(error) => {
            tracing::error!(%error, suite = %id, "test-engine suite tests listing failed");
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
}

#[derive(Debug, serde::Serialize)]
pub struct TestOut {
    id: String,
    name: String,
    state: String,
    last_status: Option<String>,
    last_captured: Option<String>,
}
