//! Grants administration API (org/user lane): list/create/delete grant rows,
//! gated to org owners. This is the management surface for the `grants` table
//! (whose authorization semantics live in `slash-core::grants` +
//! `grants_loader`); grants decide who may dispatch which command, so the
//! admin gate is the strictest: `org_members.role = 'owner'`, else 403
//! (fail-closed).
//!
//! The subject picker needs the org's members and teams, so this module also
//! serves `GET /api/org/members` (users + teams of the caller's org).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::userapi::{UserId, api_error};

/// A grant row as returned to the admin UI, with the subject's resolved name.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantView {
    pub id: Uuid,
    pub subject_type: String,
    pub subject_id: Uuid,
    pub subject_name: String,
    pub scope: String,
    pub repository: Option<String>,
    pub command: Option<String>,
    pub permission: String,
    pub effect: String,
    pub granted_by: Option<Uuid>,
    pub granted_by_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGrantRequest {
    pub subject_type: String,
    pub subject_id: Uuid,
    pub scope: String,
    pub repository: Option<String>,
    pub command: Option<String>,
    pub permission: String,
    pub effect: Option<String>,
}

/// `GET /api/grants` — list the caller's org's grants. Owner-only.
pub async fn list_grants(State(state): State<crate::AppState>, auth_user: UserId) -> Response {
    let Some(org_id) = require_org_owner(&state.pool, auth_user.0).await else {
        return api_error(StatusCode::FORBIDDEN, "only an org owner can manage grants");
    };

    let rows = sqlx::query_as::<_, (Uuid, String, Uuid, String, Option<String>, Option<String>, String, String, Option<Uuid>, Option<String>)>(
        "SELECT g.id, g.subject_type, g.subject_id, g.scope, g.repository, g.command, g.permission, g.effect, g.granted_by, granter.display_name
         FROM grants g
         LEFT JOIN users granter ON granter.id = g.granted_by
         WHERE g.organization_id = $1
         ORDER BY g.created_at",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not list grants"),
    };

    // Resolve subject names (users + teams of the org).
    let user_ids: Vec<Uuid> = rows.iter().filter(|r| r.1 == "user").map(|r| r.2).collect();
    let team_ids: Vec<Uuid> = rows.iter().filter(|r| r.1 == "team").map(|r| r.2).collect();
    let users = load_user_names(&state.pool, &user_ids)
        .await
        .unwrap_or_default();
    let teams = load_team_names(&state.pool, &team_ids)
        .await
        .unwrap_or_default();

    let views = rows
        .into_iter()
        .map(
            |(id, st, sid, scope, repo, cmd, perm, eff, granted_by, granted_by_name)| GrantView {
                id,
                subject_name: if st == "user" {
                    users.get(&sid).cloned().unwrap_or_else(|| sid.to_string())
                } else {
                    teams.get(&sid).cloned().unwrap_or_else(|| sid.to_string())
                },
                subject_type: st,
                subject_id: sid,
                scope,
                repository: repo,
                command: cmd,
                permission: perm,
                effect: eff,
                granted_by,
                granted_by_name,
            },
        )
        .collect::<Vec<_>>();

    Json(views).into_response()
}

/// `POST /api/grants` — create a grant. Owner-only; scope-conditional required
/// fields are validated, and the permission tier is normalized to the
/// read|write|admin form the schema stores.
pub async fn create_grant(
    State(state): State<crate::AppState>,
    auth_user: UserId,
    Json(body): Json<CreateGrantRequest>,
) -> Response {
    let Some(org_id) = require_org_owner(&state.pool, auth_user.0).await else {
        return api_error(StatusCode::FORBIDDEN, "only an org owner can manage grants");
    };

    // Validate scope-conditional required fields.
    let scope = body.scope.as_str();
    let repository = match scope {
        "org" => None,
        "repository" | "command" => Some(match body.repository {
            Some(repo) if !repo.is_empty() => repo,
            _ => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "repository required for this scope",
                );
            }
        }),
        _ => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "scope must be org|repository|command",
            );
        }
    };
    let command = match scope {
        "command" => Some(match body.command {
            Some(cmd) if !cmd.is_empty() => cmd,
            _ => {
                return api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "command required for command scope",
                );
            }
        }),
        _ => None,
    };

    let permission = match body.permission.as_str() {
        "read" | "write" | "admin" => body.permission,
        _ => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "permission must be read|write|admin",
            );
        }
    };
    let effect = body.effect.unwrap_or_else(|| "allow".to_string());
    if effect != "allow" && effect != "deny" {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "effect must be allow|deny",
        );
    }
    if body.subject_type != "user" && body.subject_type != "team" {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "subject_type must be user|team",
        );
    }

    // Verify the subject belongs to the org (fail-closed: unknown subject -> 422).
    if !subject_in_org(&state.pool, org_id, &body.subject_type, body.subject_id).await {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "subject does not belong to this org",
        );
    }

    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO grants (id, organization_id, subject_type, subject_id, scope, repository, command, permission, effect, granted_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(&body.subject_type)
    .bind(body.subject_id)
    .bind(scope)
    .bind(&repository)
    .bind(&command)
    .bind(&permission)
    .bind(&effect)
    .bind(auth_user.0)
    .execute(&state.pool)
    .await;

    match inserted {
        Ok(r) if r.rows_affected() == 1 => Json(json!({ "id": id })).into_response(),
        Ok(_) => api_error(
            StatusCode::CONFLICT,
            "a grant with these fields already exists",
        ),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create grant"),
    }
}

/// `DELETE /api/grants/{id}` — delete a grant. Owner-only.
pub async fn delete_grant(
    State(state): State<crate::AppState>,
    auth_user: UserId,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(org_id) = require_org_owner(&state.pool, auth_user.0).await else {
        return api_error(StatusCode::FORBIDDEN, "only an org owner can manage grants");
    };

    let deleted = sqlx::query("DELETE FROM grants WHERE id = $1 AND organization_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(&state.pool)
        .await;

    match deleted {
        Ok(r) if r.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => api_error(StatusCode::NOT_FOUND, "grant not found"),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not delete grant"),
    }
}

/// `GET /api/org/members` — the caller's org's users + teams, for the grants
/// subject picker. Owner-only (the grants admin surface).
pub async fn org_members(State(state): State<crate::AppState>, auth_user: UserId) -> Response {
    let Some(org_id) = require_org_owner(&state.pool, auth_user.0).await else {
        return api_error(StatusCode::FORBIDDEN, "only an org owner can manage grants");
    };

    let users = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT u.id, u.display_name FROM org_members om
         JOIN users u ON u.id = om.user_id
         WHERE om.organization_id = $1 ORDER BY u.display_name",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await;
    let users = match users {
        Ok(users) => users
            .into_iter()
            .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
            .collect::<Vec<_>>(),
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not list members"),
    };

    let teams = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM teams WHERE organization_id = $1 ORDER BY name",
    )
    .bind(org_id)
    .fetch_all(&state.pool)
    .await;
    let teams = match teams {
        Ok(teams) => teams
            .into_iter()
            .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
            .collect::<Vec<_>>(),
        Err(_) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not list teams"),
    };

    Json(json!({ "orgId": org_id, "users": users, "teams": teams })).into_response()
}

// ---- helpers -----------------------------------------------------------------

/// Returns the caller's org id if they are an org **owner**, else `None`
/// (fail-closed for the grants admin gate). Org ownership lives in
/// `org_members.role`, independent of team membership.
async fn require_org_owner(pool: &PgPool, user_id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM org_members
         WHERE user_id = $1 AND role = 'owner'
         ORDER BY joined_at LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}
async fn subject_in_org(pool: &PgPool, org_id: Uuid, subject_type: &str, subject_id: Uuid) -> bool {
    if subject_type == "user" {
        sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM org_members WHERE organization_id = $1 AND user_id = $2",
        )
        .bind(org_id)
        .bind(subject_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
    } else {
        sqlx::query_scalar::<_, i32>("SELECT 1 FROM teams WHERE id = $1 AND organization_id = $2")
            .bind(subject_id)
            .bind(org_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .is_some()
    }
}

async fn load_user_names(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, display_name FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

async fn load_team_names(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM teams WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::db;
    use axum::http::StatusCode;
    use sqlx::PgPool;

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query(
            "TRUNCATE grants, org_members, team_members, teams, organizations, users CASCADE",
        )
        .execute(&pool)
        .await
        .unwrap();
        Some(pool)
    }

    /// Seeds an org with an owner, a member, and a team. Returns
    /// (org_id, owner_id, member_id, team_id).
    async fn seeded(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
        let org = Uuid::new_v4();
        let org_slug = format!("g-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, state) VALUES ($1,$2,'G','active')",
        )
        .bind(org)
        .bind(&org_slug)
        .execute(pool)
        .await
        .unwrap();
        let owner = Uuid::new_v4();
        let member = Uuid::new_v4();
        let team = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, status)
             VALUES ($1,$3,'x','Owner','active'),($2,$4,'x','Member','active')",
        )
        .bind(owner)
        .bind(member)
        .bind(format!("o-{}@example.com", Uuid::new_v4()))
        .bind(format!("m-{}@example.com", Uuid::new_v4()))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO org_members (organization_id, user_id, role) VALUES
             ($1,$2,'owner'),($1,$3,'member')",
        )
        .bind(org)
        .bind(owner)
        .bind(member)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO teams (id, organization_id, name, slug, is_default_team, default_member_role)
             VALUES ($1,$2,'eng','eng',false,'member')",
        )
        .bind(team).bind(org)
        .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,'member')")
            .bind(team)
            .bind(member)
            .execute(pool)
            .await
            .unwrap();
        (org, owner, member, team)
    }

    fn state(pool: PgPool) -> crate::AppState {
        crate::AppState {
            pool,
            metrics: std::sync::Arc::new(crate::metrics::Metrics::new().unwrap()),
            webhook_secret: std::sync::Arc::from("test-webhook-secret"),
            auth_secret: crate::auth::AuthSecret(std::sync::Arc::from("test-auth-secret")),
            web_dir: std::sync::Arc::from(""),
            github_oauth: None,
        }
    }

    fn status(res: &Response) -> StatusCode {
        res.status()
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn non_owner_cannot_list_or_create_grants() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, _owner, member, _team) = seeded(&pool).await;
        let st = state(pool);
        let resp = list_grants(State(st.clone()), UserId(member)).await;
        assert_eq!(status(&resp), StatusCode::FORBIDDEN);
        let resp = create_grant(
            State(st.clone()),
            UserId(member),
            Json(CreateGrantRequest {
                subject_type: "user".into(),
                subject_id: member,
                scope: "org".into(),
                repository: None,
                command: None,
                permission: "read".into(),
                effect: Some("allow".into()),
            }),
        )
        .await;
        assert_eq!(status(&resp), StatusCode::FORBIDDEN);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn owner_creates_lists_and_deletes_a_grant() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, owner, member, _team) = seeded(&pool).await;
        let st = state(pool.clone());
        let resp = create_grant(
            State(st.clone()),
            UserId(owner),
            Json(CreateGrantRequest {
                subject_type: "user".into(),
                subject_id: member,
                scope: "command".into(),
                repository: Some("acme/widgets".into()),
                command: Some("deploy".into()),
                permission: "write".into(),
                effect: Some("deny".into()),
            }),
        )
        .await;
        assert_eq!(status(&resp), StatusCode::OK);

        let resp = list_grants(State(st.clone()), UserId(owner)).await;
        assert_eq!(status(&resp), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let views: Vec<GrantView> = serde_json::from_slice(&body).unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].subject_type, "user");
        assert_eq!(views[0].subject_name, "Member");
        assert_eq!(views[0].scope, "command");
        assert_eq!(views[0].repository.as_deref(), Some("acme/widgets"));
        assert_eq!(views[0].command.as_deref(), Some("deploy"));
        assert_eq!(views[0].permission, "write");
        assert_eq!(views[0].effect, "deny");
        assert_eq!(views[0].granted_by_name.as_deref(), Some("Owner"));

        let resp = delete_grant(State(st.clone()), UserId(owner), Path(views[0].id)).await;
        assert_eq!(status(&resp), StatusCode::NO_CONTENT);
        let resp = list_grants(State(st), UserId(owner)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let views: Vec<GrantView> = serde_json::from_slice(&body).unwrap();
        assert!(views.is_empty());
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn create_scope_validation_rejects_missing_command() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, owner, member, _team) = seeded(&pool).await;
        let st = state(pool);
        let resp = create_grant(
            State(st),
            UserId(owner),
            Json(CreateGrantRequest {
                subject_type: "user".into(),
                subject_id: member,
                scope: "command".into(),
                repository: Some("acme/widgets".into()),
                command: None,
                permission: "write".into(),
                effect: Some("allow".into()),
            }),
        )
        .await;
        assert_eq!(status(&resp), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn create_rejects_subject_outside_the_org() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, owner, _member, _team) = seeded(&pool).await;
        let stranger = Uuid::new_v4();
        let st = state(pool);
        let resp = create_grant(
            State(st),
            UserId(owner),
            Json(CreateGrantRequest {
                subject_type: "user".into(),
                subject_id: stranger,
                scope: "org".into(),
                repository: None,
                command: None,
                permission: "read".into(),
                effect: Some("allow".into()),
            }),
        )
        .await;
        assert_eq!(status(&resp), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn org_members_lists_users_and_teams_for_owner() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, owner, _member, team) = seeded(&pool).await;
        let st = state(pool);
        let resp = org_members(State(st), UserId(owner)).await;
        assert_eq!(status(&resp), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let users = json["users"].as_array().unwrap();
        assert_eq!(users.len(), 2);
        let teams = json["teams"].as_array().unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0]["id"], serde_json::json!(team));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn org_members_denies_a_plain_member() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, _owner, member, _team) = seeded(&pool).await;
        let st = state(pool);
        let resp = org_members(State(st), UserId(member)).await;
        assert_eq!(status(&resp), StatusCode::FORBIDDEN);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn duplicate_org_scope_grant_returns_conflict() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (_org, owner, member, _team) = seeded(&pool).await;
        let st = state(pool.clone());
        let body = || {
            Json(CreateGrantRequest {
                subject_type: "user".into(),
                subject_id: member,
                scope: "org".into(),
                repository: None,
                command: None,
                permission: "read".into(),
                effect: Some("allow".into()),
            })
        };
        let first = create_grant(State(st.clone()), UserId(owner), body()).await;
        assert_eq!(status(&first), StatusCode::OK);
        // Same org-scope grant again: NULLs are NOT DISTINCT now, so the
        // unique constraint fires and the API reports 409.
        let second = create_grant(State(st), UserId(owner), body()).await;
        assert_eq!(status(&second), StatusCode::CONFLICT);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM grants")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
