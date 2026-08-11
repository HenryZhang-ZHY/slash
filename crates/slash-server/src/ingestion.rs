//! `POST /v1/test-engine/upload` — the M1 ingestion endpoint (docs/design/
//! 1.0-test-engine.md §4, §5). Accepts a normalized raw-JSON batch of test
//! executions, authenticated by a per-suite collection token (design §4),
//! and writes it durably into the test engine record. This is the slim,
//! server-side collector form pinned by §8 Q2 — no compiled client binary.
//!
//! The whole batch is written in one transaction so a partial upload never
//! leaves a run recorded without its executions.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::test_engine::{
    ExecutionStatus, find_suite_for_token, insert_executions, quarantined_tests, upsert_run,
    upsert_test,
};

/// The normalized raw-JSON payload accepted by the ingestion endpoint.
#[derive(Debug, Deserialize)]
struct UploadPayload {
    /// Identity of the CI run that produced this batch (design §3).
    #[serde(flatten)]
    run: RunPayload,
    /// The observed executions, one per test.
    executions: Vec<ExecutionPayload>,
}

#[derive(Debug, Deserialize)]
struct RunPayload {
    ci_provider: String,
    run_ref: String,
    /// Set when the run was triggered by a slash command; else absent.
    #[serde(default)]
    invocation_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct ExecutionPayload {
    name: String,
    status: String,
    #[serde(default)]
    duration_ms: i64,
    #[serde(default)]
    stack: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    line_no: Option<i32>,
}

/// Attempts to extract a per-suite collection token from the Authorization
/// header (`Bearer <token>`), returning `None` if absent or malformed.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let rest = value.strip_prefix("Bearer ")?;
    Some(rest.trim())
}

/// `POST /v1/test-engine/upload`
///
/// 200 on a freshly accepted batch; 401 on a missing/unknown collection token;
/// 400 on an unparseable body or unknown execution status; 500 on a storage
/// failure. The write is all-or-nothing in a single transaction.
pub async fn handle_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(raw_token) = bearer_token(&headers) else {
        return StatusCode::UNAUTHORIZED;
    };

    // Resolve the suite + tenancy from the token (design §4): the token is the
    // authority for which suite this upload belongs to.
    let identity = match find_suite_for_token(&state.pool, raw_token).await {
        Ok(Some(identity)) => identity,
        // A token-lookup failure is indistinguishable from a bad token from
        // the client's perspective; both must fail closed (never an allow on
        // an auth error). Surface 500 with a 401 only when unknown.
        Ok(None) => return StatusCode::UNAUTHORIZED,
        Err(error) => {
            tracing::error!(%error, "test-engine token lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    let payload: UploadPayload = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::debug!(%error, "test-engine upload body rejected");
            return StatusCode::BAD_REQUEST;
        }
    };

    // Normalize execution statuses before touching the database; an unknown
    // status is a 400, not a write with a garbage value.
    let mut parsed = Vec::with_capacity(payload.executions.len());
    for exec in &payload.executions {
        let status = match parse_execution_status(&exec.status) {
            Some(status) => status,
            None => return StatusCode::BAD_REQUEST,
        };
        parsed.push((exec, status));
    }

    // Write the whole batch atomically.
    let result = write_batch(&state.pool, &identity, &payload.run, &parsed).await;

    match result {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::error!(%error, suite = %identity.suite_key, "test-engine upload write failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Persists one batch within a single transaction: resolve/create the suite,
/// resolve/create each test, upsert the run, then append executions.
async fn write_batch(
    pool: &PgPool,
    identity: &crate::test_engine::SuiteTokenIdentity,
    run: &RunPayload,
    executions: &[(&ExecutionPayload, ExecutionStatus)],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let suite_id = identity.suite_id;

    // Resolve each test, returning its id for the execution FK.
    let mut test_ids = Vec::with_capacity(executions.len());
    for (exec, _) in executions {
        let test_ref = upsert_test(
            &mut tx,
            suite_id,
            &crate::test_engine::NewTest {
                name: &exec.name,
                file: exec.file.as_deref(),
                line_no: exec.line_no,
                owner_team_ids: Vec::new(),
            },
        )
        .await?;
        test_ids.push(test_ref);
    }

    let run_id = upsert_run(
        &mut tx,
        &crate::test_engine::NewRun {
            suite_id,
            installation_id: identity.installation_id,
            ci_provider: &run.ci_provider,
            run_ref: &run.run_ref,
            invocation_id: run.invocation_id,
        },
    )
    .await?;

    let new_executions: Vec<crate::test_engine::NewExecution<'_>> = executions
        .iter()
        .zip(test_ids.iter())
        .map(
            |((exec, status), test_ref)| crate::test_engine::NewExecution {
                test_id: test_ref.id,
                status: *status,
                duration_ms: exec.duration_ms,
                stack: exec.stack.as_deref(),
            },
        )
        .collect();

    insert_executions(&mut tx, run_id, &new_executions).await?;

    tx.commit().await
}

fn parse_execution_status(status: &str) -> Option<ExecutionStatus> {
    match status {
        "passed" => Some(ExecutionStatus::Passed),
        "failed" => Some(ExecutionStatus::Failed),
        "skipped" => Some(ExecutionStatus::Skipped),
        "errored" => Some(ExecutionStatus::Errored),
        _ => None,
    }
}

/// `GET /v1/test-engine/quarantined` — the M1 disposal hook (design §5,
/// task M1-4). Authenticated by the same per-suite collection token as the
/// upload endpoint; returns the names of tests in the suite currently
/// quarantined (`muted` or `skipped`) so a slash-commanded test workflow can
/// skip/soft-fail them instead of running them — the bktec "skip/mute flaky"
/// behavior, server-side.
///
/// 200 with a JSON array of quarantined test names; 401 on a missing/unknown
/// token; 500 on a storage failure.
pub async fn handle_quarantined(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, axum::Json<serde_json::Value>), StatusCode> {
    let Some(raw_token) = bearer_token(&headers) else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let identity = match find_suite_for_token(&state.pool, raw_token).await {
        Ok(Some(identity)) => identity,
        Ok(None) => return Err(StatusCode::UNAUTHORIZED),
        Err(error) => {
            tracing::error!(%error, "test-engine token lookup failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match quarantined_tests(&state.pool, identity.suite_id).await {
        Ok(names) => Ok((StatusCode::OK, axum::Json(serde_json::json!(names)))),
        Err(error) => {
            tracing::error!(%error, suite = %identity.suite_key, "test-engine quarantined lookup failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
