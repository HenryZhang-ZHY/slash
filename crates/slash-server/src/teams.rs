//! Team roster and email invitation HTTP API.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::userapi::{SessionUserId, UserId, api_error};

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MemberView {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InvitationView {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub invited_by: String,
    pub created_at: DateTime<Utc>,
    pub last_sent_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterPayload {
    pub viewer_role: String,
    pub members: Vec<MemberView>,
    pub invitations: Vec<InvitationView>,
}

#[derive(Debug, Deserialize)]
pub struct InviteRequest {
    pub email: String,
    #[serde(default = "member_role")]
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemberRequest {
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct InvitationTokenRequest {
    pub token: String,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InvitationPreview {
    pub team_name: String,
    pub team_slug: String,
    pub email: String,
    pub role: String,
    pub expires_at: DateTime<Utc>,
}

fn member_role() -> String {
    "member".to_string()
}

pub async fn roster(
    State(state): State<crate::AppState>,
    UserId(user_id): UserId,
    Path(team_id): Path<Uuid>,
) -> Response {
    let viewer_role = match team_role(&state.pool, team_id, user_id).await {
        Ok(Some(role)) => role,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "team not found"),
        Err(error) => {
            tracing::error!(%error, %team_id, %user_id, "team role lookup failed");
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not load team");
        }
    };
    let members = sqlx::query_as::<_, MemberView>(
        "SELECT u.id AS user_id, u.display_name,
                COALESCE(pc.normalized_email, ue.normalized_email) AS email,
                tm.role, tm.joined_at
         FROM team_members tm
         JOIN users u ON u.id = tm.user_id
         LEFT JOIN password_credentials pc ON pc.user_id = u.id
         LEFT JOIN LATERAL (
             SELECT normalized_email FROM user_emails
             WHERE user_id = u.id AND is_primary ORDER BY created_at LIMIT 1
         ) ue ON true
         WHERE tm.team_id = $1
         ORDER BY tm.joined_at, u.id",
    )
    .bind(team_id)
    .fetch_all(&state.pool)
    .await;
    let members = match members {
        Ok(members) => members,
        Err(error) => {
            tracing::error!(%error, %team_id, "team roster query failed");
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not load team");
        }
    };
    let invitations = if viewer_role == "maintainer" {
        match pending_invitations(&state.pool, team_id).await {
            Ok(invitations) => invitations,
            Err(error) => {
                tracing::error!(%error, %team_id, "team invitation query failed");
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not load team");
            }
        }
    } else {
        Vec::new()
    };
    Json(RosterPayload {
        viewer_role,
        members,
        invitations,
    })
    .into_response()
}

pub async fn invite(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Path(team_id): Path<Uuid>,
    Json(body): Json<InviteRequest>,
) -> Response {
    let Some(mailer) = state.invitation_mailer.as_ref() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "team invitation email is not configured",
        );
    };
    let email = normalize_email(&body.email);
    if !valid_email(&email) || !valid_role(&body.role) {
        return api_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid email or role");
    }
    let raw_token = match random_token() {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "invitation token generation failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create invitation",
            );
        }
    };
    let invitation_id = Uuid::new_v4();
    let saved = save_invitation(
        &state.pool,
        team_id,
        user_id,
        invitation_id,
        &email,
        &body.role,
        &raw_token,
    )
    .await;
    let (invitation, team_name, inviter_name) = match saved {
        Ok(saved) => saved,
        Err(InvitationError::Forbidden) => {
            return api_error(StatusCode::FORBIDDEN, "team maintainer role required");
        }
        Err(InvitationError::AlreadyMember) => {
            return api_error(StatusCode::CONFLICT, "this person is already a team member");
        }
        Err(InvitationError::Database(error)) => {
            tracing::error!(%error, %team_id, %user_id, "invitation persistence failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not create invitation",
            );
        }
    };
    if let Err(error) = mailer
        .send_team_invitation(&email, &team_name, &inviter_name, &raw_token)
        .await
    {
        tracing::error!(%error, %team_id, invitation_id = %invitation.id, "invitation email delivery failed");
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "invitation saved but email delivery failed; retry from the team page",
        );
    }
    (StatusCode::CREATED, Json(invitation)).into_response()
}

pub async fn resend(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Path((team_id, invitation_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let Some(mailer) = state.invitation_mailer.as_ref() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "team invitation email is not configured",
        );
    };
    let raw_token = match random_token() {
        Ok(token) => token,
        Err(_) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not resend invitation",
            );
        }
    };
    let digest = slash_core::hash_token(&raw_token);
    let row = sqlx::query_as::<_, (String, String, String)>(
        "UPDATE team_invitations ti
         SET token_digest = $4, last_sent_at = now(), expires_at = now() + interval '7 days'
         FROM teams t, team_members actor, users inviter
         WHERE ti.id = $2 AND ti.team_id = $1
           AND ti.accepted_at IS NULL AND ti.revoked_at IS NULL
           AND t.id = ti.team_id
           AND actor.team_id = ti.team_id AND actor.user_id = $3 AND actor.role = 'maintainer'
           AND inviter.id = $3
         RETURNING ti.normalized_email, t.name, inviter.display_name",
    )
    .bind(team_id)
    .bind(invitation_id)
    .bind(user_id)
    .bind(digest.as_slice())
    .fetch_optional(&state.pool)
    .await;
    let (email, team_name, inviter_name) = match row {
        Ok(Some(row)) => row,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "pending invitation not found"),
        Err(error) => {
            tracing::error!(%error, %team_id, %invitation_id, "invitation resend update failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not resend invitation",
            );
        }
    };
    if let Err(error) = mailer
        .send_team_invitation(&email, &team_name, &inviter_name, &raw_token)
        .await
    {
        tracing::error!(%error, %team_id, %invitation_id, "invitation resend delivery failed");
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "email delivery failed; try again",
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn revoke(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Path((team_id, invitation_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let result = sqlx::query(
        "UPDATE team_invitations ti SET revoked_at = now()
         FROM team_members actor
         WHERE ti.id = $2 AND ti.team_id = $1
           AND ti.accepted_at IS NULL AND ti.revoked_at IS NULL
           AND actor.team_id = ti.team_id AND actor.user_id = $3 AND actor.role = 'maintainer'",
    )
    .bind(team_id)
    .bind(invitation_id)
    .bind(user_id)
    .execute(&state.pool)
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => api_error(StatusCode::NOT_FOUND, "pending invitation not found"),
        Err(error) => {
            tracing::error!(%error, %team_id, %invitation_id, "invitation revocation failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not revoke invitation",
            )
        }
    }
}

pub async fn preview(
    State(state): State<crate::AppState>,
    Json(body): Json<InvitationTokenRequest>,
) -> Response {
    let digest = slash_core::hash_token(&body.token);
    let row = sqlx::query_as::<_, InvitationPreview>(
        "SELECT t.name AS team_name, t.slug AS team_slug,
                ti.normalized_email AS email, ti.role, ti.expires_at
         FROM team_invitations ti
         JOIN teams t ON t.id = ti.team_id
         JOIN organizations o ON o.id = t.organization_id
         WHERE ti.token_digest = $1 AND ti.accepted_at IS NULL
           AND ti.revoked_at IS NULL AND ti.expires_at > now()
           AND o.state = 'active'",
    )
    .bind(digest.as_slice())
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(preview)) => Json(preview).into_response(),
        Ok(None) => api_error(StatusCode::GONE, "invitation is invalid or expired"),
        Err(error) => {
            tracing::error!(%error, "invitation preview failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not load invitation",
            )
        }
    }
}

pub async fn accept(
    State(state): State<crate::AppState>,
    SessionUserId(user_id): SessionUserId,
    Json(body): Json<InvitationTokenRequest>,
) -> Response {
    match accept_invitation(&state.pool, user_id, &body.token).await {
        Ok(slug) => Json(serde_json::json!({ "teamSlug": slug })).into_response(),
        Err(AcceptError::WrongAccount) => api_error(
            StatusCode::FORBIDDEN,
            "this invitation belongs to a different Slash account",
        ),
        Err(AcceptError::Invalid) => {
            api_error(StatusCode::GONE, "invitation is invalid or expired")
        }
        Err(AcceptError::Database(error)) => {
            tracing::error!(%error, %user_id, "invitation acceptance failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not accept invitation",
            )
        }
    }
}

pub async fn update_member(
    State(state): State<crate::AppState>,
    SessionUserId(actor_id): SessionUserId,
    Path((team_id, target_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateMemberRequest>,
) -> Response {
    if !valid_role(&body.role) {
        return api_error(StatusCode::UNPROCESSABLE_ENTITY, "invalid team role");
    }
    match change_member(&state.pool, team_id, actor_id, target_id, Some(&body.role)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(MemberError::Forbidden) => {
            api_error(StatusCode::FORBIDDEN, "team maintainer role required")
        }
        Err(MemberError::LastMaintainer) => api_error(
            StatusCode::CONFLICT,
            "a team must keep at least one maintainer",
        ),
        Err(MemberError::NotFound) => api_error(StatusCode::NOT_FOUND, "team member not found"),
        Err(MemberError::Database(error)) => {
            tracing::error!(%error, %team_id, %target_id, "member role update failed");
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not update member")
        }
    }
}

pub async fn remove_member(
    State(state): State<crate::AppState>,
    SessionUserId(actor_id): SessionUserId,
    Path((team_id, target_id)): Path<(Uuid, Uuid)>,
) -> Response {
    match change_member(&state.pool, team_id, actor_id, target_id, None).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(MemberError::Forbidden) => {
            api_error(StatusCode::FORBIDDEN, "team maintainer role required")
        }
        Err(MemberError::LastMaintainer) => api_error(
            StatusCode::CONFLICT,
            "a team must keep at least one maintainer",
        ),
        Err(MemberError::NotFound) => api_error(StatusCode::NOT_FOUND, "team member not found"),
        Err(MemberError::Database(error)) => {
            tracing::error!(%error, %team_id, %target_id, "member removal failed");
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "could not remove member")
        }
    }
}

async fn team_role(
    pool: &PgPool,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT tm.role FROM team_members tm
         JOIN teams t ON t.id = tm.team_id
         JOIN organizations o ON o.id = t.organization_id
         WHERE tm.team_id = $1 AND tm.user_id = $2 AND o.state = 'active'",
    )
    .bind(team_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

async fn pending_invitations(
    pool: &PgPool,
    team_id: Uuid,
) -> Result<Vec<InvitationView>, sqlx::Error> {
    sqlx::query_as(
        "SELECT ti.id, ti.normalized_email AS email, ti.role,
                u.display_name AS invited_by, ti.created_at, ti.last_sent_at, ti.expires_at
         FROM team_invitations ti
         JOIN users u ON u.id = ti.invited_by_user_id
         WHERE ti.team_id = $1 AND ti.accepted_at IS NULL AND ti.revoked_at IS NULL
         ORDER BY ti.created_at",
    )
    .bind(team_id)
    .fetch_all(pool)
    .await
}

#[derive(Debug)]
enum InvitationError {
    Forbidden,
    AlreadyMember,
    Database(sqlx::Error),
}

async fn save_invitation(
    pool: &PgPool,
    team_id: Uuid,
    actor_id: Uuid,
    invitation_id: Uuid,
    email: &str,
    role: &str,
    token: &str,
) -> Result<(InvitationView, String, String), InvitationError> {
    let mut tx = pool.begin().await.map_err(InvitationError::Database)?;
    let context = sqlx::query_as::<_, (String, String)>(
        "SELECT t.name, u.display_name FROM teams t
         JOIN organizations o ON o.id = t.organization_id AND o.state = 'active'
         JOIN team_members tm ON tm.team_id = t.id AND tm.user_id = $2 AND tm.role = 'maintainer'
         JOIN users u ON u.id = $2
         WHERE t.id = $1",
    )
    .bind(team_id)
    .bind(actor_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(InvitationError::Database)?
    .ok_or(InvitationError::Forbidden)?;
    let invited_user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT u.id FROM users u
         JOIN password_credentials pc ON pc.user_id = u.id
         WHERE pc.normalized_email = $1 AND u.status = 'active'",
    )
    .bind(email)
    .fetch_optional(&mut *tx)
    .await
    .map_err(InvitationError::Database)?;
    if let Some(invited_user_id) = invited_user_id {
        let already_member: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id = $1 AND user_id = $2)",
        )
        .bind(team_id)
        .bind(invited_user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(InvitationError::Database)?;
        if already_member {
            return Err(InvitationError::AlreadyMember);
        }
    }
    sqlx::query(
        "UPDATE team_invitations SET revoked_at = now()
         WHERE team_id = $1 AND normalized_email = $2
           AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(team_id)
    .bind(email)
    .execute(&mut *tx)
    .await
    .map_err(InvitationError::Database)?;
    let digest = slash_core::hash_token(token);
    let invitation = sqlx::query_as::<_, InvitationView>(
        "INSERT INTO team_invitations
            (id, team_id, normalized_email, role, token_digest, invited_user_id,
             invited_by_user_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now() + interval '7 days')
         RETURNING id, normalized_email AS email, role, $8::text AS invited_by,
                   created_at, last_sent_at, expires_at",
    )
    .bind(invitation_id)
    .bind(team_id)
    .bind(email)
    .bind(role)
    .bind(digest.as_slice())
    .bind(invited_user_id)
    .bind(actor_id)
    .bind(&context.1)
    .fetch_one(&mut *tx)
    .await
    .map_err(InvitationError::Database)?;
    tx.commit().await.map_err(InvitationError::Database)?;
    Ok((invitation, context.0, context.1))
}

#[derive(Debug)]
enum AcceptError {
    Invalid,
    WrongAccount,
    Database(sqlx::Error),
}

async fn accept_invitation(
    pool: &PgPool,
    user_id: Uuid,
    token: &str,
) -> Result<String, AcceptError> {
    let digest = slash_core::hash_token(token);
    let mut tx = pool.begin().await.map_err(AcceptError::Database)?;
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, Option<Uuid>, String)>(
        "SELECT ti.id, ti.team_id, ti.role, ti.invited_user_id, t.slug
         FROM team_invitations ti
         JOIN teams t ON t.id = ti.team_id
         JOIN organizations o ON o.id = t.organization_id
         WHERE ti.token_digest = $1 AND ti.accepted_at IS NULL
           AND ti.revoked_at IS NULL AND ti.expires_at > now() AND o.state = 'active'
         FOR UPDATE OF ti",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut *tx)
    .await
    .map_err(AcceptError::Database)?
    .ok_or(AcceptError::Invalid)?;
    if row.3.is_some_and(|bound| bound != user_id) {
        return Err(AcceptError::WrongAccount);
    }
    let organization_id: Uuid =
        sqlx::query_scalar("SELECT organization_id FROM teams WHERE id = $1")
            .bind(row.1)
            .fetch_one(&mut *tx)
            .await
            .map_err(AcceptError::Database)?;
    sqlx::query(
        "INSERT INTO org_members (organization_id, user_id, role)
         VALUES ($1, $2, 'member') ON CONFLICT (organization_id, user_id) DO NOTHING",
    )
    .bind(organization_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(AcceptError::Database)?;
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, role)
         VALUES ($1, $2, $3) ON CONFLICT (team_id, user_id) DO NOTHING",
    )
    .bind(row.1)
    .bind(user_id)
    .bind(&row.2)
    .execute(&mut *tx)
    .await
    .map_err(AcceptError::Database)?;
    sqlx::query(
        "UPDATE team_invitations SET accepted_at = now(), accepted_by_user_id = $2 WHERE id = $1",
    )
    .bind(row.0)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(AcceptError::Database)?;
    tx.commit().await.map_err(AcceptError::Database)?;
    Ok(row.4)
}

#[derive(Debug)]
enum MemberError {
    Forbidden,
    LastMaintainer,
    NotFound,
    Database(sqlx::Error),
}

async fn change_member(
    pool: &PgPool,
    team_id: Uuid,
    actor_id: Uuid,
    target_id: Uuid,
    new_role: Option<&str>,
) -> Result<(), MemberError> {
    let mut tx = pool.begin().await.map_err(MemberError::Database)?;
    lock_team_members(&mut tx, team_id).await?;
    let actor_role: Option<String> =
        sqlx::query_scalar("SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(actor_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(MemberError::Database)?;
    if actor_role.as_deref() != Some("maintainer") {
        return Err(MemberError::Forbidden);
    }
    let target_role: String =
        sqlx::query_scalar("SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(target_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(MemberError::Database)?
            .ok_or(MemberError::NotFound)?;
    if target_role == "maintainer" && new_role != Some("maintainer") {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM team_members WHERE team_id = $1 AND role = 'maintainer'",
        )
        .bind(team_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(MemberError::Database)?;
        if count <= 1 {
            return Err(MemberError::LastMaintainer);
        }
    }
    if let Some(role) = new_role {
        sqlx::query("UPDATE team_members SET role = $3 WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(target_id)
            .bind(role)
            .execute(&mut *tx)
            .await
            .map_err(MemberError::Database)?;
    } else {
        sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(target_id)
            .execute(&mut *tx)
            .await
            .map_err(MemberError::Database)?;
    }
    tx.commit().await.map_err(MemberError::Database)
}

async fn lock_team_members(
    tx: &mut Transaction<'_, Postgres>,
    team_id: Uuid,
) -> Result<(), MemberError> {
    sqlx::query("SELECT user_id FROM team_members WHERE team_id = $1 FOR UPDATE")
        .bind(team_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(MemberError::Database)?;
    Ok(())
}

fn valid_role(role: &str) -> bool {
    matches!(role, "member" | "maintainer")
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn valid_email(email: &str) -> bool {
    !email.is_empty() && email.len() <= 254 && email.contains('@')
}

fn random_token() -> Result<String, ring::error::Unspecified> {
    use base64::Engine;
    let mut bytes = [0_u8; 32];
    SystemRandom::new().fill(&mut bytes)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query(
            "TRUNCATE team_invitations, org_members, team_members, teams, organizations, users CASCADE",
        )
        .execute(&pool)
        .await
        .unwrap();
        Some(pool)
    }

    async fn user(pool: &PgPool, email: &str, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, $2)")
            .bind(id)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO password_credentials (user_id, normalized_email, password_hash)
             VALUES ($1, $2, 'unused')",
        )
        .bind(id)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn team(pool: &PgPool, maintainer: Uuid) -> (Uuid, Uuid) {
        let organization_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'Acme')")
            .bind(organization_id)
            .bind(format!("org-{organization_id}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO teams (id, organization_id, name, slug) VALUES ($1, $2, 'Platform', $3)",
        )
        .bind(team_id)
        .bind(organization_id)
        .bind(format!("team-{team_id}"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'maintainer')",
        )
        .bind(team_id)
        .bind(maintainer)
        .execute(pool)
        .await
        .unwrap();
        (organization_id, team_id)
    }

    #[test]
    fn invitation_tokens_are_256_bit_url_safe_secrets() {
        let first = random_token().unwrap();
        let second = random_token().unwrap();
        assert_eq!(first.len(), 43);
        assert_ne!(first, second);
        assert!(!first.contains(['+', '/', '=']));
    }

    #[test]
    fn invitation_input_is_normalized_and_bounded() {
        assert_eq!(normalize_email(" Alice@Example.COM "), "alice@example.com");
        assert!(valid_email("alice@example.com"));
        assert!(!valid_email("alice"));
        assert!(valid_role("member"));
        assert!(!valid_role("owner"));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn invitation_binds_existing_account_and_accepts_atomically() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let owner = user(&pool, "owner@example.com", "Owner").await;
        let invited = user(&pool, "person@example.com", "Person").await;
        let other = user(&pool, "other@example.com", "Other").await;
        let (organization_id, team_id) = team(&pool, owner).await;
        let token = random_token().unwrap();
        let (view, _, _) = save_invitation(
            &pool,
            team_id,
            owner,
            Uuid::new_v4(),
            "person@example.com",
            "member",
            &token,
        )
        .await
        .unwrap();
        assert_eq!(view.email, "person@example.com");
        assert!(matches!(
            accept_invitation(&pool, other, &token).await,
            Err(AcceptError::WrongAccount)
        ));
        let slug = accept_invitation(&pool, invited, &token).await.unwrap();
        assert!(slug.starts_with("team-"));
        let team_role: String =
            sqlx::query_scalar("SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2")
                .bind(team_id)
                .bind(invited)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(team_role, "member");
        let org_member: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM org_members WHERE organization_id = $1 AND user_id = $2)",
        )
        .bind(organization_id)
        .bind(invited)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(org_member);
        assert!(matches!(
            accept_invitation(&pool, invited, &token).await,
            Err(AcceptError::Invalid)
        ));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn unregistered_email_can_be_claimed_after_registration() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let owner = user(&pool, "owner@example.com", "Owner").await;
        let (_, team_id) = team(&pool, owner).await;
        let token = random_token().unwrap();
        save_invitation(
            &pool,
            team_id,
            owner,
            Uuid::new_v4(),
            "future@example.com",
            "maintainer",
            &token,
        )
        .await
        .unwrap();
        let future = user(&pool, "future@example.com", "Future").await;
        accept_invitation(&pool, future, &token).await.unwrap();
        let role: String =
            sqlx::query_scalar("SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2")
                .bind(team_id)
                .bind(future)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(role, "maintainer");
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn last_maintainer_cannot_be_demoted_or_removed() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let owner = user(&pool, "owner@example.com", "Owner").await;
        let (_, team_id) = team(&pool, owner).await;
        assert!(matches!(
            change_member(&pool, team_id, owner, owner, Some("member")).await,
            Err(MemberError::LastMaintainer)
        ));
        assert!(matches!(
            change_member(&pool, team_id, owner, owner, None).await,
            Err(MemberError::LastMaintainer)
        ));
    }
}
