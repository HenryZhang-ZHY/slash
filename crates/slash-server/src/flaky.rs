//! Flaky test detector — the level-triggered reconcile (`docs/test-engine.md`).
//! Runs over the durable execution record, never from
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
    ExecutionStatus, RECONCILE_PAGE_SIZE, TestState, all_tests_page, recent_executions,
    set_test_state,
};

/// Rolling window for flaky detection (design §5: 7 days).
pub const FLAKY_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Runs one detection pass over all tests via a bounded cursor sweep
/// (M2-6): pages of `RECONCILE_PAGE_SIZE` keyset-ordered by `id`, so the
/// reconcile never holds the whole `tests` table in memory and remains
/// bounded as the table grows. Idempotent and safe to call repeatedly
/// (level-triggered reconcile). Returns the number of transitions applied
/// (enabled->muted and muted->enabled), for observability.
pub async fn reconcile(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let mut transitions = 0usize;
    let mut after_id = None;

    loop {
        let page = all_tests_page(pool, after_id, RECONCILE_PAGE_SIZE).await?;
        // A short page ends the sweep; a cursor is always known from the last.
        let Some(last) = page.last() else {
            break;
        };

        let last_id = last.0;

        for (test_id, state) in &page {
            let recent = recent_executions(pool, *test_id, FLAKY_WINDOW.as_secs() as i64).await?;

            match state {
                TestState::Enabled => {
                    if is_flaky(&recent) {
                        // enabled -> muted (default disposition per §8 Q1).
                        if set_test_state(pool, *test_id, &[TestState::Enabled], TestState::Muted)
                            .await?
                        {
                            tracing::info!(test_id = %*test_id, "flaky test quarantined (muted)");
                            transitions += 1;
                        }
                    }
                }
                TestState::Muted => {
                    if !recent_contains_failure(&recent) {
                        // muted -> enabled when the window has rolled past failures
                        // and the test is stable (design §5 un-quarantine).
                        if set_test_state(pool, *test_id, &[TestState::Muted], TestState::Enabled)
                            .await?
                        {
                            tracing::info!(test_id = %*test_id, "muted test recovered, enabled");
                            transitions += 1;
                        }
                    }
                }
                TestState::Skipped => {
                    // `skipped` is never auto-un-quarantined (design §8 Q1); it
                    // is a manual / hard-cost decision.
                }
            }
        }

        // Next page continues strictly after this page's last id. Stop when
        // the page was shorter than the page size (we've drained the table).
        after_id = Some(last_id);
        if page.len() < RECONCILE_PAGE_SIZE as usize {
            break;
        }
    }

    Ok(transitions)
}

/// Returns the observed execution status as core's pure status view. This is
/// the only seam between the storage-layer status enum and `slash_core`'s
/// IO-free decision module described in `docs/test-engine.md`.
fn to_observed(executions: &[ExecutionStatus]) -> Vec<slash_core::ObservedStatus> {
    executions
        .iter()
        .map(|s| match s {
            ExecutionStatus::Passed => slash_core::ObservedStatus::Passed,
            ExecutionStatus::Failed => slash_core::ObservedStatus::Failed,
            ExecutionStatus::Skipped => slash_core::ObservedStatus::Skipped,
            ExecutionStatus::Errored => slash_core::ObservedStatus::Errored,
        })
        .collect()
}

/// Delegates the pure flaky decision to `slash_core::test_flaky` (P3/R9).
fn is_flaky(executions: &[ExecutionStatus]) -> bool {
    slash_core::is_flaky(&to_observed(executions))
}

/// Delegates the un-quarantine gate to core.
fn recent_contains_failure(executions: &[ExecutionStatus]) -> bool {
    slash_core::recent_contains_failure(&to_observed(executions))
}

// --- delegation tests: server status -> core decision (no DB needed) ---
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_engine::ExecutionStatus::*;

    #[test]
    fn flaky_decision_is_delegated_to_core() {
        // A denominator-satisfying fail-then-pass is flaky via core.
        assert!(is_flaky(&[Passed, Failed, Passed]));
        // Below the denominator is not.
        assert!(!is_flaky(&[Failed, Passed]));
        // All-pass is not.
        assert!(!is_flaky(&[Passed, Passed, Passed]));
    }

    #[test]
    fn unquarantine_gate_is_delegated_to_core() {
        assert!(!recent_contains_failure(&[Passed, Passed]));
        assert!(recent_contains_failure(&[Passed, Failed]));
    }
}
