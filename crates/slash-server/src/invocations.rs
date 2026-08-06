//! Postgres repository for the `invocations` table (spec §7.2). The state
//! machine rules themselves live in `slash_core::InvocationStatus`; this
//! module only turns them into guarded SQL (`UPDATE ... WHERE status IN
//! (...)`, per that module's `can_transition_to`).

use serde_json::Value as Json;
use slash_core::InvocationStatus;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewInvocation<'a> {
    pub id: Uuid,
    pub installation_id: i64,
    pub repository_id: i64,
    pub owner: &'a str,
    pub repo: &'a str,
    pub comment_id: i64,
    pub attempt: i32,
    pub pr_number: i64,
    pub head_sha: &'a str,
    pub head_branch: &'a str,
    pub actor: &'a str,
    pub actor_id: i64,
    pub command: &'a str,
    pub raw_comment_line: &'a str,
    pub args: Json,
    pub workflow_file: &'a str,
    pub delivery_guid: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// A fresh row was claimed; the pre-dispatch pipeline may proceed.
    Claimed(Uuid),
    /// `(installation_id, comment_id, attempt)` already exists and is past
    /// `claimed` — a duplicate delivery; later states are owned by
    /// `workflow_run` events and the sweeper (spec §5).
    AlreadyClaimed,
    /// The existing row is still `claimed` — a resumable, stranded pipeline
    /// (spec §6.1): every pre-dispatch step is idempotent, so the caller
    /// should resume it rather than starting over.
    Resume(Uuid),
}

/// The §5 idempotency claim: `UNIQUE(installation_id, comment_id, attempt)`.
pub async fn claim(pool: &PgPool, new: &NewInvocation<'_>) -> Result<ClaimOutcome, sqlx::Error> {
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO invocations (
            id, installation_id, repository_id, owner, repo, comment_id, attempt,
            pr_number, head_sha, head_branch, actor, actor_id,
            command, raw_comment_line, args, workflow_file, delivery_guid, status
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, 'claimed')
         ON CONFLICT (installation_id, comment_id, attempt) DO NOTHING
         RETURNING id",
    )
    .bind(new.id)
    .bind(new.installation_id)
    .bind(new.repository_id)
    .bind(new.owner)
    .bind(new.repo)
    .bind(new.comment_id)
    .bind(new.attempt)
    .bind(new.pr_number)
    .bind(new.head_sha)
    .bind(new.head_branch)
    .bind(new.actor)
    .bind(new.actor_id)
    .bind(new.command)
    .bind(new.raw_comment_line)
    .bind(&new.args)
    .bind(new.workflow_file)
    .bind(new.delivery_guid)
    .fetch_optional(pool)
    .await?;

    if let Some((id,)) = inserted {
        return Ok(ClaimOutcome::Claimed(id));
    }

    let existing: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM invocations \
         WHERE installation_id = $1 AND comment_id = $2 AND attempt = $3",
    )
    .bind(new.installation_id)
    .bind(new.comment_id)
    .bind(new.attempt)
    .fetch_optional(pool)
    .await?;

    Ok(match existing {
        Some((id, status)) if status == InvocationStatus::Claimed.as_str() => {
            ClaimOutcome::Resume(id)
        }
        _ => ClaimOutcome::AlreadyClaimed,
    })
}

/// A guarded `status` transition (spec §7.2): zero rows updated means the
/// invocation is no longer in a state this transition applies from — the
/// event is stale or duplicate, and dropped rather than erroring.
pub async fn transition_status(
    pool: &PgPool,
    id: Uuid,
    to: InvocationStatus,
) -> Result<bool, sqlx::Error> {
    // Derived, never caller-supplied: the only way to ask "what may
    // legally precede `to`" is `slash_core`'s own state machine, so a
    // guarded update here can never drift from `can_transition_to`.
    let from_strs: Vec<&str> = InvocationStatus::valid_predecessors(to)
        .iter()
        .map(|s| s.as_str())
        .collect();
    let column = match to {
        InvocationStatus::Dispatched => "dispatched_at",
        InvocationStatus::Correlated => "correlated_at",
        InvocationStatus::Completed
        | InvocationStatus::Aborted
        | InvocationStatus::DispatchFailed
        | InvocationStatus::CorrelationTimeout
        | InvocationStatus::Superseded => "completed_at",
        InvocationStatus::Claimed => "created_at",
    };
    let sql = format!(
        "UPDATE invocations SET status = $1, {column} = now() \
         WHERE id = $2 AND status = ANY($3)"
    );
    let result = sqlx::query(&sql)
        .bind(to.as_str())
        .bind(id)
        .bind(&from_strs)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_check_run_id(
    pool: &PgPool,
    id: Uuid,
    check_run_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invocations SET check_run_id = $2 WHERE id = $1")
        .bind(id)
        .bind(check_run_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_workflow_run_id(
    pool: &PgPool,
    id: Uuid,
    workflow_run_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invocations SET workflow_run_id = $2 WHERE id = $1")
        .bind(id)
        .bind(workflow_run_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_failure_reason(pool: &PgPool, id: Uuid, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invocations SET failure_reason = $2 WHERE id = $1")
        .bind(id)
        .bind(reason)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SupersedeCandidate {
    pub id: Uuid,
    pub check_run_id: Option<i64>,
}

/// Finds older non-terminal invocations of the same `(repo, pr, command)`
/// triple, excluding `except_id` (spec §6.7). The caller supersedes each one
/// found and writes the final "superseded" update to its check run.
pub async fn find_supersede_candidates(
    pool: &PgPool,
    installation_id: i64,
    owner: &str,
    repo: &str,
    pr_number: i64,
    command: &str,
    except_id: Uuid,
) -> Result<Vec<SupersedeCandidate>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, check_run_id FROM invocations \
         WHERE installation_id = $1 AND owner = $2 AND repo = $3 AND pr_number = $4 \
           AND command = $5 AND id != $6 \
           AND status NOT IN ('completed', 'aborted', 'dispatch_failed', 'correlation_timeout', 'superseded')",
    )
    .bind(installation_id)
    .bind(owner)
    .bind(repo)
    .bind(pr_number)
    .bind(command)
    .bind(except_id)
    .fetch_all(pool)
    .await
}

/// The subset of an invocation row M6's correlation and re-run handlers
/// need. A separate, narrower shape than [`NewInvocation`] (which is for
/// writing), since reads here only ever need a handful of columns.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Invocation {
    pub id: Uuid,
    pub installation_id: i64,
    pub repository_id: i64,
    pub owner: String,
    pub repo: String,
    pub comment_id: i64,
    pub attempt: i32,
    pub pr_number: i64,
    pub head_sha: String,
    pub head_branch: String,
    pub actor: String,
    pub command: String,
    pub raw_comment_line: String,
    pub check_run_id: Option<i64>,
    pub workflow_file: String,
    pub workflow_run_id: Option<i64>,
    pub status: String,
    pub last_reported_status: Option<String>,
    // The stale-row cutoffs are computed and bound independently in each
    // `find_stale_*` query; no caller currently needs the row's own value
    // back.
    #[allow(dead_code)]
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub dispatched_at: Option<chrono::DateTime<chrono::Utc>>,
}

const INVOCATION_COLUMNS: &str = "id, installation_id, repository_id, owner, repo, comment_id, attempt, pr_number, \
     head_sha, head_branch, actor, command, raw_comment_line, check_run_id, workflow_file, \
     workflow_run_id, status, last_reported_status, created_at, dispatched_at";

/// Correlation is exact and authenticated by construction (spec §6.3):
/// matched only by run id, never by any attacker-influenceable text.
pub async fn find_by_workflow_run_id(
    pool: &PgPool,
    installation_id: i64,
    owner: &str,
    repo: &str,
    workflow_run_id: i64,
) -> Result<Option<Invocation>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {INVOCATION_COLUMNS} FROM invocations \
         WHERE installation_id = $1 AND owner = $2 AND repo = $3 AND workflow_run_id = $4"
    ))
    .bind(installation_id)
    .bind(owner)
    .bind(repo)
    .bind(workflow_run_id)
    .fetch_optional(pool)
    .await
}

/// Used by the `check_run.rerequested` handler (spec §6.5) to find the
/// original invocation and re-gate on the rerequester.
pub async fn find_by_check_run_id(
    pool: &PgPool,
    installation_id: i64,
    owner: &str,
    repo: &str,
    check_run_id: i64,
) -> Result<Option<Invocation>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {INVOCATION_COLUMNS} FROM invocations \
         WHERE installation_id = $1 AND owner = $2 AND repo = $3 AND check_run_id = $4"
    ))
    .bind(installation_id)
    .bind(owner)
    .bind(repo)
    .bind(check_run_id)
    .fetch_optional(pool)
    .await
}

/// Invocations stranded in `claimed` past `older_than` — crashed before the
/// write-ahead `dispatched` transition, and before the POST (spec §7.2). The
/// sweeper collects these into `aborted`.
pub async fn find_stale_claimed(
    pool: &PgPool,
    older_than: chrono::Duration,
) -> Result<Vec<Invocation>, sqlx::Error> {
    let cutoff = chrono::Utc::now() - older_than;
    sqlx::query_as(&format!(
        "SELECT {INVOCATION_COLUMNS} FROM invocations \
         WHERE status = 'claimed' AND created_at < $1"
    ))
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// Invocations `dispatched` past `older_than` with no `workflow_run_id` —
/// an ambiguous dispatch outcome (spec §6.3, §7.6): the POST may or may not
/// have created a run. The sweeper resolves these via the missing-run-id
/// poll, never by re-POSTing.
pub async fn find_stale_dispatched_unresolved(
    pool: &PgPool,
    older_than: chrono::Duration,
) -> Result<Vec<Invocation>, sqlx::Error> {
    let cutoff = chrono::Utc::now() - older_than;
    sqlx::query_as(&format!(
        "SELECT {INVOCATION_COLUMNS} FROM invocations \
         WHERE status = 'dispatched' AND workflow_run_id IS NULL AND dispatched_at < $1"
    ))
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

/// Invocations `correlated` past `older_than` (spec §6.3's 72h run
/// deadline): either the `completed` webhook was lost (re-fetching resolves
/// it normally) or the run is genuinely wedged (force-completed `timed_out`).
pub async fn find_stale_correlated(
    pool: &PgPool,
    older_than: chrono::Duration,
) -> Result<Vec<Invocation>, sqlx::Error> {
    let cutoff = chrono::Utc::now() - older_than;
    sqlx::query_as(&format!(
        "SELECT {INVOCATION_COLUMNS} FROM invocations \
         WHERE status = 'correlated' AND dispatched_at < $1"
    ))
    .bind(cutoff)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRunIdOutcome {
    /// This invocation now owns `workflow_run_id`.
    Claimed,
    /// Already resolved (by this poll or another replica) since being
    /// listed as a candidate; the caller should stop, not try another run.
    AlreadyResolved,
    /// `UNIQUE(installation_id, owner, repo, workflow_run_id)`: some other
    /// invocation already owns this run id. Try the next candidate.
    RunIdTaken,
}

/// Atomically claims a candidate run id found by the missing-run-id poll
/// (spec §6.3). The `UNIQUE` constraint is what makes a double claim
/// impossible even across replicas polling concurrently.
pub async fn claim_workflow_run_id_if_unresolved(
    pool: &PgPool,
    id: Uuid,
    workflow_run_id: i64,
) -> Result<ClaimRunIdOutcome, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE invocations SET workflow_run_id = $2 WHERE id = $1 AND workflow_run_id IS NULL",
    )
    .bind(id)
    .bind(workflow_run_id)
    .execute(pool)
    .await;

    match result {
        Ok(result) if result.rows_affected() == 1 => Ok(ClaimRunIdOutcome::Claimed),
        Ok(_) => Ok(ClaimRunIdOutcome::AlreadyResolved),
        Err(sqlx::Error::Database(db_error)) if db_error.is_unique_violation() => {
            Ok(ClaimRunIdOutcome::RunIdTaken)
        }
        Err(error) => Err(error),
    }
}

/// Spec §7.4: invocation counts by status, for the `slash_invocations{status}`
/// gauge. Only present statuses are returned; the caller zeroes every known
/// status first so one that just emptied out doesn't linger at a stale value.
pub async fn count_by_status(pool: &PgPool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as("SELECT status, count(*) FROM invocations GROUP BY status")
        .fetch_all(pool)
        .await
}

/// Spec §7.4's stuck-invocation alarm: how long the oldest still-`dispatched`
/// invocation has been waiting for a run id, in seconds (`None` when there
/// are none).
pub async fn max_dispatched_age_seconds(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    let (age,): (Option<i64>,) = sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM (now() - min(dispatched_at)))::bigint FROM invocations \
         WHERE status = 'dispatched'",
    )
    .fetch_one(pool)
    .await?;
    Ok(age)
}

pub async fn set_conclusion(pool: &PgPool, id: Uuid, conclusion: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invocations SET conclusion = $2 WHERE id = $1")
        .bind(id)
        .bind(conclusion)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_last_reported_status(
    pool: &PgPool,
    id: Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invocations SET last_reported_status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

/// `pull_request.synchronize` (spec §7.3): records the new head SHA on the
/// PR's non-terminal invocations, purely informational for the eventual
/// completion summary — it never re-triggers anything (spec §2.4).
pub async fn record_new_head_sha(
    pool: &PgPool,
    installation_id: i64,
    owner: &str,
    repo: &str,
    pr_number: i64,
    new_head_sha: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE invocations SET head_sha = $5 \
         WHERE installation_id = $1 AND owner = $2 AND repo = $3 AND pr_number = $4 \
           AND status NOT IN ('completed', 'aborted', 'dispatch_failed', 'correlation_timeout', 'superseded')",
    )
    .bind(installation_id)
    .bind(owner)
    .bind(repo)
    .bind(pr_number)
    .bind(new_head_sha)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db;

    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query("TRUNCATE invocations")
            .execute(&pool)
            .await
            .unwrap();
        Some(pool)
    }

    fn sample(id: Uuid) -> NewInvocation<'static> {
        NewInvocation {
            id,
            installation_id: 1,
            repository_id: 100,
            owner: "acme",
            repo: "widgets",
            comment_id: 100,
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
            delivery_guid: None,
        }
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn claims_a_fresh_row() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        let outcome = claim(&pool, &sample(id)).await.unwrap();
        assert_eq!(outcome, ClaimOutcome::Claimed(id));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn claim_records_the_originating_webhook_delivery() {
        let Some(pool) = test_pool().await else {
            return;
        };
        crate::deliveries::insert_delivery(&pool, "origin-guid", "issue_comment", b"{}")
            .await
            .unwrap();
        let id = Uuid::new_v4();
        let mut invocation = sample(id);
        invocation.delivery_guid = Some("origin-guid");
        claim(&pool, &invocation).await.unwrap();

        let delivery_guid: Option<String> =
            sqlx::query_scalar("SELECT delivery_guid FROM invocations WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(delivery_guid.as_deref(), Some("origin-guid"));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_second_claim_of_the_same_key_past_claimed_is_a_duplicate() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();
        transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();

        let second_id = Uuid::new_v4();
        let outcome = claim(&pool, &sample(second_id)).await.unwrap();
        assert_eq!(outcome, ClaimOutcome::AlreadyClaimed);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_second_claim_while_still_claimed_resumes_the_same_row() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();

        let second_id = Uuid::new_v4();
        let outcome = claim(&pool, &sample(second_id)).await.unwrap();
        assert_eq!(outcome, ClaimOutcome::Resume(id));
    }

    /// Two-worker concurrency test (plan M6/Testing Strategy): two replicas
    /// racing to claim the identical `(installation_id, comment_id,
    /// attempt)` key at the same instant, not just in sequence.
    #[serial_test::serial(db)]
    #[tokio::test]
    async fn two_concurrent_claims_of_the_same_key_resolve_to_exactly_one_winner() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first = sample(first_id);
        let second = sample(second_id);

        let (a, b) = tokio::join!(claim(&pool, &first), claim(&pool, &second));
        let outcomes = [a.unwrap(), b.unwrap()];

        let claimed: Vec<_> = outcomes
            .iter()
            .filter_map(|o| match o {
                ClaimOutcome::Claimed(id) => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(
            claimed.len(),
            1,
            "exactly one concurrent claim must win the UNIQUE constraint"
        );
        let winner_id = claimed[0];

        let loser = outcomes
            .iter()
            .find(|o| !matches!(o, ClaimOutcome::Claimed(_)))
            .unwrap();
        assert_eq!(
            *loser,
            ClaimOutcome::Resume(winner_id),
            "the losing concurrent claim must resume the winner's row, never AlreadyClaimed (the winner hasn't progressed past `claimed` yet)"
        );

        let (count,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM invocations WHERE installation_id = $1 AND comment_id = $2 AND attempt = $3",
        )
        .bind(1i64)
        .bind(100i64)
        .bind(1i32)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "only one row must exist for the contested key");
    }

    /// Two-worker concurrency test (plan M6/Testing Strategy): a supersede
    /// (a newer attempt superseding this one) racing against this same
    /// invocation's own completion, both landing on the same row at once.
    #[serial_test::serial(db)]
    #[tokio::test]
    async fn a_supersede_and_a_completion_racing_on_the_same_row_produce_exactly_one_winner() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();
        transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        transition_status(&pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();

        let (completed, superseded) = tokio::join!(
            transition_status(&pool, id, InvocationStatus::Completed),
            transition_status(&pool, id, InvocationStatus::Superseded),
        );
        let completed = completed.unwrap();
        let superseded = superseded.unwrap();

        assert_ne!(
            completed, superseded,
            "exactly one of the two racing transitions must win"
        );

        let (status,): (String,) = sqlx::query_as("SELECT status FROM invocations WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, if completed { "completed" } else { "superseded" });
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn transition_status_fails_a_stale_or_out_of_order_update() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();

        // claimed -> correlated directly is not a valid transition.
        let ok = transition_status(&pool, id, InvocationStatus::Correlated)
            .await
            .unwrap();
        assert!(!ok);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn transition_status_succeeds_for_a_valid_transition() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();

        let ok = transition_status(&pool, id, InvocationStatus::Dispatched)
            .await
            .unwrap();
        assert!(ok);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn find_supersede_candidates_finds_older_non_terminal_same_triple() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let old_id = Uuid::new_v4();
        claim(&pool, &sample(old_id)).await.unwrap();

        let mut newer = sample(Uuid::new_v4());
        newer.comment_id = 200;
        let new_id = newer.id;
        claim(&pool, &newer).await.unwrap();

        let candidates = find_supersede_candidates(&pool, 1, "acme", "widgets", 7, "echo", new_id)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, old_id);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn find_supersede_candidates_excludes_terminal_invocations() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let old_id = Uuid::new_v4();
        claim(&pool, &sample(old_id)).await.unwrap();
        transition_status(&pool, old_id, InvocationStatus::Aborted)
            .await
            .unwrap();

        let mut newer = sample(Uuid::new_v4());
        newer.comment_id = 200;
        let new_id = newer.id;
        claim(&pool, &newer).await.unwrap();

        let candidates = find_supersede_candidates(&pool, 1, "acme", "widgets", 7, "echo", new_id)
            .await
            .unwrap();
        assert!(candidates.is_empty());
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn find_by_workflow_run_id_matches_exactly_on_run_id() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();
        set_workflow_run_id(&pool, id, 999).await.unwrap();

        let found = find_by_workflow_run_id(&pool, 1, "acme", "widgets", 999)
            .await
            .unwrap();
        assert_eq!(found.unwrap().id, id);

        let not_found = find_by_workflow_run_id(&pool, 1, "acme", "widgets", 1000)
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn find_by_check_run_id_matches_the_stored_check_run() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();
        set_check_run_id(&pool, id, 55).await.unwrap();

        let found = find_by_check_run_id(&pool, 1, "acme", "widgets", 55)
            .await
            .unwrap();
        assert_eq!(found.unwrap().id, id);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn find_stale_claimed_finds_only_old_claimed_rows() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();

        // Not stale yet under a generous window.
        let stale = find_stale_claimed(&pool, chrono::Duration::hours(1))
            .await
            .unwrap();
        assert!(stale.is_empty());

        // A zero-duration window makes every claimed row "stale".
        let stale = find_stale_claimed(&pool, chrono::Duration::zero())
            .await
            .unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, id);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn set_conclusion_and_last_reported_status_round_trip() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let id = Uuid::new_v4();
        claim(&pool, &sample(id)).await.unwrap();
        set_conclusion(&pool, id, "success").await.unwrap();
        set_last_reported_status(&pool, id, "completed")
            .await
            .unwrap();

        let row: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT conclusion, last_reported_status FROM invocations WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0.as_deref(), Some("success"));
        assert_eq!(row.1.as_deref(), Some("completed"));
    }
}
