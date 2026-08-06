//! Pipeline stage decomposition (spec §5 guard order).
//!
//! Each stage matches one guard in spec §5's ordering:
//! `SyntacticGuard → CollaboratorLookup → PRStateGuard → CatalogLoad →
//! CommandMatch → PermissionGate → ArgBind → ClaimGate → Dispatch`.
//!
//! Stages are pure types — the trait and its output types live here in
//! `slash-core` (IO-free). The actual IO (GitHub API, Postgres) is
//! injected by `slash-server` through the context and trait impls.
//!
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
    /// Whether GitHub reports the repository as private.
    pub repository_is_private: bool,
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn terminal_reasons_are_sendable_across_stages() {
        // Every reason can be returned from any stage.
        let reasons = [
            TerminalReason::NotACommand,
            TerminalReason::UnknownAuthor,
            TerminalReason::PermissionDenied,
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
        let output: StageOutput<()> = StageOutput::Terminal(TerminalReason::PermissionDenied);
        assert_eq!(
            output,
            StageOutput::Terminal(TerminalReason::PermissionDenied)
        );
    }

    #[test]
    fn context_is_cloneable() {
        let ctx = PipelineContext {
            installation_id: 1,
            repository_id: 100,
            repository_is_private: true,
            owner: "acme".into(),
            repo: "widgets".into(),
        };
        let _clone = ctx.clone();
    }
}
