//! JUnit XML → normalized execution parser (`docs/test-engine.md`). Accepts
//! the widely-produced JUnit XML report format
//! (`<testsuite>`/`<testcase>` with `<failure>`/`<error>`/`<skipped>` children,
//! `time` attributes) and normalizes it into the ingestion batch shape already
//! understood by the test-engine data model (§3) — the same schema as the
//! raw-JSON upload path. Pure + dependency-light (quick-xml), fully unit-testable.
//!
//! Supports the permissiveness the format actually has in the wild: a testcase
//! `name` may be absent (fall back to `classname`), `time` is seconds with an
//! optional leading dot (`.012`), and a passing test has no failure child.

use crate::test_engine::ExecutionStatus;

/// One normalized execution extracted from a JUnit report, ready for the same
/// write path as the JSON upload.
#[derive(Debug, Clone)]
pub struct JunitExecution {
    /// Test name (design §3), ideally fully-qualified `classname#name`.
    pub name: String,
    pub status: ExecutionStatus,
    pub duration_ms: i64,
    pub stack: Option<String>,
}

/// A normalized batch of executions extracted from one JUnit document. The
/// suite key / tenancy come from the caller's collection token, not the XML.
#[derive(Debug, Clone)]
pub struct JunitBatch {
    pub executions: Vec<JunitExecution>,
}

/// Parses a JUnit XML document into a normalized batch of executions.
///
/// `Err` on malformed XML or when no usable `<testcase>` is found. A testcase
/// with neither `name` nor `classname` cannot be keyed and is skipped so the
/// rest of the report is preserved.
pub fn parse(bytes: &[u8]) -> Result<JunitBatch, JunitError> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut batch = JunitBatch {
        executions: Vec::new(),
    };
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let event = reader.read_event_into(&mut buf).map_err(JunitError::Xml)?;
        match event {
            quick_xml::events::Event::Start(e) => {
                if e.name().as_ref() == b"testcase"
                    && let Some(exec) = parse_testcase(&mut reader, &e, false)?
                {
                    batch.executions.push(exec);
                }
            }
            quick_xml::events::Event::Empty(e) => {
                if e.name().as_ref() == b"testcase"
                    && let Some(exec) = parse_testcase(&mut reader, &e, true)?
                {
                    batch.executions.push(exec);
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }

    if batch.executions.is_empty() {
        return Err(JunitError::NoTestcases);
    }
    Ok(batch)
}

/// Parses a single `<testcase>` element plus its children into one execution.
/// `self_closing` distinguishes `<testcase ... />` (no children) from
/// `<testcase ...>...</testcase>` so an unkeyed self-closing testcase isn't
/// drained past the next sibling.
fn parse_testcase<R: std::io::BufRead>(
    reader: &mut quick_xml::Reader<R>,
    start: &quick_xml::events::BytesStart<'_>,
    self_closing: bool,
) -> Result<Option<JunitExecution>, JunitError> {
    let key = attr(start, "name").or_else(|| attr(start, "classname"));
    // A testcase with neither name nor classname can't be keyed; skip it.
    let key = match key {
        Some(k) if !k.is_empty() => k,
        _ => {
            if !self_closing {
                drain_children(reader)?;
            }
            return Ok(None);
        }
    };

    // A self-closing `<testcase .../>` has no children: any status marker is a
    // sibling element, so it must not be drained. It's a clean pass captured
    // here before the child loop reads any following event.
    if self_closing {
        let duration_ms = (parse_time(start) * 1000.0).round() as i64;
        return Ok(Some(JunitExecution {
            name: key,
            status: ExecutionStatus::Passed,
            duration_ms,
            stack: None,
        }));
    }

    let duration_ms = (parse_time(start) * 1000.0).round() as i64;

    // Children `<failure>`, `<error>`, `<skipped>` determine the status; a
    // passing testcase has none. Child text (the exception trace) becomes the
    // preserved stack for posteriority.
    let mut status = ExecutionStatus::Passed;
    let mut stack: Option<String> = None;
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let event = reader.read_event_into(&mut buf).map_err(JunitError::Xml)?;
        match event {
            quick_xml::events::Event::Start(child) => {
                let tag = child.name().as_ref().to_vec();
                let text = read_text(reader).unwrap_or_default();
                match tag.as_slice() {
                    b"failure" => status = ExecutionStatus::Failed,
                    b"error" => status = ExecutionStatus::Errored,
                    b"skipped" => status = ExecutionStatus::Skipped,
                    _ => {} // ignore unknown children
                }
                if !text.is_empty() {
                    // Preserve the trace from failure/error/skipped bodies.
                    stack = Some(text);
                }
            }
            quick_xml::events::Event::Empty(child) => match child.name().as_ref() {
                b"failure" => status = ExecutionStatus::Failed,
                b"error" => status = ExecutionStatus::Errored,
                b"skipped" => status = ExecutionStatus::Skipped,
                _ => {}
            },
            quick_xml::events::Event::End(_) | quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }

    Ok(Some(JunitExecution {
        name: key,
        status,
        duration_ms,
        stack,
    }))
}

fn attr(start: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    start
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == key.as_bytes())
        .and_then(|a| String::from_utf8(a.value.as_ref().to_vec()).ok())
}

/// The `time` attribute in seconds, tolerant of a leading dot (`.012`).
fn parse_time(start: &quick_xml::events::BytesStart<'_>) -> f64 {
    attr(start, "time")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.0)
}

/// Reads the text content of the just-opened element (e.g. the `<failure>`
/// message body), consuming up to and including its closing tag.
fn read_text<R: std::io::BufRead>(reader: &mut quick_xml::Reader<R>) -> Result<String, JunitError> {
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let event = reader.read_event_into(&mut buf).map_err(JunitError::Xml)?;
        match event {
            quick_xml::events::Event::Text(t) => {
                out.push_str(&t.unescape().map_err(JunitError::Xml)?)
            }
            quick_xml::events::Event::CData(t) => {
                out.push_str(&String::from_utf8_lossy(t.as_ref()))
            }
            quick_xml::events::Event::End(_) | quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(out.trim().to_string())
}

/// Skips the subtree of a skipped testcase so the outer parser continues past
/// it cleanly.
fn drain_children<R: std::io::BufRead>(
    reader: &mut quick_xml::Reader<R>,
) -> Result<(), JunitError> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(JunitError::Xml)? {
            quick_xml::events::Event::End(_) | quick_xml::events::Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JunitError {
    #[error("malformed JUnit XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("no testcases found in JUnit report")]
    NoTestcases,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_passing_testcase() {
        let xml = br#"<?xml version="1.0"?>
            <testsuite name="widgets">
              <testcase classname="tests.foo" name="it_works" time="0.12"/>
            </testsuite>"#;
        let batch = parse(xml).unwrap();
        assert_eq!(batch.executions.len(), 1);
        let e = &batch.executions[0];
        assert_eq!(e.name, "it_works");
        assert_eq!(e.status, ExecutionStatus::Passed);
        assert_eq!(e.duration_ms, 120);
        assert!(e.stack.is_none());
    }

    #[test]
    fn parses_a_failed_testcase_with_stack() {
        let xml = br#"<?xml version="1.0"?>
            <testsuite name="s">
              <testcase classname="tests.foo" name="it_breaks" time="1.5">
                <failure message="boom">assertion failed: left == right
            at tests/foo.rs:4</failure>
              </testcase>
            </testsuite>"#;
        let batch = parse(xml).unwrap();
        let e = &batch.executions[0];
        assert_eq!(e.status, ExecutionStatus::Failed);
        assert_eq!(e.duration_ms, 1500);
        assert!(e.stack.as_deref().unwrap().contains("tests/foo.rs:4"));
    }

    #[test]
    fn error_and_skipped_testcases_map_to_their_statuses() {
        let xml = br#"<?xml version="1.0"?>
            <testsuite name="s">
              <testcase classname="b" name="e" time="0.1"><error message="x"/></testcase>
              <testcase classname="b" name="k" time="0.2"><skipped/></testcase>
            </testsuite>"#;
        let batch = parse(xml).unwrap();
        assert_eq!(batch.executions[0].status, ExecutionStatus::Errored);
        assert_eq!(batch.executions[1].status, ExecutionStatus::Skipped);
    }

    #[test]
    fn name_falls_back_to_classname() {
        let xml =
            br#"<testsuite name="s"><testcase classname="tests.bar" time="0.1"/></testsuite>"#;
        let batch = parse(xml).unwrap();
        assert_eq!(batch.executions[0].name, "tests.bar");
    }

    #[test]
    fn a_testcase_with_no_name_or_classname_is_skipped_not_fatal() {
        let xml = br#"
            <testsuite name="s">
              <testcase time="0.1"/>
              <testcase classname="tests.ok" name="fine" time="0.2"/>
            </testsuite>"#;
        let batch = parse(xml).unwrap();
        assert_eq!(batch.executions.len(), 1);
        assert_eq!(batch.executions[0].name, "fine");
    }

    #[test]
    fn empty_report_is_an_error() {
        assert!(matches!(
            parse(br#"<testsuite name="empty"/>"#),
            Err(JunitError::NoTestcases)
        ));
    }

    #[test]
    fn a_leading_dot_time_parses() {
        let xml =
            br#"<testsuite name="s"><testcase classname="c" name="n" time=".012"/></testsuite>"#;
        let batch = parse(xml).unwrap();
        assert_eq!(batch.executions[0].duration_ms, 12);
    }

    /// Realistic `vitest --reporter=junit` output: a `<testsuites>` root
    /// wrapping nested `<testsuite>` elements, each with `<testcase>` children.
    /// Our parser is root-agnostic (it finds every `<testcase>` in document
    /// order), so this nests cleanly and every testcase maps to the correct
    /// status.
    #[test]
    fn vitest_junit_report_parses_cleanly() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" ?>
            <testsuites name="test" tests="3" failures="1" errors="0" skipped="1" time="1.23">
              <testsuite name="src/foo.test.ts" tests="3" errors="0" failures="1" skipped="1" time="1.20">
                <testcase classname="src/foo.test.ts" name="adds numbers" time="0.051">
                  <failure message="AssertionError: expected 2 to be 3">
    at src/foo.test.ts:7:22
                  </failure>
                </testcase>
                <testcase classname="src/foo.test.ts" name="subtracts" time="0.030"/>
                <testcase classname="src/foo.test.ts" name="skipped one" time="0.0"><skipped/></testcase>
              </testsuite>
            </testsuites>"#;
        let batch = parse(xml).unwrap();
        assert_eq!(batch.executions.len(), 3);

        // Order preserved (document order), statuses mapped per child.
        assert_eq!(batch.executions[0].name, "adds numbers");
        assert_eq!(batch.executions[0].status, ExecutionStatus::Failed);
        assert_eq!(batch.executions[0].duration_ms, 51);
        assert!(
            batch.executions[0]
                .stack
                .as_deref()
                .unwrap()
                .contains("src/foo.test.ts:7:22")
        );

        assert_eq!(batch.executions[1].name, "subtracts");
        assert_eq!(batch.executions[1].status, ExecutionStatus::Passed);

        assert_eq!(batch.executions[2].name, "skipped one");
        assert_eq!(batch.executions[2].status, ExecutionStatus::Skipped);
    }
}
