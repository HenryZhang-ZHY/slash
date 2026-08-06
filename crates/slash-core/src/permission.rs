//! The spec §5.2 permission gate. Resolving a role name is IO (a GitHub API
//! call, `slash-github`'s job); comparing an already-resolved role against a
//! command's required permission is pure and lives here.

use slash_config::Permission;

/// A GitHub collaborator role, ordered by privilege. Built from
/// `role_name`, never the legacy `permission` field, which collapses
/// `maintain` into `write` and `triage` into `read` (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolvedRole {
    None,
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

impl ResolvedRole {
    pub fn from_role_name(role_name: &str) -> Option<Self> {
        match role_name {
            "none" => Some(Self::None),
            "read" => Some(Self::Read),
            "triage" => Some(Self::Triage),
            "write" => Some(Self::Write),
            "maintain" => Some(Self::Maintain),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    /// Fallback for a custom role name `from_role_name` doesn't recognize
    /// (spec §5.2): map via the `user.permissions` booleans, most-privileged
    /// first, `None` if none are set.
    pub fn from_permission_booleans(
        admin: bool,
        maintain: bool,
        push: bool,
        triage: bool,
        pull: bool,
    ) -> Self {
        if admin {
            Self::Admin
        } else if maintain {
            Self::Maintain
        } else if push {
            Self::Write
        } else if triage {
            Self::Triage
        } else if pull {
            Self::Read
        } else {
            Self::None
        }
    }
}

/// Spec §5.2: the comment author's role must be at least the command's
/// required permission (`write`, `maintain`, or `admin` — `read`/`triage`
/// are not configurable, spec §4.1).
pub fn meets(role: ResolvedRole, required: Permission) -> bool {
    let required_role = match required {
        Permission::Write => ResolvedRole::Write,
        Permission::Maintain => ResolvedRole::Maintain,
        Permission::Admin => ResolvedRole::Admin,
    };
    role >= required_role
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_known_role_names() {
        assert_eq!(
            ResolvedRole::from_role_name("none"),
            Some(ResolvedRole::None)
        );
        assert_eq!(
            ResolvedRole::from_role_name("read"),
            Some(ResolvedRole::Read)
        );
        assert_eq!(
            ResolvedRole::from_role_name("triage"),
            Some(ResolvedRole::Triage)
        );
        assert_eq!(
            ResolvedRole::from_role_name("write"),
            Some(ResolvedRole::Write)
        );
        assert_eq!(
            ResolvedRole::from_role_name("maintain"),
            Some(ResolvedRole::Maintain)
        );
        assert_eq!(
            ResolvedRole::from_role_name("admin"),
            Some(ResolvedRole::Admin)
        );
    }

    #[test]
    fn rejects_unknown_role_names() {
        assert_eq!(ResolvedRole::from_role_name("owner"), None);
    }

    #[test]
    fn falls_back_to_permission_booleans_most_privileged_first() {
        assert_eq!(
            ResolvedRole::from_permission_booleans(true, true, true, true, true),
            ResolvedRole::Admin
        );
        assert_eq!(
            ResolvedRole::from_permission_booleans(false, true, true, true, true),
            ResolvedRole::Maintain
        );
        assert_eq!(
            ResolvedRole::from_permission_booleans(false, false, true, true, true),
            ResolvedRole::Write
        );
        assert_eq!(
            ResolvedRole::from_permission_booleans(false, false, false, true, true),
            ResolvedRole::Triage
        );
        assert_eq!(
            ResolvedRole::from_permission_booleans(false, false, false, false, true),
            ResolvedRole::Read
        );
        assert_eq!(
            ResolvedRole::from_permission_booleans(false, false, false, false, false),
            ResolvedRole::None
        );
    }

    #[test]
    fn write_role_meets_a_write_gate_but_not_maintain_or_admin() {
        assert!(meets(ResolvedRole::Write, Permission::Write));
        assert!(!meets(ResolvedRole::Write, Permission::Maintain));
        assert!(!meets(ResolvedRole::Write, Permission::Admin));
    }

    #[test]
    fn maintain_role_meets_write_and_maintain_gates_but_not_admin() {
        assert!(meets(ResolvedRole::Maintain, Permission::Write));
        assert!(meets(ResolvedRole::Maintain, Permission::Maintain));
        assert!(!meets(ResolvedRole::Maintain, Permission::Admin));
    }

    #[test]
    fn admin_role_meets_every_gate() {
        assert!(meets(ResolvedRole::Admin, Permission::Write));
        assert!(meets(ResolvedRole::Admin, Permission::Maintain));
        assert!(meets(ResolvedRole::Admin, Permission::Admin));
    }

    #[test]
    fn read_and_triage_never_meet_any_configurable_gate() {
        for role in [ResolvedRole::None, ResolvedRole::Read, ResolvedRole::Triage] {
            assert!(!meets(role, Permission::Write));
        }
    }
}
