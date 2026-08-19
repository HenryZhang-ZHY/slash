//! The `deliveries` durable inbox (spec §7.3). Claiming is one short,
//! committed statement: the worker changes an eligible row to `processing`
//! and receives a unique fencing token plus an expiry. The GitHub pipeline
//! therefore never holds a database transaction or connection open. A worker
//! may complete or fail only the lease token it owns; after expiry, a new
//! worker can reclaim the row and the stale owner can no longer mutate it.

use sqlx::PgPool;

pub const DEFAULT_LEASE_DURATION: chrono::Duration = chrono::Duration::seconds(60);

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Delivery {
    pub delivery_guid: String,
    pub event: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    /// The delivery GUID was already present — a redelivery, and a no-op
    /// (spec §7.3, §8).
    AlreadyExists,
}

pub async fn insert_delivery(
    pool: &PgPool,
    guid: &str,
    event: &str,
    payload: &[u8],
) -> Result<InsertOutcome, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO deliveries (delivery_guid, event, payload) \
         VALUES ($1, $2, $3) ON CONFLICT (delivery_guid) DO NOTHING",
    )
    .bind(guid)
    .bind(event)
    .bind(payload)
    .execute(pool)
    .await?;

    Ok(if result.rows_affected() == 1 {
        InsertOutcome::Inserted
    } else {
        InsertOutcome::AlreadyExists
    })
}

/// Feeds `slash_deliveries_pending` (spec §7.4's inbox-depth gauge).
pub async fn count_pending(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM deliveries WHERE state = 'pending'")
            .fetch_one(pool)
            .await?;
    Ok(count)
}

/// Test-only: production code never needs to look up one delivery's state
/// by GUID outside of a claim.
#[cfg(test)]
pub(crate) async fn state_of(pool: &PgPool, guid: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT state FROM deliveries WHERE delivery_guid = $1")
            .bind(guid)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(state,)| state))
}

/// A delivery whose lease was committed before it was returned to the worker.
/// Mutations are fenced by `lease_token`; dropping this value leaves the row
/// `processing` until its lease expires and another worker reclaims it.
pub struct ClaimedDelivery {
    pool: PgPool,
    lease_token: uuid::Uuid,
    pub delivery: Delivery,
}

/// Claims the oldest eligible delivery with the production lease duration.
pub async fn claim_pending(pool: &PgPool) -> Result<Option<ClaimedDelivery>, sqlx::Error> {
    claim_pending_for(pool, DEFAULT_LEASE_DURATION).await
}

/// Claims one pending or expired delivery in a single autocommit statement.
/// `FOR UPDATE SKIP LOCKED` protects candidate selection while the update
/// commits the lease before this function returns.
pub async fn claim_pending_for(
    pool: &PgPool,
    lease_duration: chrono::Duration,
) -> Result<Option<ClaimedDelivery>, sqlx::Error> {
    let lease_token = uuid::Uuid::new_v4();
    let lease_expires_at = chrono::Utc::now() + lease_duration;
    let delivery = sqlx::query_as::<_, Delivery>(
        "WITH candidate AS ( \
             SELECT delivery_guid FROM deliveries \
             WHERE (state = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= now())) \
                OR (state = 'processing' AND lease_expires_at <= now()) \
             ORDER BY received_at \
             FOR UPDATE SKIP LOCKED LIMIT 1 \
         ) \
         UPDATE deliveries AS d \
         SET state = 'processing', lease_token = $1, lease_expires_at = $2, \
             attempts = attempts + 1 \
         FROM candidate \
         WHERE d.delivery_guid = candidate.delivery_guid \
         RETURNING d.delivery_guid, d.event, d.payload",
    )
    .bind(lease_token)
    .bind(lease_expires_at)
    .fetch_optional(pool)
    .await?;

    Ok(delivery.map(|delivery| ClaimedDelivery {
        pool: pool.clone(),
        lease_token,
        delivery,
    }))
}

impl ClaimedDelivery {
    pub async fn complete(self) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE deliveries \
             SET state = 'done', processed_at = now(), lease_token = NULL, lease_expires_at = NULL \
             WHERE delivery_guid = $1 AND state = 'processing' AND lease_token = $2",
        )
        .bind(&self.delivery.delivery_guid)
        .bind(self.lease_token)
        .execute(&self.pool)
        .await?;
        require_owned_lease(result.rows_affected())
    }

    pub async fn fail(self, error: &str) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE deliveries \
             SET state = 'failed', last_error = $2, processed_at = now(), \
                 lease_token = NULL, lease_expires_at = NULL \
             WHERE delivery_guid = $1 AND state = 'processing' AND lease_token = $3",
        )
        .bind(&self.delivery.delivery_guid)
        .bind(error)
        .bind(self.lease_token)
        .execute(&self.pool)
        .await?;
        require_owned_lease(result.rows_affected())
    }
}

fn require_owned_lease(rows_affected: u64) -> Result<(), sqlx::Error> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(sqlx::Error::RowNotFound)
    }
}

/// Spec §7.4: age of the oldest pending delivery, the inbox-stall alarm
/// (`None` when the inbox is empty).
pub async fn oldest_pending_age_seconds(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    let (age,): (Option<i64>,) = sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM (now() - min(received_at)))::bigint FROM deliveries \
         WHERE state = 'pending'",
    )
    .fetch_one(pool)
    .await?;
    Ok(age)
}

/// Deletes terminal (`done`/`failed`) deliveries older than `retention`
/// (spec §7.2's retention rule, applied here to the delivery inbox).
pub async fn delete_old_terminal(
    pool: &PgPool,
    retention: chrono::Duration,
) -> Result<u64, sqlx::Error> {
    let cutoff = chrono::Utc::now() - retention;
    let result = sqlx::query(
        "DELETE FROM deliveries WHERE state IN ('done', 'failed') AND received_at < $1",
    )
    .bind(cutoff)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db;

    /// `None` when `SLASH_TEST_DATABASE_URL` is unset — callers skip
    /// cleanly rather than failing (plan M4).
    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE deliveries, invocations")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn inserting_the_same_guid_twice_is_a_no_op() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let first = insert_delivery(&pool, "guid-1", "issue_comment", b"{}")
            .await
            .unwrap();
        let second = insert_delivery(&pool, "guid-1", "issue_comment", b"{}")
            .await
            .unwrap();
        assert_eq!(first, InsertOutcome::Inserted);
        assert_eq!(second, InsertOutcome::AlreadyExists);

        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM deliveries WHERE delivery_guid = 'guid-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn claim_and_complete_marks_the_row_done() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-2", "issue_comment", b"{}")
            .await
            .unwrap();

        let claimed = claim_pending(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.delivery.delivery_guid, "guid-2");
        claimed.complete().await.unwrap();

        assert_eq!(
            state_of(&pool, "guid-2").await.unwrap().as_deref(),
            Some("done")
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn an_unexpired_lease_is_not_claimed_twice() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-3", "issue_comment", b"{}")
            .await
            .unwrap();

        let held = claim_pending(&pool).await.unwrap().unwrap();
        // A second worker must not see a committed but unexpired lease.
        let second_attempt = claim_pending(&pool).await.unwrap();
        assert!(second_attempt.is_none());

        held.complete().await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn claim_commits_before_pipeline_database_work() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-invocation", "issue_comment", b"{}")
            .await
            .unwrap();

        let held = claim_pending(&pool).await.unwrap().unwrap();
        let id = uuid::Uuid::new_v4();
        let invocation = crate::invocations::NewInvocation {
            id,
            installation_id: 1,
            repository_id: 100,
            owner: "acme",
            repo: "widgets",
            comment_id: 101,
            attempt: 1,
            pr_number: 7,
            head_sha: "deadbeef",
            head_branch: "feature",
            actor: "alice",
            actor_id: 1,
            command: "echo",
            raw_comment_line: "/echo hi",
            args: serde_json::json!({}),
            workflow_file: "echo.yml",
            delivery_guid: Some("guid-invocation"),
        };

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::invocations::claim(&pool, &invocation),
        )
        .await
        .expect("the committed delivery lease must not block its invocation foreign-key check")
        .unwrap();
        assert_eq!(outcome, crate::invocations::ClaimOutcome::Claimed(id));

        held.complete().await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn an_expired_lease_is_reclaimed_and_fences_the_stale_worker() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-4", "issue_comment", b"{}")
            .await
            .unwrap();

        let stale = claim_pending(&pool).await.unwrap().unwrap();
        assert_eq!(stale.delivery.delivery_guid, "guid-4");

        assert_eq!(
            state_of(&pool, "guid-4").await.unwrap().as_deref(),
            Some("processing")
        );

        sqlx::query(
            "UPDATE deliveries SET lease_expires_at = now() - interval '1 second' \
             WHERE delivery_guid = 'guid-4'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let second_worker_claim = claim_pending(&pool).await.unwrap().unwrap();
        assert_eq!(second_worker_claim.delivery.delivery_guid, "guid-4");
        assert!(matches!(
            stale.complete().await,
            Err(sqlx::Error::RowNotFound)
        ));
        second_worker_claim.complete().await.unwrap();

        assert_eq!(
            state_of(&pool, "guid-4").await.unwrap().as_deref(),
            Some("done")
        );
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn fail_marks_the_row_failed_with_the_error() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-5", "issue_comment", b"{}")
            .await
            .unwrap();

        let claimed = claim_pending(&pool).await.unwrap().unwrap();
        claimed.fail("boom").await.unwrap();

        assert_eq!(
            state_of(&pool, "guid-5").await.unwrap().as_deref(),
            Some("failed")
        );
        let (last_error,): (Option<String>,) =
            sqlx::query_as("SELECT last_error FROM deliveries WHERE delivery_guid = 'guid-5'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last_error.as_deref(), Some("boom"));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn count_pending_reflects_only_pending_rows() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-6", "issue_comment", b"{}")
            .await
            .unwrap();
        insert_delivery(&pool, "guid-7", "issue_comment", b"{}")
            .await
            .unwrap();
        let claimed = claim_pending(&pool).await.unwrap().unwrap();
        claimed.complete().await.unwrap();

        assert_eq!(count_pending(&pool).await.unwrap(), 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn delete_old_terminal_only_removes_old_done_or_failed_rows() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-8", "issue_comment", b"{}")
            .await
            .unwrap();
        insert_delivery(&pool, "guid-9", "issue_comment", b"{}")
            .await
            .unwrap();
        claim_pending(&pool)
            .await
            .unwrap()
            .unwrap()
            .complete()
            .await
            .unwrap();
        // guid-9 stays pending.

        // Not old enough yet: a long retention window keeps it.
        let deleted = delete_old_terminal(&pool, chrono::Duration::days(30))
            .await
            .unwrap();
        assert_eq!(deleted, 0);

        // A zero retention window deletes anything terminal right away.
        let deleted = delete_old_terminal(&pool, chrono::Duration::zero())
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(
            state_of(&pool, "guid-9").await.unwrap().as_deref(),
            Some("pending")
        );
    }

    /// Two-worker concurrency test (plan M6/Testing Strategy): genuinely
    /// concurrent claims against one Postgres, not just the sequential
    /// held-lock check above.
    #[serial_test::serial(db)]
    #[tokio::test]
    async fn two_concurrent_workers_never_claim_the_same_pending_delivery() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "concurrent-guid-1", "issue_comment", b"{}")
            .await
            .unwrap();
        insert_delivery(&pool, "concurrent-guid-2", "issue_comment", b"{}")
            .await
            .unwrap();

        let (first, second) = tokio::join!(claim_pending(&pool), claim_pending(&pool));
        let first = first
            .unwrap()
            .expect("first worker should have claimed a row");
        let second = second
            .unwrap()
            .expect("second worker should have claimed a row");

        assert_ne!(
            first.delivery.delivery_guid, second.delivery.delivery_guid,
            "SKIP LOCKED must never let two concurrent workers claim the same row"
        );

        first.complete().await.unwrap();
        second.complete().await.unwrap();
    }
}
