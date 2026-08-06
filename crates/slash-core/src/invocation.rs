//! The invocation lifecycle (spec §7.2): `claimed` -> `dispatched` ->
//! `correlated` -> `completed`, plus the terminal `aborted`,
//! `dispatch_failed`, `correlation_timeout`, `superseded`. This module is
//! the single source of truth for which transitions are valid — reused both
//! by `slash-server`'s guarded `UPDATE ... WHERE status IN (...)` statements
//! and by tests, so the two can never drift apart.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvocationStatus {
    Claimed,
    Dispatched,
    Correlated,
    Completed,
    Aborted,
    DispatchFailed,
    CorrelationTimeout,
    Superseded,
}

impl InvocationStatus {
    pub const ALL: [InvocationStatus; 8] = [
        Self::Claimed,
        Self::Dispatched,
        Self::Correlated,
        Self::Completed,
        Self::Aborted,
        Self::DispatchFailed,
        Self::CorrelationTimeout,
        Self::Superseded,
    ];

    /// Every status that may legally transition to `to` — the derived
    /// inverse of [`can_transition_to`](Self::can_transition_to), so a
    /// caller building a guarded `WHERE status IN (...)` can never supply an
    /// inconsistent set: there's only one way to ask this question.
    pub fn valid_predecessors(to: Self) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|from| from.can_transition_to(to))
            .collect()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Dispatched => "dispatched",
            Self::Correlated => "correlated",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::DispatchFailed => "dispatch_failed",
            Self::CorrelationTimeout => "correlation_timeout",
            Self::Superseded => "superseded",
        }
    }

    /// The inverse of [`as_str`](Self::as_str), for interpreting the
    /// `status` column read back from storage.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claimed" => Some(Self::Claimed),
            "dispatched" => Some(Self::Dispatched),
            "correlated" => Some(Self::Correlated),
            "completed" => Some(Self::Completed),
            "aborted" => Some(Self::Aborted),
            "dispatch_failed" => Some(Self::DispatchFailed),
            "correlation_timeout" => Some(Self::CorrelationTimeout),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }

    /// Terminal states absorb everything (spec §7.2): once here, no further
    /// transition is ever valid.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Aborted
                | Self::DispatchFailed
                | Self::CorrelationTimeout
                | Self::Superseded
        )
    }

    /// Whether `self -> next` is a valid guarded transition. A zero-row
    /// `UPDATE ... WHERE status IN (...)` reflecting this is what makes
    /// out-of-order and duplicate event delivery harmless (spec §7.2).
    pub fn can_transition_to(self, next: Self) -> bool {
        if self.is_terminal() {
            return false;
        }
        match (self, next) {
            // `aborted` is pre-dispatch only (spec §7.2): the head moved
            // before the POST, or a stranded `claimed` row was swept. Once
            // a row is `dispatched`, the POST may already have reached
            // GitHub, so it can no longer simply be aborted.
            (Self::Claimed, Self::Dispatched | Self::Aborted) => true,
            (
                Self::Dispatched,
                Self::Correlated | Self::DispatchFailed | Self::CorrelationTimeout,
            ) => true,
            (Self::Correlated, Self::Completed | Self::CorrelationTimeout) => true,
            // Any non-terminal invocation can be superseded by a newer one
            // of the same (repo, pr, command), independent of which
            // pre-dispatch/post-dispatch state it's currently in (spec §6.7).
            (_, Self::Superseded) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use InvocationStatus::*;

    #[test]
    fn parse_is_the_exact_inverse_of_as_str_for_every_status() {
        for status in InvocationStatus::ALL {
            assert_eq!(InvocationStatus::parse(status.as_str()), Some(status));
        }
    }

    #[test]
    fn parse_rejects_unknown_strings() {
        assert_eq!(InvocationStatus::parse("not-a-status"), None);
    }

    #[test]
    fn happy_path_transitions_are_valid_in_order() {
        assert!(Claimed.can_transition_to(Dispatched));
        assert!(Dispatched.can_transition_to(Correlated));
        assert!(Correlated.can_transition_to(Completed));
    }

    #[test]
    fn terminal_states_accept_no_further_transition() {
        for terminal in [
            Completed,
            Aborted,
            DispatchFailed,
            CorrelationTimeout,
            Superseded,
        ] {
            for target in [
                Claimed,
                Dispatched,
                Correlated,
                Completed,
                Aborted,
                DispatchFailed,
                CorrelationTimeout,
                Superseded,
            ] {
                assert!(
                    !terminal.can_transition_to(target),
                    "{terminal:?} must not transition to {target:?}"
                );
            }
        }
    }

    #[test]
    fn cannot_skip_stages() {
        assert!(!Claimed.can_transition_to(Correlated));
        assert!(!Claimed.can_transition_to(Completed));
        assert!(!Dispatched.can_transition_to(Completed));
    }

    #[test]
    fn any_non_terminal_state_can_be_superseded() {
        for state in [Claimed, Dispatched, Correlated] {
            assert!(state.can_transition_to(Superseded));
        }
    }

    #[test]
    fn aborted_is_reachable_only_from_claimed() {
        assert!(Claimed.can_transition_to(Aborted));
        assert!(!Dispatched.can_transition_to(Aborted));
        assert!(!Correlated.can_transition_to(Aborted));
    }

    #[test]
    fn dispatched_can_resolve_via_either_sweeper_fallback() {
        assert!(Dispatched.can_transition_to(DispatchFailed));
        assert!(Dispatched.can_transition_to(CorrelationTimeout));
    }

    #[test]
    fn valid_predecessors_is_the_exact_inverse_of_can_transition_to() {
        for to in InvocationStatus::ALL {
            let predecessors = InvocationStatus::valid_predecessors(to);
            for from in InvocationStatus::ALL {
                assert_eq!(
                    predecessors.contains(&from),
                    from.can_transition_to(to),
                    "mismatch for {from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn valid_predecessors_of_correlated_is_only_dispatched() {
        assert_eq!(
            InvocationStatus::valid_predecessors(Correlated),
            vec![Dispatched]
        );
    }

    #[test]
    fn cannot_transition_backwards() {
        assert!(!Dispatched.can_transition_to(Claimed));
        assert!(!Correlated.can_transition_to(Dispatched));
    }
}
