//! Test-engine flaky detection — the *pure* part (docs/design/1.0-test-engine.md
//! §5, @Quality P3 / R9). Lives in `slash-core` because the decision —
//! "does this execution sequence constitute a flaky test?" — is pure
//! computation over a status sequence, with zero network or database IO.
//! The server layer collects the executions and drives the reconcile; it
//! only ever consults this module for the flaky ↔ not-flaky decision.
//!
//! M1 criterion (design §5, tightened per review): a test is flaky iff, for
//! the same input, with a denominator of >= `FLAKY_MIN_EXECUTIONS` executions
//! in the window, there is an observed fail-then-pass recovery.
//!
//! `ObservedStatus` is a purpose-built, dependency-light status view (kept
//! distinct from any storage-layer enum so core stays IO-free). Collectors map
//! their richer status to this at the edge.

/// A test execution's outcome, as observed by the flaky detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedStatus {
    Passed,
    Failed,
    Skipped,
    Errored,
}

/// Minimum executions in the window before a test can be judged flaky (design
/// §8 Q4: a denominator against small-sample false positives).
pub const FLAKY_MIN_EXECUTIONS: usize = 3;

/// Returns true when the execution sequence qualifies as flaky: at least
/// `FLAKY_MIN_EXECUTIONS` executions and at least one fail-then-pass recovery
/// (a `Failed`/`Errored` followed by a later `Passed`).
pub fn is_flaky(executions: &[ObservedStatus]) -> bool {
    executions.len() >= FLAKY_MIN_EXECUTIONS && has_fail_then_pass(executions)
}

/// True when a failure (`Failed`/`Errored`) is followed by a later `Passed` in
/// window order — the recovery signal.
pub fn has_fail_then_pass(executions: &[ObservedStatus]) -> bool {
    let mut saw_failure = false;
    for status in executions {
        match status {
            ObservedStatus::Failed | ObservedStatus::Errored => saw_failure = true,
            ObservedStatus::Passed if saw_failure => return true,
            ObservedStatus::Passed | ObservedStatus::Skipped => {}
        }
    }
    false
}

/// True when the window still contains a failure — used to gate un-quarantine
/// (a muted test only recovers once the window has rolled past failures and it
/// is stable).
pub fn recent_contains_failure(executions: &[ObservedStatus]) -> bool {
    executions
        .iter()
        .any(|s| matches!(s, ObservedStatus::Failed | ObservedStatus::Errored))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::ObservedStatus::*;
    use super::*;

    #[test]
    fn below_min_executions_is_not_flaky() {
        assert!(!is_flaky(&[Failed, Passed]));
        assert!(!is_flaky(&[Passed]));
        assert!(!is_flaky(&[]));
    }

    #[test]
    fn fail_then_pass_within_window_is_flaky() {
        assert!(is_flaky(&[Passed, Failed, Passed]));
        assert!(is_flaky(&[Failed, Errored, Passed]));
    }

    #[test]
    fn all_pass_is_not_flaky() {
        assert!(!is_flaky(&[Passed, Passed, Passed, Passed]));
    }

    #[test]
    fn failure_with_no_recovery_is_not_flaky() {
        assert!(!is_flaky(&[Passed, Failed, Failed]));
        assert!(!is_flaky(&[Failed, Failed, Failed]));
    }

    #[test]
    fn skipped_do_not_break_recovery_detection() {
        assert!(is_flaky(&[Passed, Failed, Skipped, Passed]));
    }

    #[test]
    fn recent_contains_failure_detects_presence() {
        assert!(!recent_contains_failure(&[Passed, Passed]));
        assert!(recent_contains_failure(&[Passed, Failed]));
        assert!(recent_contains_failure(&[Errored, Passed]));
    }
}
