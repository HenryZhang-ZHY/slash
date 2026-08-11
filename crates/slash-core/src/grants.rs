//! Offline command-authorization grants (org/user management lane, M2).
//!
//! Given a flat set of grant rows already scoped to an org + actor (direct
//! user grants and the actor's team grants), decide whether the actor may
//! invoke a command at its required permission tier. Pure — no IO — so it's
//! unit-testable without a database, per the IO-at-the-edges rule.
//!
//! Semantics (docs/design/1.0-org-grants.md, §3):
//!   * deny rows win over any allow (deny-first);
//!   * the applicable tier is the highest allowed across matched rows;
//!   * any non-matching grant is ignored; no allow reaching the required
//!     tier (and any explicit deny) resolves to `Denied`;
//!   * failures are **not** representable here: a DB error in the caller
//!     must surface as `Denied` (fail closed), it can never become allow.

use slash_config::Permission;

/// How specific a grant row is. More specific scope wins for tier selection:
/// `command` > `repository` > `org`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrantScope {
    Org,
    Repository,
    Command,
}

/// allow | deny. deny wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantEffect {
    Allow,
    Deny,
}

/// A single grant row, as produced by the server's DB query for one
/// org+actor (user direct grants + the actor's team grants). `repository`
/// and `command` are owned strings so the DB loader can build them without
/// leaking borrowed references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRow {
    pub scope: GrantScope,
    /// `owner/repo` when the grant is repository- or command-scoped.
    pub repository: Option<String>,
    /// command name when `scope == Command`.
    pub command: Option<String>,
    pub tier: Permission,
    pub effect: GrantEffect,
}

/// Convert pre-loaded `ResolvedGrant`s (from `slash_core::pipeline::TrustGate`)
/// back into full `GrantRow`s that `decide` can consume, threading the repo
/// token and command context so scope matching works.
///
/// `repo_token` must be the same value passed to `decide` as its `repo`;
/// repository-scoped grants get it attached so they match. Command-scoped
/// grants get the actual `command`.
pub fn resolve_grant_rows(
    grants: &[crate::pipeline::ResolvedGrant],
    repo: &str,
    command: &str,
) -> Vec<GrantRow> {
    grants
        .iter()
        .map(|g| GrantRow {
            scope: g.scope,
            repository: (g.scope == GrantScope::Repository).then(|| repo.to_string()),
            command: (g.scope == GrantScope::Command).then(|| command.to_string()),
            tier: g.permission,
            effect: g.effect,
        })
        .collect()
}

/// Outcome of evaluating the grants for one (repo, command) invoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// At least one allow reaches the required tier and no deny applies.
    Allow,
    /// No allow reaches the required tier, or an explicit deny applies.
    Denied,
}

/// Decide whether the actor may invoke `command` at `required` tier in
/// `repo`. `grants` must already be restricted to the actor's org + direct
/// user grants + team grants.
pub fn decide(grants: &[GrantRow], repo: &str, command: &str, required: Permission) -> Decision {
    let mut highest_allow: Option<Permission> = None;

    for g in grants {
        if !matches_scope(g, repo, command) {
            continue;
        }
        if g.effect == GrantEffect::Deny {
            // Deny-first: any matching deny wins immediately.
            return Decision::Denied;
        }
        // Keep the highest allowed tier across user + team grants.
        if highest_allow.is_none_or(|cur| tier_rank(g.tier) > tier_rank(cur)) {
            highest_allow = Some(g.tier);
        }
    }

    match highest_allow {
        Some(tier) if tier_meets(tier, required) => Decision::Allow,
        _ => Decision::Denied,
    }
}

/// Does this grant row apply to the (repo, command) context?
fn matches_scope(g: &GrantRow, repo: &str, command: &str) -> bool {
    match g.scope {
        GrantScope::Org => true,
        GrantScope::Repository => g.repository.as_deref() == Some(repo),
        GrantScope::Command => g.command.as_deref() == Some(command),
    }
}

/// Rank a grant tier against the command's required tier, reusing the
/// existing `ResolvedRole` ordering (write < maintain < admin).
pub fn tier_meets(granted: Permission, required: Permission) -> bool {
    let granted_role = permission_to_role(granted);
    let required_role = permission_to_role(required);
    granted_role >= required_role
}

fn tier_rank(p: Permission) -> u8 {
    match p {
        Permission::Read => 1,
        Permission::Write => 2,
        Permission::Admin => 3,
    }
}

fn permission_to_role(p: Permission) -> super::ResolvedRole {
    match p {
        Permission::Read => super::ResolvedRole::Read,
        Permission::Write => super::ResolvedRole::Write,
        Permission::Admin => super::ResolvedRole::Admin,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::permission::ResolvedRole;

    fn cmdgrants(
        scope: GrantScope,
        repo: Option<&str>,
        cmd: Option<&str>,
        tier: Permission,
        effect: GrantEffect,
    ) -> GrantRow {
        GrantRow {
            scope,
            repository: repo.map(str::to_string),
            command: cmd.map(str::to_string),
            tier,
            effect,
        }
    }

    #[test]
    fn no_grants_is_denied() {
        assert_eq!(decide(&[], "acme/widgets", "deploy", Permission::Write), Decision::Denied);
    }

    #[test]
    fn org_write_allows_write_command() {
        let g = [cmdgrants(GrantScope::Org, None, None, Permission::Write, GrantEffect::Allow)];
        assert_eq!(decide(&g, "acme/widgets", "deploy", Permission::Write), Decision::Allow);
    }

    #[test]
    fn org_write_does_not_allow_maintain_command() {
        let g = [cmdgrants(GrantScope::Org, None, None, Permission::Write, GrantEffect::Allow)];
        assert_eq!(decide(&g, "acme/widgets", "release", Permission::Admin), Decision::Denied);
    }

    #[test]
    fn repository_scoped_grant_only_applies_to_that_repo() {
        let g = [cmdgrants(
            GrantScope::Repository,
            Some("acme/widgets"),
            None,
            Permission::Admin,
            GrantEffect::Allow,
        )];
        assert_eq!(decide(&g, "acme/widgets", "deploy", Permission::Write), Decision::Allow);
        assert_eq!(decide(&g, "acme/other", "deploy", Permission::Write), Decision::Denied);
    }

    #[test]
    fn command_scoped_grant_only_applies_to_that_command() {
        let g = [cmdgrants(
            GrantScope::Command,
            Some("acme/widgets"),
            Some("deploy"),
            Permission::Admin,
            GrantEffect::Allow,
        )];
        assert_eq!(decide(&g, "acme/widgets", "deploy", Permission::Write), Decision::Allow);
        assert_eq!(decide(&g, "acme/widgets", "release", Permission::Write), Decision::Denied);
    }

    #[test]
    fn deny_wins_over_allow() {
        let g = [
            cmdgrants(GrantScope::Org, None, None, Permission::Admin, GrantEffect::Allow),
            cmdgrants(
                GrantScope::Command,
                Some("acme/widgets"),
                Some("deploy"),
                Permission::Write,
                GrantEffect::Deny,
            ),
        ];
        assert_eq!(decide(&g, "acme/widgets", "deploy", Permission::Write), Decision::Denied);
    }

    #[test]
    fn highest_allow_across_user_and_team_grants_wins() {
        // team grants write, user direct grant admin → admin reachable.
        let g = [
            cmdgrants(GrantScope::Org, None, None, Permission::Write, GrantEffect::Allow),
            cmdgrants(
                GrantScope::Command,
                Some("acme/widgets"),
                Some("deploy"),
                Permission::Admin,
                GrantEffect::Allow,
            ),
        ];
        assert_eq!(
            decide(&g, "acme/widgets", "deploy", Permission::Admin),
            Decision::Allow
        );
    }

    #[test]
    fn unrelated_scope_grants_are_ignored() {
        let g = [
            cmdgrants(
                GrantScope::Command,
                Some("acme/other"),
                Some("other-cmd"),
                Permission::Admin,
                GrantEffect::Allow,
            ),
            cmdgrants(
                GrantScope::Repository,
                Some("acme/other"),
                None,
                Permission::Admin,
                GrantEffect::Deny,
            ),
        ];
        // None match acme/widgets deploy → denied.
        assert_eq!(decide(&g, "acme/widgets", "deploy", Permission::Write), Decision::Denied);
    }

    #[test]
    fn tier_ordering_is_increasing() {
        assert!(tier_meets(Permission::Write, Permission::Write));
        assert!(tier_meets(Permission::Write, Permission::Read));
        assert!(tier_meets(Permission::Admin, Permission::Write));
        assert!(!tier_meets(Permission::Write, Permission::Admin));
        // Aligns with the existing ResolvedRole ordering used by permission::meets.
        assert!(ResolvedRole::Admin > ResolvedRole::Maintain);
    }
}
