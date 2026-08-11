//! Slash's domain layer (spec §5, §6, §7.2): the invocation state machine,
//! the permission gate, the §6.2 conclusion mapping, the §6.4 anti-spam
//! primitives, and every user-facing string. Pure — testable without
//! network or database, per the repository's IO-at-the-edges rule.
//! Orchestrating these into the actual guarded pipeline against a real
//! GitHub App and Postgres is `slash-server`'s job.

mod antispam;
mod checks;
mod grants;
mod invocation;
pub mod messages;
mod permission;
pub mod pipeline;
mod test_flaky;

pub use antispam::{TokenBucket, edit_distance, should_suggest_commands};
pub use checks::{CheckConclusion, map_conclusion};
pub use grants::{Decision, GrantEffect, GrantRow, GrantScope, decide, tier_meets};
pub use invocation::InvocationStatus;
pub use permission::{ResolvedRole, meets};
pub use pipeline::{
    Actor, PipelineContext, PipelineStage, StageOutput, TerminalReason, TrustGate,
    TrustOutcome,
};
pub use test_flaky::{
    FLAKY_MIN_EXECUTIONS, ObservedStatus, has_fail_then_pass, is_flaky, recent_contains_failure,
};
