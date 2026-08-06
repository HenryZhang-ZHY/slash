//! Slash's domain layer (spec §5, §6, §7.2): the invocation state machine,
//! the permission gate, the §6.2 conclusion mapping, the §6.4 anti-spam
//! primitives, and every user-facing string. Pure — testable without
//! network or database, per the repository's IO-at-the-edges rule.
//! Orchestrating these into the actual guarded pipeline against a real
//! GitHub App and Postgres is `slash-server`'s job.

mod antispam;
mod checks;
mod invocation;
pub mod messages;
mod permission;

pub use antispam::{TokenBucket, edit_distance, should_suggest_commands};
pub use checks::{CheckConclusion, map_conclusion};
pub use invocation::InvocationStatus;
pub use permission::{ResolvedRole, meets};
