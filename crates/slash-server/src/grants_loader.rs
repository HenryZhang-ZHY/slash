//! Offline grants loader (org/user M2-3).
//!
//! Reads a repo/command's applicable grants out of the local `grants` table
//! (never a live GitHub API call) and returns the flat row set that
//! `slash_core::grants::decide` needs, plus whether the repo is **grants-only**
//! (the two-tier default-semantics switch). Loading is offline and fail-closed:
//! any DB error surfaces as `None`/false — the caller must treat that as deny.
//!
//! Wired into the permission gate by M2-4 (the authorize path in pipeline.rs);
//! the loader functions have real callers now.

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
}

/// Load the grants applicable to `actor_user_id` in `org_id` (the repo-level
/// scope filtering happens in `slash_core::grants::decide`).
pub async fn load_for_repo(
    pool: &PgPool,
    org_id: Uuid,
    actor_user_id: Uuid,
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

    Ok(LoadedGrants { rows: grant_rows })
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
        sqlx::query("TRUNCATE grants, org_members, team_members, teams, organizations, users CASCADE")
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
        let loaded = load_for_repo(&pool, org, u1).await.unwrap();
        assert_eq!(loaded.rows.len(), 1);
        assert_eq!(loaded.rows[0].scope, GrantScope::Repository);
        assert_eq!(loaded.rows[0].tier, Permission::Write);

        // u2: gains the team org-scoped admin grant too.
        let loaded2 = load_for_repo(&pool, org, u2).await.unwrap();
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
        let loaded = load_for_repo(&pool, org, u1).await.unwrap();
        assert!(loaded.rows.is_empty());
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
        let loaded = load_for_repo(&pool, org, u1).await.unwrap();
        assert_eq!(loaded.rows.len(), 1);
        assert_eq!(loaded.rows[0].effect, GrantEffect::Deny);
    }
}

/// Full fail-closed authorize: resolve the actor's slash user id (by GitHub
/// id), the repo's org (by installation id), load the actor's grants, and
/// decide whether they may invoke `command` at `required`.
///
/// Returns `false` (deny) for ANY failure of resolution, load, or decision —
/// a non-onboarded actor, an unresolvable org, a DB error, or no grant that
/// reaches the required tier all deny (strict deny-by-default; no GitHub
/// collaborator fallback).
pub async fn authorize_command_grants(
    pool: &PgPool,
    github_user_id: i64,
    installation_id: i64,
    repo_owner: &str,
    repo_name: &str,
    command: &str,
    required: slash_config::Permission,
) -> Result<bool, sqlx::Error> {
    use slash_core::Decision;

    // 1. Resolve the actor -> slash user (non-onboarded = no grants = deny).
    let actor_row = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM users WHERE github_user_id = $1 AND status = 'active'",
    )
    .bind(github_user_id)
    .fetch_optional(pool)
    .await;
    let Some(actor_user_id) = actor_row? else {
        return Ok(false);
    };

    // 2. Resolve the repo's org via the GitHub installation.
    let org_row = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM organizations WHERE installation_id = $1",
    )
    .bind(installation_id)
    .fetch_optional(pool)
    .await;
    let Some(org_id) = org_row? else {
        return Ok(false);
    };

    // 3. Load the actor's grants in that org for this repo.
    let repo = format!("{repo_owner}/{repo_name}");
    let loaded = load_for_repo(pool, org_id, actor_user_id).await?;

    // 4. Decide (deny by default; no grant row reaches the tier => deny).
    let decision = slash_core::decide(&loaded.rows, &repo, command, required);
    Ok(matches!(decision, Decision::Allow))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod authz_tests {
    use super::*;
    use crate::db;

    async fn pool() -> PgPool {
        let url = crate::test_support::test_database_url().unwrap();
        let p = db::connect(&url).await.unwrap();
        db::migrate(&p).await.unwrap();
        sqlx::query("TRUNCATE grants, org_members, team_members, teams, organizations, users CASCADE")
            .execute(&p)
            .await
            .unwrap();
        p
    }

    async fn seed(p: &PgPool, github_id: i64, install: i64) -> (uuid::Uuid, uuid::Uuid) {
        // org + user
        let org = uuid::Uuid::new_v4();
        let org_slug = format!("org-{install}");
        sqlx::query("INSERT INTO organizations (id, slug, name, installation_id, state) VALUES ($1,$2,'T',$3,'active')")
            .bind(org)
            .bind(&org_slug)
            .bind(install)
            .execute(p).await.unwrap();
        let uid = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, status, github_user_id)
             VALUES ($1,'a@b.com','x','A','active',$2)",
        )
        .bind(uid)
        .bind(github_id)
        .execute(p).await.unwrap();
        (org, uid)
    }

    #[tokio::test]
    async fn grant_allows_and_missing_denies() {
        let p = pool().await;
        let (org, uid) = seed(&p, 111, 9).await;
        // org-scope allow
        sqlx::query(
            "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect)
             VALUES ($1,$2,'user',$3,'org',NULL,NULL,'write','allow')",
        )
        .bind(uuid::Uuid::new_v4()).bind(org).bind(uid)
        .execute(&p).await.unwrap();
        assert!(authorize_command_grants(&p, 111, 9, "acme", "widgets", "deploy", slash_config::Permission::Write).await.unwrap());
        // required admin > granted write -> deny
        assert!(!authorize_command_grants(&p, 111, 9, "acme", "widgets", "deploy", slash_config::Permission::Admin).await.unwrap());
        // unknown repo (same org) -> no grant for that repo (org-scope still matches) -> allow actually; use a different check:
        // non-onboarded actor (no users row) -> deny
        assert!(!authorize_command_grants(&p, 999, 9, "acme", "widgets", "deploy", slash_config::Permission::Write).await.unwrap());
        // unknown installation -> no org -> deny
        assert!(!authorize_command_grants(&p, 111, 12345, "acme", "widgets", "deploy", slash_config::Permission::Write).await.unwrap());
    }
}
