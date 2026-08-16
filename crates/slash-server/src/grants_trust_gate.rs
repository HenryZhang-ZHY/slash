//! Grants-backed `TrustGate` impl (org/user lane, M3 #23).
//!
//! Implements `slash_core::pipeline::TrustGate` with the pure grants
//! decision (`slash_core::grants::decide`). The grants are **pre-loaded**
//! async by the server (`load_for_repo`) and handed in as `&[ResolvedGrant]`;
//! this stage is sync and IO-free (R2 Option B).

use slash_core::Decision;
use slash_core::pipeline::{Actor, ResolvedGrant, TrustGate, TrustOutcome};
use slash_config::Permission;

/// A `TrustGate` that decides via `slash_core::grants::decide`.
///
/// Stateless and pure — `check` has no IO; it only maps the pre-loaded
/// `ResolvedGrant`s back to the fields `decide` needs and runs the pure
/// decision. Fail-closed: a deny is never `Granted`; the caller treats
/// `Error` as deny too.
///
/// NOTE: not yet wired into a live pipeline stage; full wiring must wait on
/// `@Quality` adding `command`/`repository` to `slash_core::pipeline::ResolvedGrant`
/// (see the module doc) so command-scoped grants are preserved through the
/// boundary. The impl + tests here pin the pure decision.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GrantsTrustGate;

impl TrustGate for GrantsTrustGate {
    fn check(
        &self,
        grants: &[ResolvedGrant],
        _actor: &Actor,
        command: &str,
        command_permission: Permission,
    ) -> TrustOutcome {
        // The grants were pre-loaded repo-scoped by `load_for_repo`, so the
        // repo token here is a fixed placeholder that `decide` matches on;
        // command-scoped grants get the real `command`.
        let rows = slash_core::resolve_grant_rows(grants, "", command);
        match slash_core::decide(&rows, "", command, command_permission) {
            Decision::Allow => TrustOutcome::Granted,
            Decision::Denied => TrustOutcome::Denied,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use slash_core::{GrantEffect, GrantScope};
    use slash_core::pipeline::Actor;    fn rg(scope: GrantScope, effect: GrantEffect, permission: Permission) -> ResolvedGrant {
        ResolvedGrant {
            scope,
            effect,
            permission,
            command: None,
        }
    }

    // For command-scoped grant rows, assign a command name.
    fn rgc(cmd: &str, effect: GrantEffect, permission: Permission) -> ResolvedGrant {
        ResolvedGrant {
            scope: GrantScope::Command,
            effect,
            permission,
            command: Some(cmd.to_string()),
        }
    }

    fn actor() -> Actor {
        Actor { login: "alice".into(), github_user_id: 1 }
    }

    #[test]
    fn org_write_tier_allows_write_command() {
        let gate = GrantsTrustGate;
        let grants = [rg(GrantScope::Org, GrantEffect::Allow, Permission::Write)];
        assert!(gate.check(&grants, &actor(), "deploy", Permission::Write).is_granted());
    }

    #[test]
    fn org_write_tier_denies_admin_command() {
        let gate = GrantsTrustGate;
        let grants = [rg(GrantScope::Org, GrantEffect::Allow, Permission::Write)];
        assert!(!gate.check(&grants, &actor(), "release", Permission::Admin).is_granted());
    }

    #[test]
    fn deny_grant_wins_over_allow() {
        let gate = GrantsTrustGate;
        let grants = [
            rg(GrantScope::Org, GrantEffect::Allow, Permission::Admin),
            rgc("deploy", GrantEffect::Deny, Permission::Write),
        ];
        assert!(!gate.check(&grants, &actor(), "deploy", Permission::Write).is_granted());
    }

    #[test]
    fn command_scoped_deny_only_blocks_that_command() {
        // `ResolvedGrant` now carries the target command, so a command-scoped
        // deny applies to its specific command and not others.
        let gate = GrantsTrustGate;
        let grants = [
            rg(GrantScope::Org, GrantEffect::Allow, Permission::Admin),
            rgc("deploy", GrantEffect::Deny, Permission::Write),
        ];
        assert!(!gate.check(&grants, &actor(), "deploy", Permission::Write).is_granted());
        assert!(gate.check(&grants, &actor(), "release", Permission::Write).is_granted());
    }

    #[test]
    fn no_matching_grant_is_denied() {
        let gate = GrantsTrustGate;
        // org-scope allow only; a command-scope den y for a different command
        // doesn't matter; anything not allowed is denied.
        let grants: &[ResolvedGrant] = &[];
        assert!(!gate.check(grants, &actor(), "deploy", Permission::Write).is_granted());
    }
}
