//! The `deliveries` transactional inbox (spec §7.3). A claim holds the row's
//! `FOR UPDATE SKIP LOCKED` lock inside one open transaction that spans the
//! whole pipeline; the row is marked `done`/`failed` only as part of that
//! same transaction's commit. If the process dies anywhere in between, the
//! transaction is never committed, Postgres rolls it back, and the row is
//! exactly as it was — still `pending` — for a second worker to claim. This
//! is what makes "worker killed mid-pipeline" safe without any explicit
//! "in-progress" state to get stuck in.

use sqlx::{PgPool, Postgres, Transaction};

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

/// A pending delivery claimed under `FOR UPDATE SKIP LOCKED`, holding its
/// transaction open. Dropping this without calling [`complete`] or [`fail`]
/// rolls the transaction back, leaving the row `pending`.
pub struct ClaimedDelivery<'a> {
    tx: Transaction<'a, Postgres>,
    pub delivery: Delivery,
}

/// Claims the oldest pending delivery, if any, skipping rows already locked
/// by another worker/replica.
pub async fn claim_pending(pool: &PgPool) -> Result<Option<ClaimedDelivery<'_>>, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let delivery = sqlx::query_as::<_, Delivery>(
        "SELECT delivery_guid, event, payload FROM deliveries \
         WHERE state = 'pending' ORDER BY received_at FOR UPDATE SKIP LOCKED LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;

    match delivery {
        Some(delivery) => Ok(Some(ClaimedDelivery { tx, delivery })),
        None => {
            tx.commit().await?;
            Ok(None)
        }
    }
}

impl ClaimedDelivery<'_> {
    pub async fn complete(mut self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE deliveries SET state = 'done', attempts = attempts + 1 WHERE delivery_guid = $1",
        )
        .bind(&self.delivery.delivery_guid)
        .execute(&mut *self.tx)
        .await?;
        self.tx.commit().await
    }

    pub async fn fail(mut self, error: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE deliveries SET state = 'failed', attempts = attempts + 1, last_error = $2 \
             WHERE delivery_guid = $1",
        )
        .bind(&self.delivery.delivery_guid)
        .bind(error)
        .execute(&mut *self.tx)
        .await?;
        self.tx.commit().await
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
        sqlx::query("TRUNCATE deliveries")
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
    async fn claim_pending_skips_a_row_locked_by_another_transaction() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-3", "issue_comment", b"{}")
            .await
            .unwrap();

        let held = claim_pending(&pool).await.unwrap().unwrap();
        // A second, concurrent claim attempt must not see the locked row.
        let second_attempt = claim_pending(&pool).await.unwrap();
        assert!(second_attempt.is_none());

        held.complete().await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_worker_killed_mid_pipeline_leaves_the_delivery_pending_for_a_second_worker() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-4", "issue_comment", b"{}")
            .await
            .unwrap();

        {
            let claimed = claim_pending(&pool).await.unwrap().unwrap();
            assert_eq!(claimed.delivery.delivery_guid, "guid-4");
            // Simulate a crash: drop the claim without calling `complete`.
            // The transaction rolls back and the lock is released.
        }

        assert_eq!(
            state_of(&pool, "guid-4").await.unwrap().as_deref(),
            Some("pending")
        );

        let second_worker_claim = claim_pending(&pool).await.unwrap().unwrap();
        assert_eq!(second_worker_claim.delivery.delivery_guid, "guid-4");
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
            "FOR UPDATE SKIP LOCKED must never let two concurrent workers claim the same row"
        );

        first.complete().await.unwrap();
        second.complete().await.unwrap();
    }
}
