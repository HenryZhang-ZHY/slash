//! Provider-neutral external identity persistence.
//!
//! Protocol adapters normalize their provider response into an
//! [`AuthenticatedIdentity`]. Account creation and explicit account linking
//! then operate only on the configured trust domain (`connection_id`) and the
//! provider-stable subject. Mutable profile data is never an account key.

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedIdentity {
    pub connection_id: Uuid,
    pub subject: String,
    pub username: String,
    pub display_name: String,
    pub profile: Value,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IdentityError {
    #[error("this external identity is connected to another Slash account")]
    IdentityInUse,
    #[error("this Slash account already has a different identity for this connection")]
    ConnectionAlreadyLinked,
    #[error("the Slash account is not available")]
    UserUnavailable,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

pub(crate) async fn sign_in_or_create(
    pool: &PgPool,
    identity: &AuthenticatedIdentity,
) -> Result<Uuid, IdentityError> {
    let mut tx = pool.begin().await?;
    let existing = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT u.id, u.status
         FROM user_identities ui
         JOIN users u ON u.id = ui.user_id
         WHERE ui.connection_id = $1 AND ui.subject = $2
         FOR UPDATE OF ui, u",
    )
    .bind(identity.connection_id)
    .bind(&identity.subject)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((user_id, status)) = existing {
        if status != "active" {
            return Err(IdentityError::UserUnavailable);
        }
        update_identity(&mut tx, user_id, identity).await?;
        tx.commit().await?;
        return Ok(user_id);
    }

    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, display_name, status)
         VALUES ($1, $2, 'active')",
    )
    .bind(user_id)
    .bind(&identity.display_name)
    .execute(&mut *tx)
    .await
    .map_err(map_constraint)?;
    insert_identity(&mut tx, user_id, identity)
        .await
        .map_err(map_constraint)?;
    tx.commit().await?;
    Ok(user_id)
}

pub(crate) async fn connect(
    pool: &PgPool,
    user_id: Uuid,
    identity: &AuthenticatedIdentity,
) -> Result<(), IdentityError> {
    let mut tx = pool.begin().await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if status.as_deref() != Some("active") {
        return Err(IdentityError::UserUnavailable);
    }

    let subject_owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id
         FROM user_identities
         WHERE connection_id = $1 AND subject = $2
         FOR UPDATE",
    )
    .bind(identity.connection_id)
    .bind(&identity.subject)
    .fetch_optional(&mut *tx)
    .await?;
    if subject_owner.is_some_and(|owner| owner != user_id) {
        return Err(IdentityError::IdentityInUse);
    }

    let current_subject: Option<String> = sqlx::query_scalar(
        "SELECT subject
         FROM user_identities
         WHERE connection_id = $1 AND user_id = $2
         FOR UPDATE",
    )
    .bind(identity.connection_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    match current_subject {
        Some(subject) if subject != identity.subject => {
            return Err(IdentityError::ConnectionAlreadyLinked);
        }
        Some(_) => update_identity(&mut tx, user_id, identity).await?,
        None => insert_identity(&mut tx, user_id, identity)
            .await
            .map_err(map_constraint)?,
    }
    tx.commit().await?;
    Ok(())
}

async fn insert_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    identity: &AuthenticatedIdentity,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_identities
            (id, user_id, connection_id, subject, username, display_name, profile)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(identity.connection_id)
    .bind(&identity.subject)
    .bind(&identity.username)
    .bind(&identity.display_name)
    .bind(&identity.profile)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    identity: &AuthenticatedIdentity,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE user_identities
         SET username = $1, display_name = $2, profile = $3,
             last_authenticated_at = now(), updated_at = now()
         WHERE user_id = $4 AND connection_id = $5 AND subject = $6",
    )
    .bind(&identity.username)
    .bind(&identity.display_name)
    .bind(&identity.profile)
    .bind(user_id)
    .bind(identity.connection_id)
    .bind(&identity.subject)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn map_constraint(error: sqlx::Error) -> IdentityError {
    let constraint = match &error {
        sqlx::Error::Database(database) => database.constraint(),
        _ => None,
    };
    match constraint {
        Some("user_identities_connection_subject_unique") => IdentityError::IdentityInUse,
        Some("user_identities_user_connection_unique") => IdentityError::ConnectionAlreadyLinked,
        _ => IdentityError::Database(error),
    }
}
