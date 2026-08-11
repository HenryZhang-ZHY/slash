//! Pipeline stage decomposition (spec §5 guard order, redesign R2).
//!
//! Each stage matches one guard in spec §5's ordering:
//! `SyntacticGuard → TrustGate → PRStateGuard → CatalogLoad →
//! CommandMatch → PermissionGate → ArgBind → ClaimGate → Dispatch`.
//!
//! Stages are pure types — the trait and its output types live here in
//! `slash-core` (IO-free). The actual IO (GitHub API, Postgres) is
//! injected by `slash-server` through the context and trait impls.
//!
//! The `TrustGate` stage is the grants injection point (§5.2 redesign):
//! it resolves `(actor, repo, command_permission) → Granted | Denied`,
//! with fail-closed semantics (any error = Denied).

use slash_config::Permission;

// ---------------------------------------------------------------------------
// Pipeline context
// ---------------------------------------------------------------------------

/// Everything a stage needs that doesn't change between stages.
/// Constructed once per webhook event by `slash-server`.
#[derive(Debug, Clone)]
pub struct PipelineContext {
    /// The GitHub App installation id for this repository.
    pub installation_id: u64,
    /// The repository's numeric id.
    pub repository_id: u64,
    /// The repository owner (org or user).
    pub owner: String,
    /// The repository name.
    pub repo: String,
}

// ---------------------------------------------------------------------------
// Stage outputs
// ---------------------------------------------------------------------------

/// Either the pipeline proceeds to the next stage, or it terminates here
/// with a reason (which the caller maps to a user-visible feedback surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutput<T> {
    /// Proceed: carry this value into the next stage.
    Continue(T),
    /// Stop: the pipeline is done. `reason` drives the feedback.
    Terminal(TerminalReason),
}

/// Every way the pipeline can terminate before dispatch.
/// Matches the spec §5 guard order's rejection branches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalReason {
    /// Not a slash command (first line doesn't match).
    NotACommand,
    /// Permission resolution failed — author is unknown/untrusted.
    /// spec §5.2: fail-closed, 😕 reaction at most, never a comment.
    UnknownAuthor,
    /// The PR is closed or merged.
    PrNotOpen,
    /// Fork PRs are unsupported (§2.4, §11).
    ForkUnsupported,
    /// `.slash/` directory missing or unreadable (installed but not configured).
    NotConfigured,
    /// `.slash/` exists but parsing/validation failed.
    ConfigError,
    /// Command not found in the loaded catalog.
    UnknownCommand,
    /// Author's permission tier is below the command's requirement.
    PermissionDenied,
    /// Argument binding failed (missing required, bad value).
    UsageError,
    /// The PR head moved between comment capture and dispatch.
    HeadMoved,
    /// Grants say no — the actor has no matching grant for this repo+command.
    DeniedByGrants,
    /// An internal error (DB, API) that should be surfaced as a check-run
    /// failure, not a user comment.
    InternalError,
}

// ---------------------------------------------------------------------------
// Pipeline trait
// ---------------------------------------------------------------------------

/// One stage in the spec §5 guard pipeline.
///
/// Each `apply` call receives an immutable context plus the output of the
/// previous stage (or the initial input for the first stage). It returns
/// either `Continue(NextValue)` or `Terminal(Reason)`.
///
/// # Sync-composability
///
/// Stages carry no mutable state and share only `&PipelineContext`.
/// This means the whole pipeline can be tested stage-by-stage, and
/// re-run / re-request paths can reuse individual stages without
/// duplicating code.
pub trait PipelineStage<Input, Output> {
    fn apply(
        &self,
        ctx: &PipelineContext,
        input: Input,
    ) -> Result<StageOutput<Output>, TerminalReason>;
}

// ---------------------------------------------------------------------------
// TrustGate: the grants injection point  (R2 key deliverable)
// ---------------------------------------------------------------------------

/// The actor identity that `TrustGate` authorizes.
#[derive(Debug, Clone)]
pub struct Actor {
    /// The GitHub login of the comment author.
    pub login: String,
    /// Numeric GitHub user id, for identity mapping.
    pub github_user_id: u64,
}

/// Outcome of a `TrustGate` check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustOutcome {
    /// The actor is authorized at the given tier for this repo+command.
    Granted,
    /// The actor does not have a matching grant.
    Denied,
    /// An error occurred during resolution — must be treated as Denied
    /// (fail-closed, spec §5.2).
    Error(String),
}

impl TrustOutcome {
    /// fail-closed: an error during resolution is a deny.
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted)
    }
}

/// The `TrustGate` stage: resolves `(actor, repo, command_permission) →
/// Granted | Denied` by delegating to the grants system.
///
/// This trait is defined here in `slash-core` so it can be referenced from
/// the pipeline definition. The actual implementation lives in
/// `slash-server` (it needs the database to load grants).
///
/// # Fail-closed contract
///
/// Any `Err` from `check` must be treated as `Denied` by the caller.
/// The trait doc encodes this invariant; the server impl guarantees it.
pub trait TrustGate {
    /// Check whether `actor` is authorized to invoke a command that
    /// requires `command_permission` on `(owner, repo)`.
    ///
    /// # Errors
    ///
    /// Returns `TrustOutcome::Error(msg)` on any infrastructure failure
    /// (database error, missing user mapping, etc.). Callers must treat
    /// `Error` identically to `Denied` — this is the spec §5.2 fail-closed
    /// rule.
    fn check(
        &self,
        ctx: &PipelineContext,
        actor: &Actor,
        command_permission: Permission,
    ) -> TrustOutcome;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn terminal_reasons_are_sendable_across_stages() {
        // Every reason can be returned from any stage.
        let reasons = vec![
            TerminalReason::NotACommand,
            TerminalReason::UnknownAuthor,
            TerminalReason::DeniedByGrants,
        ];
        assert_eq!(reasons.len(), 3);
    }

    #[test]
    fn stage_output_continue_carries_value() {
        let output: StageOutput<i32> = StageOutput::Continue(42);
        assert_eq!(output, StageOutput::Continue(42));
    }

    #[test]
    fn stage_output_terminal_carries_reason() {
        let output: StageOutput<()> =
            StageOutput::Terminal(TerminalReason::PermissionDenied);
        assert_eq!(
            output,
            StageOutput::Terminal(TerminalReason::PermissionDenied)
        );
    }

    #[test]
    fn trust_outcome_fail_closed() {
        assert!(TrustOutcome::Granted.is_granted());
        assert!(!TrustOutcome::Denied.is_granted());
        assert!(!TrustOutcome::Error("db down".into()).is_granted());
    }

    #[test]
    fn context_is_cloneable() {
        let ctx = PipelineContext {
            installation_id: 1,
            repository_id: 100,
            owner: "acme".into(),
            repo: "widgets".into(),
        };
        let _clone = ctx.clone();
    }

    /// A trivial no-op TrustGate for use in tests that don't need grants.
    struct AllowAllTrustGate;

    impl TrustGate for AllowAllTrustGate {
        fn check(
            &self,
            _ctx: &PipelineContext,
            _actor: &Actor,
            _command_permission: Permission,
        ) -> TrustOutcome {
            TrustOutcome::Granted
        }
    }

    #[test]
    fn allow_all_trust_gate_grants_everyone() {
        let gate = AllowAllTrustGate;
        let ctx = PipelineContext {
            installation_id: 1,
            repository_id: 100,
            owner: "acme".into(),
            repo: "widgets".into(),
        };
        let actor = Actor {
            login: "alice".into(),
            github_user_id: 1,
        };
        assert!(gate.check(&ctx, &actor, Permission::Write).is_granted());
        assert!(gate.check(&ctx, &actor, Permission::Admin).is_granted());
    }

    /// A deny-all TrustGate (simulates an empty grants table).
    struct DenyAllTrustGate;

    impl TrustGate for DenyAllTrustGate {
        fn check(
            &self,
            _ctx: &PipelineContext,
            _actor: &Actor,
            _command_permission: Permission,
        ) -> TrustOutcome {
            TrustOutcome::Denied
        }
    }

    #[test]
    fn deny_all_trust_gate_denies_everyone() {
        let gate = DenyAllTrustGate;
        let ctx = PipelineContext {
            installation_id: 1,
            repository_id: 100,
            owner: "acme".into(),
            repo: "widgets".into(),
        };
        let actor = Actor {
            login: "alice".into(),
            github_user_id: 1,
        };
        assert!(!gate.check(&ctx, &actor, Permission::Write).is_granted());
    }

    /// A failing TrustGate (simulates a DB error).
    struct FailingTrustGate;

    impl TrustGate for FailingTrustGate {
        fn check(
            &self,
            _ctx: &PipelineContext,
            _actor: &Actor,
            _command_permission: Permission,
        ) -> TrustOutcome {
            TrustOutcome::Error("database unreachable".into())
        }
    }

    #[test]
    fn failing_trust_gate_is_fail_closed() {
        let gate = FailingTrustGate;
        let ctx = PipelineContext {
            installation_id: 1,
            repository_id: 100,
            owner: "acme".into(),
            repo: "widgets".into(),
        };
        let actor = Actor {
            login: "alice".into(),
            github_user_id: 1,
        };
        let outcome = gate.check(&ctx, &actor, Permission::Write);
        assert!(matches!(outcome, TrustOutcome::Error(_)));
        assert!(!outcome.is_granted(), "fail-closed: errors must deny");
    }
}
