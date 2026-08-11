//! Buildkite-collector JSON → normalized execution mapping (docs/design/
//! 1.0-test-engine.md §6 M2, task M2-2). Per the "极可能抄 Buildkite"
//! strategy, the open-source Buildkite collectors (rust / vitest) are reused as
//! the client side; slash only implements the *server-side receiving logic*:
//! parse their on-the-wire JSON dialect and normalize it into the same
//! `NormalizedExecution` shape the raw-JSON ingestion path already writes.
//!
//! Two dialects are normalized here:
//!   - **cargo-libtest** (`cargo test -- -Z unstable-options --format json`):
//!     a line-delimited stream of test/suite events consumed by
//!     `buildkite-test-collector`. Each test event carries `name`, `status`,
//!     and `exec_time` (seconds).
//!   - **Vitest** (`@buildkite/test-collector-javascript` vitest reporter):
//!     a batch of per-test objects with `name`, `status`, `duration`.

use crate::test_engine::ExecutionStatus;

/// A source-agnostic normalized execution, matching the write path's shape.
#[derive(Debug, Clone)]
pub struct NormalizedExecution {
    pub name: String,
    pub status: ExecutionStatus,
    pub duration_ms: i64,
    pub stack: Option<String>,
    pub file: Option<String>,
    pub line_no: Option<i32>,
}

/// A batch of executions from one collector upload, plus an optional run
/// identity the collector supplied.
#[derive(Debug, Clone, Default)]
pub struct CollectorBatch {
    pub run_ref: Option<String>,
    pub executions: Vec<NormalizedExecution>,
}

#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no test results found in collector payload")]
    Empty,
}

/// Parses the line-delimited `cargo test --format json` (libtest) event stream.
///
/// Skips non-test events (`suite`, `summary`, `toolchain`); maps each `test`
/// event's `status` to an execution status (`ignored` → `Skipped`; `failed` →
/// `Failed`; everything else → `Passed`). `exec_time` is seconds → ms.
pub fn parse_cargo_libtest(input: &str) -> Result<CollectorBatch, CollectorError> {
    let mut batch = CollectorBatch::default();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("type").and_then(|t| t.as_str()) != Some("test") {
            continue;
        }
        let name = match value.get("name").and_then(|n| n.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let status = match value.get("status").and_then(|s| s.as_str()) {
            Some("failed") => ExecutionStatus::Failed,
            Some("ignored") => ExecutionStatus::Skipped,
            _ => ExecutionStatus::Passed,
        };
        let exec_time: f64 = value
            .get("exec_time")
            .and_then(|t| t.as_f64())
            .unwrap_or(0.0);
        let duration_ms = (exec_time * 1000.0).round() as i64;
        batch.executions.push(NormalizedExecution {
            name: name.to_string(),
            status,
            duration_ms,
            stack: None,
            file: None,
            line_no: None,
        });
    }
    if batch.executions.is_empty() {
        return Err(CollectorError::Empty);
    }
    Ok(batch)
}

/// Parses the Vitest / buildkite-report result batch (an array of per-test
/// objects). Each object may have `name`, `status` (`passed|failed|skipped`),
/// `duration` (ms) / `duration_ms`, optional `location` (`{file, line}`).
///
/// Unknown fields are untouched (tolerance per the ci_env/run_env note) — the
/// parser reads only the keys it needs.
pub fn parse_vitest_batch(input: &str) -> Result<CollectorBatch, CollectorError> {
    let value: serde_json::Value = serde_json::from_str(input)?;
    let array: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Object(mut m) => match m.remove("tests") {
            Some(serde_json::Value::Array(a)) => a,
            _ => return Err(CollectorError::Empty),
        },
        _ => return Err(CollectorError::Empty),
    };

    let mut batch = CollectorBatch::default();
    for item in array {
        let name = item.get("name").and_then(|n| n.as_str());
        let Some(name) = name.map(str::to_owned).filter(|n| !n.is_empty()) else {
            continue;
        };
        let status = match item.get("status").and_then(|s| s.as_str()) {
            Some("failed") => ExecutionStatus::Failed,
            Some("skipped") => ExecutionStatus::Skipped,
            Some("errored") => ExecutionStatus::Errored,
            _ => ExecutionStatus::Passed,
        };
        let duration_ms = item
            .get("duration")
            .and_then(|d| d.as_f64())
            .or_else(|| item.get("duration_ms").and_then(|d| d.as_f64()))
            .map(|d| d.round() as i64)
            .unwrap_or(0);

        // Optional location { file, line } (vitest includeTaskLocation).
        let file = item
            .get("location")
            .and_then(|loc: &serde_json::Value| loc.get("file"))
            .and_then(|f: &serde_json::Value| f.as_str())
            .map(str::to_owned);
        let line_no = item
            .get("location")
            .and_then(|loc: &serde_json::Value| loc.get("line"))
            .and_then(|l: &serde_json::Value| l.as_i64())
            .map(|l| l as i32);

        batch.executions.push(NormalizedExecution {
            name,
            status,
            duration_ms,
            stack: None,
            file,
            line_no,
        });
    }
    if batch.executions.is_empty() {
        return Err(CollectorError::Empty);
    }
    Ok(batch)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_cargo_libtest_passing_and_failing_events() {
        let input = "\
{\"type\":\"suite\",\"event\":\"started\",\"test_count\":2}
{\"type\":\"test\",\"name\":\"tests::it_works\",\"test_type\":\"Test\",\"status\":\"passed\",\"exec_time\":0.012,\"event\":\"ok\"}
{\"type\":\"test\",\"name\":\"tests::it_breaks\",\"test_type\":\"Test\",\"status\":\"failed\",\"exec_time\":0.5,\"event\":\"failed\"}
{\"type\":\"suite\",\"event\":\"ok\",\"passed\":1,\"failed\":1,\"ignored\":0}
{\"type\":\"summary\",\"passed\":1,\"failed\":1,\"ignored\":0}
";
        let batch = parse_cargo_libtest(input).unwrap();
        assert_eq!(batch.executions.len(), 2);
        assert_eq!(batch.executions[0].name, "tests::it_works");
        assert_eq!(batch.executions[0].status, ExecutionStatus::Passed);
        assert_eq!(batch.executions[0].duration_ms, 12);
        assert_eq!(batch.executions[1].name, "tests::it_breaks");
        assert_eq!(batch.executions[1].status, ExecutionStatus::Failed);
        assert_eq!(batch.executions[1].duration_ms, 500);
    }

    #[test]
    fn cargo_ignored_maps_to_skipped() {
        let input = "\
{\"type\":\"test\",\"name\":\"a::b\",\"status\":\"ignored\",\"event\":\"ignored\"}
";
        let batch = parse_cargo_libtest(input).unwrap();
        assert_eq!(batch.executions[0].status, ExecutionStatus::Skipped);
    }

    #[test]
    fn cargo_non_test_events_are_skipped() {
        let input = "\
{\"type\":\"suite\",\"event\":\"started\",\"test_count\":1}
{\"type\":\"toolchain\",\"version\":\"1.94.1\"}
";
        assert!(matches!(
            parse_cargo_libtest(input),
            Err(CollectorError::Empty)
        ));
    }

    #[test]
    fn parses_a_vitest_batch_array() {
        let input = r#"[
          {"name":"src/foo.test.ts > adds","status":"passed","duration":51,"location":{"file":"src/foo.test.ts","line":7}},
          {"name":"src/foo.test.ts > breaks","status":"failed","duration":30,"location":{"file":"src/foo.test.ts","line":12}},
          {"name":"src/foo.test.ts > skips","status":"skipped","duration":0}
        ]"#;
        let batch = parse_vitest_batch(input).unwrap();
        assert_eq!(batch.executions.len(), 3);
        assert_eq!(batch.executions[0].status, ExecutionStatus::Passed);
        assert_eq!(batch.executions[0].duration_ms, 51);
        assert_eq!(batch.executions[0].file.as_deref(), Some("src/foo.test.ts"));
        assert_eq!(batch.executions[0].line_no, Some(7));
        assert_eq!(batch.executions[1].status, ExecutionStatus::Failed);
        assert_eq!(batch.executions[2].status, ExecutionStatus::Skipped);
    }

    #[test]
    fn vitest_unknown_extra_fields_are_tolerated() {
        // ci_env / run_env style extra top-level fields must not break parsing.
        let input = r#"{
          "run_env": {"CI":"true","branch":"main","commit":"abc"},
          "tests": [
            {"name":"t","status":"passed","duration":1,"ci_env":{"build":"1"}}
          ]
        }"#;
        let batch = parse_vitest_batch(input).unwrap();
        assert_eq!(batch.executions.len(), 1);
        assert_eq!(batch.executions[0].status, ExecutionStatus::Passed);
    }

    #[test]
    fn empty_collector_payload_is_an_error() {
        assert!(matches!(
            parse_vitest_batch("[]"),
            Err(CollectorError::Empty)
        ));
        assert!(matches!(
            parse_cargo_libtest(""),
            Err(CollectorError::Empty)
        ));
    }
}
