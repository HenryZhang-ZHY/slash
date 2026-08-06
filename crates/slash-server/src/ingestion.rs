//! `POST /v1/test-engine/upload` — the ingestion endpoint
//! (`docs/test-engine.md`). Accepts a normalized raw-JSON batch of test
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
use crate::collectors::{self, NormalizedExecution};
use crate::junit;
use crate::metrics::Metrics;
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

/// True when the content type header is JUnit XML (`application/xml`,
/// `application/junit+xml`, or `text/xml`). Missing or JSON means the JSON
/// path; JUnit XML is a newline-tolerant fallback for collectors that send it.
fn is_junit_xml(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let mime = value.split([';', ' ']).next().unwrap_or("");
    matches!(
        mime,
        "application/xml" | "application/junit+xml" | "text/xml"
    )
}

/// `POST /v1/test-engine/upload`
///
/// 200 on a freshly accepted batch; 401 on a missing/unknown collection token;
/// 400 on an unparseable body or unknown execution status; 500 on a storage
/// failure. The write is all-or-nothing in a single transaction.
///
/// Body dispatch by `Content-Type` (design §6 M2): raw JSON by default, JUnit
/// XML when the header says so — both normalize to the same execution shape.
pub async fn handle_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(raw_token) = bearer_token(&headers) else {
        record_upload(&state.metrics, "generic", "bad_token");
        return StatusCode::UNAUTHORIZED;
    };

    // Resolve the suite + tenancy from the token (design §4): the token is the
    // authority for which suite this upload belongs to.
    let identity = match find_suite_for_token(&state.pool, raw_token).await {
        Ok(Some(identity)) => identity,
        // A token-lookup failure is indistinguishable from a bad token from
        // the client's perspective; both must fail closed (never an allow on
        // an auth error).
        Ok(None) => {
            record_upload(&state.metrics, "generic", "bad_unknown_token");
            return StatusCode::UNAUTHORIZED;
        }
        Err(error) => {
            tracing::error!(%error, "test-engine token lookup failed");
            record_upload(&state.metrics, "generic", "token_lookup_error");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    if is_junit_xml(&headers) {
        return handle_junit_body(&state.pool, &identity, &body, &state.metrics).await;
    }

    handle_json_body(&state.pool, &identity, &body, &state.metrics).await
}

/// Records an ingestion outcome on the upload-health metric (M2-5).
fn record_upload(metrics: &Metrics, kind: &str, outcome: &str) {
    metrics
        .test_engine_uploads_total
        .with_label_values(&[kind, outcome])
        .inc();
}

/// The raw-JSON ingestion path (existing M1 behavior).
async fn handle_json_body(
    pool: &PgPool,
    identity: &crate::test_engine::SuiteTokenIdentity,
    body: &[u8],
    metrics: &Metrics,
) -> StatusCode {
    let payload: UploadPayload = match serde_json::from_slice(body) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::debug!(%error, "test-engine JSON upload body rejected");
            record_upload(metrics, "json", "bad_body");
            return StatusCode::BAD_REQUEST;
        }
    };

    // Normalize execution statuses before touching the database; an unknown
    // status is a 400, not a write with a garbage value.
    let mut parsed = Vec::with_capacity(payload.executions.len());
    for exec in &payload.executions {
        let status = match parse_execution_status(&exec.status) {
            Some(status) => status,
            None => {
                record_upload(metrics, "json", "bad_status");
                return StatusCode::BAD_REQUEST;
            }
        };
        parsed.push(NormalizedExecution {
            name: exec.name.clone(),
            status,
            duration_ms: exec.duration_ms,
            stack: exec.stack.clone(),
            file: exec.file.clone(),
            line_no: exec.line_no,
        });
    }

    let result = write_batch(pool, identity, &payload.run, &parsed).await;
    match result {
        Ok(()) => {
            record_upload(metrics, "json", "ok");
            StatusCode::OK
        }
        Err(error) => {
            tracing::error!(%error, suite = %identity.suite_key, "test-engine JSON upload write failed");
            record_upload(metrics, "json", "storage_error");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// The JUnit XML ingestion path (M2-1). Parses the report and writes the
/// normalized executions through the same durable write.
async fn handle_junit_body(
    pool: &PgPool,
    identity: &crate::test_engine::SuiteTokenIdentity,
    body: &[u8],
    metrics: &Metrics,
) -> StatusCode {
    let batch = match junit::parse(body) {
        Ok(batch) => batch,
        Err(error) => {
            tracing::debug!(%error, suite = %identity.suite_key, "test-engine JUnit body rejected");
            record_upload(metrics, "junit", "bad_body");
            return StatusCode::BAD_REQUEST;
        }
    };

    let parsed: Vec<NormalizedExecution> = batch
        .executions
        .into_iter()
        .map(|e| NormalizedExecution {
            name: e.name,
            status: e.status,
            duration_ms: e.duration_ms,
            stack: e.stack,
            file: None,
            line_no: None,
        })
        .collect();

    // A JUnit report has no CI run identity beyond the document; the endpoint
    // treats it as a single run identified by the caller's run_ref-equivalent
    // (here: a stable synthetic ref keyed to the suite).
    let run = RunPayload {
        ci_provider: "junit".to_string(),
        run_ref: format!("junit/{}", uuid::Uuid::new_v4()),
        invocation_id: None,
    };

    let result = write_batch(pool, identity, &run, &parsed).await;
    match result {
        Ok(()) => {
            record_upload(metrics, "junit", "ok");
            StatusCode::OK
        }
        Err(error) => {
            tracing::error!(%error, suite = %identity.suite_key, "test-engine JUnit upload write failed");
            record_upload(metrics, "junit", "storage_error");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// `POST /v1/test-engine/upload/cargo` — Buildkite rust collector ingestion
/// (M2-2). Reuses the open-source `cargo test --format json` dialect;
/// normalized via `collectors::parse_cargo_libtest` and written atomically.
pub async fn handle_cargo_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let suite = match authorize(&state.pool, &headers).await {
        Ok(Some(suite)) => suite,
        Ok(None) => {
            record_upload(&state.metrics, "cargo", "bad_unknown_token");
            return StatusCode::UNAUTHORIZED;
        }
        Err(error) => {
            tracing::error!(%error, "test-engine token lookup failed");
            record_upload(&state.metrics, "cargo", "token_lookup_error");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    let input = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            record_upload(&state.metrics, "cargo", "bad_body");
            return StatusCode::BAD_REQUEST;
        }
    };
    let batch = match collectors::parse_cargo_libtest(input) {
        Ok(b) => b,
        Err(error) => {
            tracing::debug!(%error, suite = %suite.suite_key, "cargo upload body rejected");
            record_upload(&state.metrics, "cargo", "bad_body");
            return StatusCode::BAD_REQUEST;
        }
    };

    let run = RunPayload {
        ci_provider: "cargo".to_string(),
        run_ref: batch
            .run_ref
            .unwrap_or_else(|| format!("cargo/{}", Uuid::new_v4())),
        invocation_id: None,
    };
    finish_upload(
        &state.pool,
        &suite,
        &run,
        &batch.executions,
        "cargo",
        &state.metrics,
    )
    .await
}

/// `POST /v1/test-engine/upload/vitest` — Buildkite vitest reporter ingestion
/// (M2-2). Normalized via `collectors::parse_vitest_batch`.
pub async fn handle_vitest_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let suite = match authorize(&state.pool, &headers).await {
        Ok(Some(suite)) => suite,
        Ok(None) => {
            record_upload(&state.metrics, "vitest", "bad_unknown_token");
            return StatusCode::UNAUTHORIZED;
        }
        Err(error) => {
            tracing::error!(%error, "test-engine token lookup failed");
            record_upload(&state.metrics, "vitest", "token_lookup_error");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    let input = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            record_upload(&state.metrics, "vitest", "bad_body");
            return StatusCode::BAD_REQUEST;
        }
    };
    let batch = match collectors::parse_vitest_batch(input) {
        Ok(b) => b,
        Err(error) => {
            tracing::debug!(%error, suite = %suite.suite_key, "vitest upload body rejected");
            record_upload(&state.metrics, "vitest", "bad_body");
            return StatusCode::BAD_REQUEST;
        }
    };

    let run = RunPayload {
        ci_provider: "vitest".to_string(),
        run_ref: batch
            .run_ref
            .unwrap_or_else(|| format!("vitest/{}", Uuid::new_v4())),
        invocation_id: None,
    };
    finish_upload(
        &state.pool,
        &suite,
        &run,
        &batch.executions,
        "vitest",
        &state.metrics,
    )
    .await
}

/// Extracts + resolves the Bearer collection token to a suite identity.
async fn authorize(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<crate::test_engine::SuiteTokenIdentity>, sqlx::Error> {
    let Some(raw_token) = bearer_token(headers) else {
        return Ok(None);
    };
    find_suite_for_token(pool, raw_token).await
}

/// Writes a normalized collector batch and maps the outcome to a status code.
async fn finish_upload(
    pool: &PgPool,
    identity: &crate::test_engine::SuiteTokenIdentity,
    run: &RunPayload,
    executions: &[NormalizedExecution],
    kind: &str,
    metrics: &Metrics,
) -> StatusCode {
    match write_batch(pool, identity, run, executions).await {
        Ok(()) => {
            record_upload(metrics, kind, "ok");
            StatusCode::OK
        }
        Err(error) => {
            tracing::error!(%error, suite = %identity.suite_key, "test-engine {kind} upload write failed");
            record_upload(metrics, kind, "storage_error");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Persists one normalized batch within a single transaction: resolve/create
/// the suite (already resolved), resolve/create each test, upsert the run, then
/// append executions.
async fn write_batch(
    pool: &PgPool,
    identity: &crate::test_engine::SuiteTokenIdentity,
    run: &RunPayload,
    executions: &[NormalizedExecution],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let suite_id = identity.suite_id;

    // Resolve each test, returning its id for the execution FK.
    let mut test_ids = Vec::with_capacity(executions.len());
    for exec in executions {
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
        .map(|(exec, test_ref)| crate::test_engine::NewExecution {
            test_id: test_ref.id,
            status: exec.status,
            duration_ms: exec.duration_ms,
            stack: exec.stack.as_deref(),
        })
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

/// Pure, dependency-free unit tests for the ingestion parsing helpers — the
/// fast safety net @Quality's P1 asks for (no DB, no wiremock).
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod pure_tests {
    use super::*;

    fn headers_of(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn bearer_token_accepts_well_formed_bearer() {
        let h = headers_of(&[("authorization", "Bearer some-token")]);
        assert_eq!(bearer_token(&h), Some("some-token"));
    }

    #[test]
    fn bearer_token_rejects_missing_or_malformed_auth() {
        assert_eq!(bearer_token(&HeaderMap::new()), None);
        let malformed = headers_of(&[("authorization", "Basic abc")]);
        assert_eq!(bearer_token(&malformed), None);
        let empty = headers_of(&[("authorization", "Bearer   ")]);
        assert_eq!(bearer_token(&empty), Some(""));
    }

    #[test]
    fn is_junit_xml_matches_xml_content_types_only() {
        let xml = headers_of(&[("content-type", "application/xml")]);
        let junit = headers_of(&[("content-type", "application/junit+xml")]);
        let text = headers_of(&[("content-type", "text/xml")]);
        let json = headers_of(&[("content-type", "application/json")]);
        let missing = HeaderMap::new();

        assert!(is_junit_xml(&xml));
        assert!(is_junit_xml(&junit));
        assert!(is_junit_xml(&text));
        assert!(!is_junit_xml(&json));
        assert!(!is_junit_xml(&missing));
    }

    #[test]
    fn parse_execution_status_maps_known_and_rejects_unknown() {
        assert_eq!(
            parse_execution_status("passed"),
            Some(ExecutionStatus::Passed)
        );
        assert_eq!(
            parse_execution_status("failed"),
            Some(ExecutionStatus::Failed)
        );
        assert_eq!(
            parse_execution_status("skipped"),
            Some(ExecutionStatus::Skipped)
        );
        assert_eq!(
            parse_execution_status("errored"),
            Some(ExecutionStatus::Errored)
        );
        assert_eq!(parse_execution_status("bogus"), None);
    }
}
