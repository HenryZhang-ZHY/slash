//! Flaky test detector — the level-triggered reconcile (docs/design/
//! 1.0-test-engine.md §5). Runs over the durable execution record, never from
//! events directly, and transitions `tests.state` with a guarded CAS so manual
//! state edits and concurrent reconciles can't race destructively.
//!
//! M1 criterion (design §5, tightened per review): a test is flaky iff, for the
//! same input, over a rolling window with a denominator of >= `min_executions`
//! executions, there is an observed fail-then-pass recovery. Un-quarantine
//! fires when the window rolls past the failures and the test is stable.

use std::time::Duration;

use sqlx::PgPool;

use crate::test_engine::{
    ExecutionStatus, TestState, all_tests, recent_executions, set_test_state,
};

/// Rolling window for flaky detection (design §5: 7 days).
pub const FLAKY_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Minimum executions in the window before a test can be judged flaky (design
/// §8 Q4: a denominator against small-sample false positives).
pub const FLAKY_MIN_EXECUTIONS: usize = 3;

/// Runs one detection pass over all tests. Idempotent and safe to call
/// repeatedly (level-triggered reconcile). Returns the number of transitions
/// applied (enabled->muted and muted->enabled), for observability.
pub async fn reconcile(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let tests = all_tests(pool).await?;
    let mut transitions = 0usize;

    for (test_id, state) in tests {
        let recent = recent_executions(pool, test_id, FLAKY_WINDOW.as_secs() as i64).await?;

        match state {
            TestState::Enabled => {
                if is_flaky(&recent) {
                    // enabled -> muted (default disposition per §8 Q1).
                    if set_test_state(pool, test_id, &[TestState::Enabled], TestState::Muted)
                        .await?
                    {
                        tracing::info!(test_id = %test_id, "flaky test quarantined (muted)");
                        transitions += 1;
                    }
                }
            }
            TestState::Muted => {
                if !recent_contains_failure(&recent) {
                    // muted -> enabled when the window has rolled past failures
                    // and the test is stable (design §5 un-quarantine).
                    if set_test_state(pool, test_id, &[TestState::Muted], TestState::Enabled)
                        .await?
                    {
                        tracing::info!(test_id = %test_id, "muted test recovered, enabled");
                        transitions += 1;
                    }
                }
            }
            TestState::Skipped => {
                // `skipped` is never auto-un-quarantined (design §8 Q1); it is
                // a manual / hard-cost decision.
            }
        }
    }

    Ok(transitions)
}

/// Returns true when the execution sequence within the window qualifies as
/// flaky: at least `FLAKY_MIN_EXECUTIONS` executions and at least one
/// fail-then-pass recovery (a `failed`/`errored` followed by a later `passed`).
fn is_flaky(executions: &[ExecutionStatus]) -> bool {
    if executions.len() < FLAKY_MIN_EXECUTIONS {
        return false;
    }
    has_fail_then_pass(executions)
}

fn has_fail_then_pass(executions: &[ExecutionStatus]) -> bool {
    let mut saw_failure = false;
    for status in executions {
        match status {
            ExecutionStatus::Failed | ExecutionStatus::Errored => saw_failure = true,
            ExecutionStatus::Passed if saw_failure => return true,
            ExecutionStatus::Passed | ExecutionStatus::Skipped => {}
        }
    }
    false
}

fn recent_contains_failure(executions: &[ExecutionStatus]) -> bool {
    executions
        .iter()
        .any(|s| matches!(s, ExecutionStatus::Failed | ExecutionStatus::Errored))
}

// --- pure correctness tests, no database needed ---
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_engine::ExecutionStatus::*;

    fn seq(statuses: &[ExecutionStatus]) -> Vec<ExecutionStatus> {
        statuses.to_vec()
    }

    #[test]
    fn below_min_executions_is_not_flaky() {
        // 2 executions, one fail then pass — but below the denominator.
        assert!(!is_flaky(&seq(&[Failed, Passed])));
        assert!(!is_flaky(&seq(&[Passed])));
        assert!(!is_flaky(&seq(&[])));
    }

    #[test]
    fn fail_then_pass_within_window_is_flaky() {
        assert!(is_flaky(&seq(&[Passed, Failed, Passed])));
        assert!(is_flaky(&seq(&[Failed, Errored, Passed])));
    }

    #[test]
    fn all_pass_is_not_flaky() {
        assert!(!is_flaky(&seq(&[Passed, Passed, Passed, Passed])));
    }

    #[test]
    fn failure_with_no_recovery_is_not_flaky() {
        // A deployed-broken intermittent failure that never recovered is not
        // flaky — it's a genuine failure (design §8 Q4 rationale).
        assert!(!is_flaky(&seq(&[Passed, Failed, Failed])));
        assert!(!is_flaky(&seq(&[Failed, Failed, Failed])));
    }

    #[test]
    fn skipped_do_not_break_recovery_detection() {
        assert!(is_flaky(&seq(&[Passed, Failed, Skipped, Passed])));
    }

    #[test]
    fn muted_recovery_requires_no_failure_in_window() {
        assert!(!recent_contains_failure(&seq(&[Passed, Passed])));
        assert!(recent_contains_failure(&seq(&[Passed, Failed])));
    }

    #[test]
    fn min_executions_constant_is_stable() {
        assert_eq!(FLAKY_MIN_EXECUTIONS, 3);
    }
}
