//! Offline grants loader (org/user M2-3).
//!
//! Reads a repo/command's applicable grants out of the local `grants` table
//! (never a live GitHub API call) and returns the flat row set that
//! `slash_core::grants::decide` needs, plus whether the repo is **grants-only**
//! (the two-tier default-semantics switch). Loading is offline and fail-closed:
//! any DB error surfaces as `None`/false — the caller must treat that as deny.
//!
//! Wired into the permission gate by M2-4; until then the module is exercised
//! by its DB-backed tests only.
#![allow(dead_code)]

use slash_core::{GrantEffect, GrantRow, GrantScope};
use slash_config::Permission;
use sqlx::PgPool;
use uuid::Uuid;

/// Raw row projection from the grants query: (subject_type, repository,
/// command, permission, effect).
type GrantDbRow = (String, Option<String>, Option<String>, String, String);

/// Result of loading grants for one (org, actor, repo).
#[derive(Debug)]
pub struct LoadedGrants {
    /// Flat grant rows restricted to the actor (direct user grants + team
    /// grants) within the org, matching the repo's command context.
    pub rows: Vec<GrantRow>,
    /// Whether any grant row exists for this org+repo at all. When true the
    /// repo is grants-only (deny-by-default); when false the caller keeps the
    /// current fallback behavior (GitHub collaborator API).
    pub repo_is_grants_only: bool,
}

/// Load the grants applicable to `actor_user_id` in `org_id` for `repo`.
/// Exposes both the flat rows and the grants-only flag.
pub async fn load_for_repo(
    pool: &PgPool,
    org_id: Uuid,
    actor_user_id: Uuid,
    repo: &str,
) -> Result<LoadedGrants, sqlx::Error> {
    // Direct user grants + grants of the actor's teams, scoped to the org.
    let rows: Vec<GrantDbRow> = sqlx::query_as(
        "SELECT g.subject_type, g.repository, g.command, g.permission, g.effect
         FROM grants g
         LEFT JOIN team_members tm ON tm.team_id = g.subject_id
              AND g.subject_type = 'team' AND tm.user_id = $2
         WHERE g.organization_id = $1
           AND (
                (g.subject_type = 'user' AND g.subject_id = $2)
             OR (g.subject_type = 'team' AND tm.user_id = $2)
           )",
    )
    .bind(org_id)
    .bind(actor_user_id)
    .fetch_all(pool)
    .await?;

    let mut grant_rows = Vec::with_capacity(rows.len());
    for (subject_type, repository, command, permission, effect) in rows {
        let scope = match (&subject_type[..], repository.as_deref(), command.as_deref()) {
            (_, _, Some(_)) => GrantScope::Command,
            (_, Some(_), None) => GrantScope::Repository,
            _ => GrantScope::Org,
        };
        let tier = parse_permission(&permission)?;
        let effect = parse_effect(&effect)?;
        grant_rows.push(GrantRow {
            scope,
            repository,
            command,
            tier,
            effect,
        });
    }

    let repo_grants_only = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM grants
            WHERE organization_id = $1
              AND (scope = 'org' OR repository = $2)
         )",
    )
    .bind(org_id)
    .bind(repo)
    .fetch_one(pool)
    .await?;

    Ok(LoadedGrants {
        rows: grant_rows,
        repo_is_grants_only: repo_grants_only,
    })
}

/// Whether `repo`'s grants apply (i.e. it is grants-only). Fail-closed: a DB
/// error returns `false`, which under the two-tier model means "fall back to
/// the existing behavior" — securing fail-closed against grant-parsing errors
/// is the caller's responsibility, see `decide_impl`.
pub async fn repo_is_grants_only(pool: &PgPool, org_id: Uuid, repo: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM grants
            WHERE organization_id = $1 AND (scope = 'org' OR repository = $2)
         )",
    )
    .bind(org_id)
    .bind(repo)
    .fetch_one(pool)
    .await
}

fn parse_permission(s: &str) -> Result<Permission, sqlx::Error> {
    match s {
        "write" => Ok(Permission::Write),
        "maintain" => Ok(Permission::Maintain),
        "admin" => Ok(Permission::Admin),
        other => Err(sqlx::Error::Protocol(format!(
            "unexpected grant permission tier: {other}"
        ))),
    }
}

fn parse_effect(s: &str) -> Result<GrantEffect, sqlx::Error> {
    match s {
        "allow" => Ok(GrantEffect::Allow),
        "deny" => Ok(GrantEffect::Deny),
        other => Err(sqlx::Error::Protocol(format!(
            "unexpected grant effect: {other}"
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::db;
    use sqlx::PgPool;

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE grants, team_members, teams, organizations, users CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    async fn seeded(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
        // (org_id, user_id, user_id2, team_id)
        let org = Uuid::new_v4();
        let org_slug = format!("op-{}", Uuid::new_v4());
        sqlx::query("INSERT INTO organizations (id, slug, name, state) VALUES ($1,$2,'T','active')")
            .bind(org)
            .bind(&org_slug)
            .execute(pool).await.unwrap();
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        let team = Uuid::new_v4();
        let u1_email = format!("u1-{}@example.com", Uuid::new_v4());
        let u2_email = format!("u2-{}@example.com", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, status)
             VALUES ($1,$3,'x','U1','active'),($2,$4,'x','U2','active')",
        )
        .bind(u1)
        .bind(u2)
        .bind(&u1_email)
        .bind(&u2_email)
        .execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO teams (id, organization_id, name, slug, is_default_team, default_member_role)
             VALUES ($1,$2,'eng','eng',false,'member')",
        )
        .bind(team)
        .bind(org)
        .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,'member')")
            .bind(team)
            .bind(u2)
            .execute(pool).await.unwrap();
        (org, u1, u2, team)
    }

    #[serial_test::serial(db)]

    #[tokio::test]
    async fn loads_direct_and_team_grants_and_flags_grants_only() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (org, u1, u2, _team) = seeded(&pool).await;

        // User u1: write on a specific repo.
        sqlx::query(
            "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect)
             VALUES ($1,$2,'user',$3,'repository','acme/widgets',NULL,'write','allow')",
        )
        .bind(Uuid::new_v4())
        .bind(org)
        .bind(u1)
        .execute(&pool).await.unwrap();
        // Team eng: admin at org scope (u2 belongs to it).
        sqlx::query(
            "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect)
             VALUES ($1,$2,'team',$3,'org',NULL,NULL,'admin','allow')",
        )
        .bind(Uuid::new_v4())
        .bind(org)
        .bind(_team)
        .execute(&pool).await.unwrap();

        // u1: only their own direct grant matches; repo is grants-only.
        let loaded = load_for_repo(&pool, org, u1, "acme/widgets").await.unwrap();
        assert_eq!(loaded.rows.len(), 1);
        assert_eq!(loaded.rows[0].scope, GrantScope::Repository);
        assert_eq!(loaded.rows[0].tier, Permission::Write);
        assert!(loaded.repo_is_grants_only);

        // u2: gains the team org-scoped admin grant too.
        let loaded2 = load_for_repo(&pool, org, u2, "acme/widgets").await.unwrap();
        assert_eq!(loaded2.rows.len(), 1);
        assert_eq!(loaded2.rows[0].scope, GrantScope::Org);
        assert_eq!(loaded2.rows[0].tier, Permission::Admin);
    }

    #[serial_test::serial(db)]

    #[tokio::test]
    async fn repo_with_no_grants_is_not_grants_only() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (org, u1, _u2, _team) = seeded(&pool).await;
        let repo_grants_only = repo_is_grants_only(&pool, org, "acme/other").await.unwrap();
        assert!(!repo_grants_only);
        let loaded = load_for_repo(&pool, org, u1, "acme/other").await.unwrap();
        assert!(loaded.rows.is_empty());
        assert!(!loaded.repo_is_grants_only);
    }

    #[serial_test::serial(db)]

    #[tokio::test]
    async fn deny_grant_for_a_repo_is_loaded() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (org, u1, _u2, _team) = seeded(&pool).await;
        sqlx::query(
            "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect)
             VALUES ($1,$2,'user',$3,'repository','acme/widgets',NULL,'maintain','deny')",
        )
        .bind(Uuid::new_v4())
        .bind(org)
        .bind(u1)
        .execute(&pool).await.unwrap();
        let loaded = load_for_repo(&pool, org, u1, "acme/widgets").await.unwrap();
        assert_eq!(loaded.rows.len(), 1);
        assert_eq!(loaded.rows[0].effect, GrantEffect::Deny);
    }
}
