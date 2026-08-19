//! The `deliveries` durable inbox (spec §7.3). Claiming is one short,
//! committed transaction: the worker changes an eligible row to `processing`
//! and receives a unique fencing token plus an expiry. The GitHub pipeline
//! therefore never holds a database transaction or connection open. A worker
//! may complete or fail only the lease token it owns; after expiry, a new
//! worker can reclaim the row and the stale owner can no longer mutate it.

use sqlx::{PgPool, Postgres, Transaction};

pub const DEFAULT_LEASE_DURATION: chrono::Duration = chrono::Duration::seconds(60);
pub const MAX_ACTIVE_DELIVERIES_PER_INSTALLATION: i64 = 8;
const INSTALLATION_LOCK_NAMESPACE: i64 = i64::MIN | 0x534c_4153_4800_0000;
const MAX_SATURATED_INSTALLATIONS_TO_SKIP: usize = 32;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Delivery {
    pub delivery_guid: String,
    pub event: String,
    pub payload: Vec<u8>,
    pub attempts: i32,
    pub installation_id: Option<i64>,
    pub repository_id: Option<i64>,
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
    insert_delivery_routed(pool, guid, event, payload, None, None).await
}

pub async fn insert_delivery_routed(
    pool: &PgPool,
    guid: &str,
    event: &str,
    payload: &[u8],
    installation_id: Option<i64>,
    repository_id: Option<i64>,
) -> Result<InsertOutcome, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO deliveries \
             (delivery_guid, event, payload, installation_id, repository_id) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (delivery_guid) DO NOTHING",
    )
    .bind(guid)
    .bind(event)
    .bind(payload)
    .bind(installation_id)
    .bind(repository_id)
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
    recovered: bool,
    pub delivery: Delivery,
}

/// Claims the oldest eligible delivery with the production lease duration.
pub async fn claim_pending(pool: &PgPool) -> Result<Option<ClaimedDelivery>, sqlx::Error> {
    claim_pending_for(pool, DEFAULT_LEASE_DURATION).await
}

/// Claims one pending or expired delivery in a short transaction.
/// `FOR UPDATE SKIP LOCKED` protects candidate selection, and an
/// installation-scoped advisory lock serializes the cross-replica limit. The
/// lease is committed before this function returns.
pub async fn claim_pending_for(
    pool: &PgPool,
    lease_duration: chrono::Duration,
) -> Result<Option<ClaimedDelivery>, sqlx::Error> {
    claim_pending_with_limit(pool, lease_duration, MAX_ACTIVE_DELIVERIES_PER_INSTALLATION).await
}

async fn claim_pending_with_limit(
    pool: &PgPool,
    lease_duration: chrono::Duration,
    max_active_per_installation: i64,
) -> Result<Option<ClaimedDelivery>, sqlx::Error> {
    let mut skipped_installations = Vec::new();
    for _ in 0..MAX_SATURATED_INSTALLATIONS_TO_SKIP {
        let mut tx = pool.begin().await?;
        let Some(row) = select_candidate(&mut tx, &skipped_installations).await? else {
            tx.commit().await?;
            return Ok(None);
        };

        if let Some(installation_id) = row.installation_id {
            let lock_key = INSTALLATION_LOCK_NAMESPACE ^ installation_id;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(lock_key)
                .execute(&mut *tx)
                .await?;
            let active: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM deliveries \
                 WHERE installation_id = $1 AND state = 'processing' \
                   AND lease_expires_at > now()",
            )
            .bind(installation_id)
            .fetch_one(&mut *tx)
            .await?;
            if active >= max_active_per_installation {
                skipped_installations.push(installation_id);
                tx.commit().await?;
                continue;
            }
        }

        return claim_candidate(pool, tx, row, lease_duration)
            .await
            .map(Some);
    }
    Ok(None)
}

#[derive(sqlx::FromRow)]
struct CandidateRow {
    delivery_guid: String,
    event: String,
    payload: Vec<u8>,
    attempts: i32,
    installation_id: Option<i64>,
    repository_id: Option<i64>,
    recovered: bool,
}

async fn select_candidate(
    tx: &mut Transaction<'_, Postgres>,
    skipped_installations: &[i64],
) -> Result<Option<CandidateRow>, sqlx::Error> {
    sqlx::query_as::<_, CandidateRow>(
        "SELECT d.delivery_guid, d.event, d.payload, d.attempts, \
                d.installation_id, d.repository_id, d.state = 'processing' AS recovered \
         FROM deliveries AS d \
         WHERE ((d.state = 'pending' AND (d.next_attempt_at IS NULL OR d.next_attempt_at <= now())) \
             OR (d.state = 'processing' AND d.lease_expires_at <= now())) \
           AND (d.installation_id IS NULL OR NOT (d.installation_id = ANY($1))) \
         ORDER BY ( \
             SELECT count(*) FROM deliveries AS active \
             WHERE active.installation_id = d.installation_id \
               AND active.state = 'processing' AND active.lease_expires_at > now() \
         ), d.received_at \
         FOR UPDATE OF d SKIP LOCKED LIMIT 1",
    )
    .bind(skipped_installations)
    .fetch_optional(&mut **tx)
    .await
}

async fn claim_candidate(
    pool: &PgPool,
    mut tx: Transaction<'_, Postgres>,
    row: CandidateRow,
    lease_duration: chrono::Duration,
) -> Result<ClaimedDelivery, sqlx::Error> {
    let lease_token = uuid::Uuid::new_v4();
    let lease_expires_at = chrono::Utc::now() + lease_duration;
    let result = sqlx::query(
        "UPDATE deliveries \
         SET state = 'processing', lease_token = $2, lease_expires_at = $3, \
             attempts = attempts + 1 \
         WHERE delivery_guid = $1",
    )
    .bind(&row.delivery_guid)
    .bind(lease_token)
    .bind(lease_expires_at)
    .execute(&mut *tx)
    .await?;
    require_owned_lease(result.rows_affected())?;
    tx.commit().await?;

    Ok(ClaimedDelivery {
        pool: pool.clone(),
        lease_token,
        recovered: row.recovered,
        delivery: Delivery {
            delivery_guid: row.delivery_guid,
            event: row.event,
            payload: row.payload,
            attempts: row.attempts + 1,
            installation_id: row.installation_id,
            repository_id: row.repository_id,
        },
    })
}

impl ClaimedDelivery {
    pub fn was_recovered(&self) -> bool {
        self.recovered
    }

    /// Extends only the currently owned lease. A stale worker gets
    /// `RowNotFound` and must stop processing.
    pub async fn renew(&self, lease_duration: chrono::Duration) -> Result<(), sqlx::Error> {
        let lease_expires_at = chrono::Utc::now() + lease_duration;
        let result = sqlx::query(
            "UPDATE deliveries SET lease_expires_at = $3 \
             WHERE delivery_guid = $1 AND state = 'processing' AND lease_token = $2",
        )
        .bind(&self.delivery.delivery_guid)
        .bind(self.lease_token)
        .bind(lease_expires_at)
        .execute(&self.pool)
        .await?;
        require_owned_lease(result.rows_affected())
    }

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

    /// Releases the owned lease back to the durable queue after a delay.
    pub async fn retry_after(
        self,
        error: &str,
        delay: chrono::Duration,
    ) -> Result<(), sqlx::Error> {
        let next_attempt_at = chrono::Utc::now() + delay;
        let result = sqlx::query(
            "UPDATE deliveries \
             SET state = 'pending', last_error = $2, next_attempt_at = $3, \
                 lease_token = NULL, lease_expires_at = NULL \
             WHERE delivery_guid = $1 AND state = 'processing' AND lease_token = $4",
        )
        .bind(&self.delivery.delivery_guid)
        .bind(error)
        .bind(next_attempt_at)
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
    async fn renew_extends_only_the_current_lease() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-renew", "issue_comment", b"{}")
            .await
            .unwrap();

        let claimed = claim_pending(&pool).await.unwrap().unwrap();
        sqlx::query(
            "UPDATE deliveries SET lease_expires_at = now() + interval '1 second' \
             WHERE delivery_guid = 'guid-renew'",
        )
        .execute(&pool)
        .await
        .unwrap();
        claimed.renew(chrono::Duration::minutes(1)).await.unwrap();

        let remaining: i64 = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM (lease_expires_at - now()))::bigint \
             FROM deliveries WHERE delivery_guid = 'guid-renew'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(remaining >= 55);
        claimed.complete().await.unwrap();
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn retry_after_releases_the_lease_but_honors_the_delay() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery(&pool, "guid-retry", "issue_comment", b"{}")
            .await
            .unwrap();

        let claimed = claim_pending(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.delivery.attempts, 1);
        claimed
            .retry_after("temporary", chrono::Duration::minutes(1))
            .await
            .unwrap();
        assert!(claim_pending(&pool).await.unwrap().is_none());

        sqlx::query(
            "UPDATE deliveries SET next_attempt_at = now() - interval '1 second' \
             WHERE delivery_guid = 'guid-retry'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let retried = claim_pending(&pool).await.unwrap().unwrap();
        assert_eq!(retried.delivery.attempts, 2);
        retried.complete().await.unwrap();
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

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn installation_limit_is_shared_by_concurrent_claimers() {
        let Some(pool) = test_pool().await else {
            return;
        };
        for index in 0..4 {
            insert_delivery_routed(
                &pool,
                &format!("limited-guid-{index}"),
                "ping",
                b"{}",
                Some(42),
                Some(100 + index),
            )
            .await
            .unwrap();
        }

        let claims = tokio::join!(
            claim_pending_with_limit(&pool, DEFAULT_LEASE_DURATION, 2),
            claim_pending_with_limit(&pool, DEFAULT_LEASE_DURATION, 2),
            claim_pending_with_limit(&pool, DEFAULT_LEASE_DURATION, 2),
            claim_pending_with_limit(&pool, DEFAULT_LEASE_DURATION, 2),
        );
        let mut claimed = Vec::new();
        for result in [claims.0, claims.1, claims.2, claims.3] {
            if let Some(delivery) = result.unwrap() {
                claimed.push(delivery);
            }
        }
        assert_eq!(claimed.len(), 2);

        let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM deliveries \
             WHERE installation_id = 42 AND state = 'processing'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active, 2);
        for delivery in claimed {
            delivery.complete().await.unwrap();
        }
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_busy_installation_does_not_block_an_idle_installation() {
        let Some(pool) = test_pool().await else {
            return;
        };
        insert_delivery_routed(&pool, "busy-1", "ping", b"{}", Some(1), Some(10))
            .await
            .unwrap();
        insert_delivery_routed(&pool, "busy-2", "ping", b"{}", Some(1), Some(11))
            .await
            .unwrap();
        insert_delivery_routed(&pool, "idle-1", "ping", b"{}", Some(2), Some(20))
            .await
            .unwrap();

        let first = claim_pending_with_limit(&pool, DEFAULT_LEASE_DURATION, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.delivery.installation_id, Some(1));
        let second = claim_pending_with_limit(&pool, DEFAULT_LEASE_DURATION, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.delivery.installation_id, Some(2));

        first.complete().await.unwrap();
        second.complete().await.unwrap();
    }
}
