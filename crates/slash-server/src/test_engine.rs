//! Postgres repository for the Test Engine durable record (docs/design/
//! 1.0-test-engine.md §3, §3.1): test_suites, tests, test_runs,
//! test_executions. Mirrors the `invocations` module's conventions — guarded
//! compare-and-swap on `tests.state`, tenancy-scoped identities, Postgres-only
//! via sqlx.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// A test's current disposition (design §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestState {
    Enabled,
    Muted,
    Skipped,
}

impl TestState {
    pub fn as_str(self) -> &'static str {
        match self {
            TestState::Enabled => "enabled",
            TestState::Muted => "muted",
            TestState::Skipped => "skipped",
        }
    }
}

/// A single observed result status (design §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Passed,
    Failed,
    Skipped,
    Errored,
}

impl ExecutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionStatus::Passed => "passed",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Skipped => "skipped",
            ExecutionStatus::Errored => "errored",
        }
    }
}

/// A named collection of tests within a tenancy + repo (design §3).
///
/// Exercised by the integration tests (suite provisioning) and the token-mint
/// paths; `#[cfg]`-gated from the dead-code pass in the non-test build.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct NewSuite<'a> {
    pub installation_id: i64,
    pub owner: &'a str,
    pub repo: &'a str,
    pub suite_key: &'a str,
}

/// A single test within a suite.
#[derive(Debug, Clone)]
pub struct NewTest<'a> {
    /// Unique per (suite, name); on conflict the existing row is returned.
    pub name: &'a str,
    pub file: Option<&'a str>,
    pub line_no: Option<i32>,
    /// `owners[].team_id` slot (design §8 Q3): org/user lane team uuids.
    pub owner_team_ids: Vec<Uuid>,
}

/// A CI run that produced a batch of executions (design §3).
#[derive(Debug, Clone)]
pub struct NewRun<'a> {
    /// The suite this run's batch belongs to.
    pub suite_id: Uuid,
    pub installation_id: i64,
    pub ci_provider: &'a str,
    pub run_ref: &'a str,
    /// Set when run by a slash command; None for non-slash CI uploads.
    pub invocation_id: Option<Uuid>,
}

/// One observed execution result (immutable once recorded).
#[derive(Debug, Clone)]
pub struct NewExecution<'a> {
    pub test_id: Uuid,
    pub status: ExecutionStatus,
    pub duration_ms: i64,
    pub stack: Option<&'a str>,
}

/// A resolved test row alongside the run id, so a batch can map execution ->
/// (test_id, run_id) without a second lookup. Carries only `id` — the
/// disposition is resolved separately by the flaky reconcile via the cursor
/// sweep (`all_tests_page`); the ingestion path needs nothing but the id.
pub struct TestRef {
    pub id: Uuid,
}

/// Resolves (creating if absent) the suite for a tenancy + repo + suite_key.
/// Returns the suite id, whether it was newly created, and the created/again
/// existing id. Exercised by the integration tests; the ingestion path
/// resolves the suite from the collection token instead, so it is
/// `#[cfg]`-gated from the dead-code pass in the non-test build.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn upsert_suite(
    tx: &mut Transaction<'_, Postgres>,
    suite: &NewSuite<'_>,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query(
        "INSERT INTO test_suites (id, installation_id, owner, repo, suite_key)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (installation_id, owner, repo, suite_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(suite.installation_id)
    .bind(suite.owner)
    .bind(suite.repo)
    .bind(suite.suite_key)
    .execute(&mut **tx)
    .await?;

    // The conflict clause suppressed the insert if the row existed; always
    // read back the canonical id for this tenancy key.
    let (id,): (Uuid,) = sqlx::query_as(
        "SELECT id FROM test_suites \
         WHERE installation_id = $1 AND owner = $2 AND repo = $3 AND suite_key = $4",
    )
    .bind(suite.installation_id)
    .bind(suite.owner)
    .bind(suite.repo)
    .bind(suite.suite_key)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Resolves (creating if absent) a test within a suite, returning its id.
///
/// **First-writer-wins on metadata.** Test identity is `(suite_id, name)`;
/// on a conflict (test re-discovered) `file`, `line_no`, and `owner_team_ids`
/// are intentionally **not** updated — the first recorded metadata wins, so a
/// transient re-upload with stale/empty metadata never clobbers the original.
/// State (`tests.state`) is only ever changed via `set_test_state`, never here.
pub async fn upsert_test(
    tx: &mut Transaction<'_, Postgres>,
    suite_id: Uuid,
    test: &NewTest<'_>,
) -> Result<TestRef, sqlx::Error> {
    sqlx::query(
        "INSERT INTO tests (id, suite_id, name, file, line_no, owner_team_ids)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (suite_id, name) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(suite_id)
    .bind(test.name)
    .bind(test.file)
    .bind(test.line_no)
    .bind(&test.owner_team_ids)
    .execute(&mut **tx)
    .await?;

    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM tests WHERE suite_id = $1 AND name = $2")
        .bind(suite_id)
        .bind(test.name)
        .fetch_one(&mut **tx)
        .await?;

    Ok(TestRef { id })
}

/// Inserts a run, returning its id. A conflict on `(installation_id,
/// ci_provider, run_ref)` means a duplicate upload; the existing run id is
/// returned so re-uploads append to (or are idempotent against) the same run.
pub async fn upsert_run(
    tx: &mut Transaction<'_, Postgres>,
    run: &NewRun<'_>,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query(
        "INSERT INTO test_runs (id, suite_id, installation_id, ci_provider, run_ref, invocation_id)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (installation_id, ci_provider, run_ref) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(run.suite_id)
    .bind(run.installation_id)
    .bind(run.ci_provider)
    .bind(run.run_ref)
    .bind(run.invocation_id)
    .execute(&mut **tx)
    .await?;

    // Read back the canonical run id for the identity key.
    let (id,): (Uuid,) = sqlx::query_as(
        "SELECT id FROM test_runs \
         WHERE installation_id = $1 AND ci_provider = $2 AND run_ref = $3",
    )
    .bind(run.installation_id)
    .bind(run.ci_provider)
    .bind(run.run_ref)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Appends executions for a run (design §3: immutable once recorded).
pub async fn insert_executions(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    executions: &[NewExecution<'_>],
) -> Result<(), sqlx::Error> {
    for exec in executions {
        sqlx::query(
            "INSERT INTO test_executions (id, test_id, run_id, status, duration_ms, stack)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(exec.test_id)
        .bind(run_id)
        .bind(exec.status.as_str())
        .bind(exec.duration_ms)
        .bind(exec.stack)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Guarded CAS transition on a test's disposition (design §3.1): the update
/// only applies from the states in `from`. Returns whether a row was changed
/// (false = another writer moved it first, caller treats as stale/no-op).
pub async fn set_test_state(
    conn: &PgPool,
    test_id: Uuid,
    from: &[TestState],
    to: TestState,
) -> Result<bool, sqlx::Error> {
    let sql = format!(
        "UPDATE tests SET state = $2, updated_at = now() WHERE id = $1 AND state IN ({})",
        // `from` placeholders start at $3: $1 is the test id, $2 the new
        // state, and each permitted predecessor state is bound after it.
        placeholders(3, from.len())
    );
    let mut query = sqlx::query(&sql).bind(test_id).bind(to.as_str());
    for s in from {
        query = query.bind(s.as_str());
    }
    let result = query.execute(conn).await?;
    Ok(result.rows_affected() > 0)
}

/// Returns the ids of all tests currently `enabled` (candidates for flaky
/// detection) and all currently `muted` (candidates for un-quarantine).
/// Cursor page size for the flaky-reconcile sweep (M2-6).
pub const RECONCILE_PAGE_SIZE: i64 = 256;

/// Returns one keyset page of `(id, state)` for the flaky sweep, ordered by
/// `id`, picking up strictly after `after_id`. A cursor sweep (`after_id`
/// advancing across pages) replaces the full-table scan of `all_tests` so the
/// reconcile stays bounded on memory as the `tests` table grows and is
/// naturally tenancy-neutral to batch (SlashLead review note, design §5).
///
/// Pass `None` for the first page; stop when a page is shorter than the page
/// size (or empty).
pub async fn all_tests_page(
    conn: &PgPool,
    after_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<(Uuid, TestState)>, sqlx::Error> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM tests \
         WHERE ($1::uuid IS NULL OR id > $1) \
         ORDER BY id LIMIT $2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, state)| (id, parse_test_state(&state)))
        .collect())
}

/// Returns the names of tests in a suite currently quarantined (`muted` or
/// `skipped`). Backs the M1 disposal hook (design §5): a slash-commanded test
/// workflow queries this before running to skip/soft-fail already-quarantined
/// tests — the bktec client "skip/mute flaky" behavior, server-side.
pub async fn quarantined_tests(conn: &PgPool, suite_id: Uuid) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM tests \
         WHERE suite_id = $1 AND state IN ('muted', 'skipped') \
         ORDER BY name",
    )
    .bind(suite_id)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(name,)| name).collect())
}

/// A suite row for the console read API (§6 M2 / UI).
#[derive(Debug, Clone)]
pub struct SuiteSummary {
    pub id: Uuid,
    pub suite_key: String,
    pub owner: String,
    pub repo: String,
    pub total_tests: i64,
    pub muted: i64,
    pub skipped: i64,
}

/// Lists suites for a tenancy, each with test counts by disposition — the data
/// the Test Engine console UI renders.
pub async fn list_suites(
    conn: &PgPool,
    installation_id: i64,
) -> Result<Vec<SuiteSummary>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT ts.id, ts.suite_key, ts.owner, ts.repo,\n\
                count(t.id)::int8 FILTER (WHERE t.id IS NOT NULL) AS total,\n\
                count(t.id) FILTER (WHERE t.state = 'muted')::int8 AS muted,\n\
                count(t.id) FILTER (WHERE t.state = 'skipped')::int8 AS skipped\n\
         FROM test_suites ts\n\
         LEFT JOIN tests t ON t.suite_id = ts.id\n\
         WHERE ts.installation_id = $1\n\
         GROUP BY ts.id ORDER BY ts.suite_key",
    )
    .bind(installation_id)
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, suite_key, owner, repo, total_tests, muted, skipped)| SuiteSummary {
                id,
                suite_key,
                owner,
                repo,
                total_tests,
                muted,
                skipped,
            },
        )
        .collect())
}

/// A test row for the console read API.
#[derive(Debug, Clone)]
pub struct TestSummary {
    pub id: Uuid,
    pub name: String,
    pub state: String,
    pub last_status: Option<String>,
    pub last_captured: Option<chrono::DateTime<chrono::Utc>>,
}

/// Row type for `list_tests` (a suite's test + latest execution).
type TestRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// Lists a suite's tests with current disposition and latest execution.
pub async fn list_tests(conn: &PgPool, suite_id: Uuid) -> Result<Vec<TestSummary>, sqlx::Error> {
    let rows: Vec<TestRow> = sqlx::query_as(
        "SELECT t.id, t.name, t.state, e.status, e.captured_at\n\
             FROM tests t\n\
             LEFT JOIN LATERAL (\n\
               SELECT status, captured_at FROM test_executions\n\
               WHERE test_id = t.id ORDER BY captured_at DESC LIMIT 1\n\
             ) e ON true\n\
             WHERE t.suite_id = $1 ORDER BY t.name",
    )
    .bind(suite_id)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, state, last_status, last_captured)| TestSummary {
                id,
                name,
                state,
                last_status,
                last_captured,
            },
        )
        .collect())
}

/// Returns the observed execution statuses for a test within the last `window`
/// seconds, oldest first. Smallest surface the flaky detector needs: the
/// criterion is purely about the presence of a fail-then-pass recovery over a
/// denominator within the window (design §5).
pub async fn recent_executions(
    conn: &PgPool,
    test_id: Uuid,
    window_seconds: i64,
) -> Result<Vec<ExecutionStatus>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT status FROM test_executions \
         WHERE test_id = $1 AND captured_at >= now() - make_interval(secs => $2) \
         ORDER BY captured_at",
    )
    .bind(test_id)
    .bind(window_seconds)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(status,)| parse_execution_status(&status))
        .collect())
}

fn placeholders(offset: usize, count: usize) -> String {
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("${}", offset + i));
    }
    out
}

fn parse_test_state(state: &str) -> TestState {
    match state {
        "enabled" => TestState::Enabled,
        "muted" => TestState::Muted,
        "skipped" => TestState::Skipped,
        _ => TestState::Enabled,
    }
}

fn parse_execution_status(status: &str) -> ExecutionStatus {
    match status {
        "passed" => ExecutionStatus::Passed,
        "failed" => ExecutionStatus::Failed,
        "skipped" => ExecutionStatus::Skipped,
        "errored" => ExecutionStatus::Errored,
        _ => ExecutionStatus::Errored,
    }
}

// --- collection tokens (design §4) ---

/// Tenancy resolved from a collection token's suite.
#[derive(Debug, Clone)]
pub struct SuiteTokenIdentity {
    pub suite_id: Uuid,
    pub suite_key: String,
    pub installation_id: i64,
    /// Read by the integration tests; unused by the ingestion path, so these
    /// fields are `#[cfg]`-gated from the dead-code pass in the non-test build.
    #[cfg_attr(not(test), allow(dead_code))]
    pub owner: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub repo: String,
}

/// Hashes a raw token with sha256 for storage / lookup. Returns the raw byte
/// hash.
pub fn hash_token(token: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

/// Issues a new collection token scoped to a suite, storing only its sha256
/// hash. Returns the raw token exactly once; the caller is responsible for
/// handing it to the collector.
///
/// The mint path is exercised by the integration tests in M1 (suite
/// provisioning); the actual token-issuance admin surface lands in M2, so it
/// Issues a new collection token scoped to a suite, storing only its sha256
/// hash. Returns the raw token exactly once; the caller is responsible for
/// handing it to the collector. Backs the M2-4 token-management surface.
pub async fn issue_collection_token(pool: &PgPool, suite_id: Uuid) -> Result<String, sqlx::Error> {
    let raw = crypto_random_token();
    let hash = hash_token(&raw);
    sqlx::query(
        "INSERT INTO collection_tokens (id, suite_id, token_hash, status)
         VALUES ($1, $2, $3, 'active')
         ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(suite_id)
    .bind(&hash[..])
    .execute(pool)
    .await?;
    Ok(raw)
}

/// Resolves a presented collection token to its suite identity + tenancy, or
/// `None` if the token is unknown **or revoked**. Auth for the ingestion
/// endpoint (design §4) — fail-closed: a revoked token must not authenticate.
pub async fn find_suite_for_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<SuiteTokenIdentity>, sqlx::Error> {
    let hash = hash_token(raw_token);
    let row: Option<(Uuid, String, i64, String, String)> = sqlx::query_as(
        "SELECT ts.id, ts.suite_key, ts.installation_id, ts.owner, ts.repo
         FROM collection_tokens ct\n         JOIN test_suites ts ON ts.id = ct.suite_id
         WHERE ct.token_hash = $1 AND ct.status = 'active'",
    )
    .bind(&hash[..])
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(suite_id, suite_key, installation_id, owner, repo)| SuiteTokenIdentity {
            suite_id,
            suite_key,
            installation_id,
            owner,
            repo,
        },
    ))
}

/// Revokes a collection token for a suite (marks it `revoked`, stopping
/// `find_suite_for_token` from accepting it). Returns whether a matching
/// active token was revoked. `None` result = unknown or already non-active
/// token. Revocation is idempotent and only touches this suite.
pub async fn revoke_collection_token(
    pool: &PgPool,
    suite_id: Uuid,
    raw_token: &str,
) -> Result<bool, sqlx::Error> {
    let hash = hash_token(raw_token);
    let result = sqlx::query(
        "UPDATE collection_tokens SET status = 'revoked', revoked_at = now() \
         WHERE suite_id = $1 AND token_hash = $2 AND status = 'active'",
    )
    .bind(suite_id)
    .bind(&hash[..])
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Generates a cryptographically random, URL-safe token. Backs
/// `issue_collection_token` (M2-4 token management).
pub fn crypto_random_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db;
    use crate::test_engine::ExecutionStatus::{Failed, Passed};

    /// `None` when `SLASH_TEST_DATABASE_URL` is unset — callers skip cleanly
    /// (plan M4).
    async fn test_pool() -> Option<PgPool> {
        let url = crate::test_support::test_database_url()?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        sqlx::query(
            "TRUNCATE test_executions, test_runs, tests, test_suites, collection_tokens CASCADE",
        )
        .execute(&pool)
        .await
        .unwrap();
        Some(pool)
    }

    /// Provisions a suite + a collection token, returning (suite_id, raw_token).
    async fn provision_suite(pool: &PgPool) -> (Uuid, String) {
        let mut tx = pool.begin().await.unwrap();
        let suite_id = upsert_suite(
            &mut tx,
            &NewSuite {
                installation_id: 1,
                owner: "acme",
                repo: "widgets",
                suite_key: "wire",
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let raw = issue_collection_token(pool, suite_id).await.unwrap();
        (suite_id, raw)
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn provisioned_token_resolves_to_the_suite_identity() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, raw) = provision_suite(&pool).await;
        let _ = suite_id;

        let identity = find_suite_for_token(&pool, &raw).await.unwrap();
        let identity = identity.expect("token should resolve");
        assert_eq!(identity.suite_key, "wire");
        assert_eq!(identity.installation_id, 1);
        assert_eq!(identity.owner, "acme");
        assert_eq!(identity.repo, "widgets");

        let unknown = find_suite_for_token(&pool, "definitely-not-a-token")
            .await
            .unwrap();
        assert!(unknown.is_none());
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn revoked_token_no_longer_authenticates() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, raw) = provision_suite(&pool).await;

        // Active token resolves.
        assert!(find_suite_for_token(&pool, &raw).await.unwrap().is_some());

        // Revoke: applies and the token no longer authenticates (fail-closed).
        let revoked = revoke_collection_token(&pool, suite_id, &raw)
            .await
            .unwrap();
        assert!(revoked);
        assert!(find_suite_for_token(&pool, &raw).await.unwrap().is_none());

        // Revoking again is a no-op (already non-active).
        let again = revoke_collection_token(&pool, suite_id, &raw)
            .await
            .unwrap();
        assert!(!again);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn ingestion_writes_suite_test_run_and_executions_durably() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, _raw) = provision_suite(&pool).await;

        let mut tx = pool.begin().await.unwrap();
        let test_a = upsert_test(
            &mut tx,
            suite_id,
            &NewTest {
                name: "tests::it_works",
                file: Some("tests/foo.rs"),
                line_no: Some(3),
                owner_team_ids: vec![],
            },
        )
        .await
        .unwrap();
        let run_id = upsert_run(
            &mut tx,
            &NewRun {
                suite_id,
                installation_id: 1,
                ci_provider: "github_actions",
                run_ref: "run-1",
                invocation_id: None,
            },
        )
        .await
        .unwrap();
        insert_executions(
            &mut tx,
            run_id,
            &[NewExecution {
                test_id: test_a.id,
                status: ExecutionStatus::Passed,
                duration_ms: 12,
                stack: None,
            }],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let (exec_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM test_executions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(exec_count, 1);

        let (run_count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM test_runs WHERE run_ref = 'run-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(run_count, 1);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn duplicate_run_ref_is_a_single_run() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, _raw) = provision_suite(&pool).await;

        let mut tx = pool.begin().await.unwrap();
        let run = NewRun {
            suite_id,
            installation_id: 1,
            ci_provider: "github_actions",
            run_ref: "run-x",
            invocation_id: None,
        };
        let first = upsert_run(&mut tx, &run).await.unwrap();
        let second = upsert_run(&mut tx, &run).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(first, second);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn flaky_reconcile_mutes_then_recovers_a_test() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, _raw) = provision_suite(&pool).await;

        // Seed: test "tests::wobbly" gets one failing run then a passing run,
        // so across >=3 executions there is a fail->pass recovery.
        seed_runs(&pool, suite_id, "tests::wobbly", &[Failed, Passed, Passed]).await;
        // A healthy control must NOT be muted.
        seed_runs(&pool, suite_id, "tests::steady", &[Passed, Passed, Passed]).await;

        let transitions = crate::flaky::reconcile(&pool).await.unwrap();
        assert!(transitions >= 1);

        let state = state_of(&pool, suite_id, "tests::wobbly").await;
        assert_eq!(state, Some(TestState::Muted));
        let steady = state_of(&pool, suite_id, "tests::steady").await;
        assert_eq!(steady, Some(TestState::Enabled));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn flaky_reconcile_leaves_sub_threshold_tests_alone() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, _raw) = provision_suite(&pool).await;

        // Only 2 executions, one fail then pass — below the denominator.
        seed_runs(&pool, suite_id, "tests::edge", &[Failed, Passed]).await;

        crate::flaky::reconcile(&pool).await.unwrap();

        let state = state_of(&pool, suite_id, "tests::edge").await;
        assert_eq!(state, Some(TestState::Enabled));
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn direct_set_state_is_a_guarded_cas() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, _raw) = provision_suite(&pool).await;
        let mut tx = pool.begin().await.unwrap();
        let test = upsert_test(
            &mut tx,
            suite_id,
            &NewTest {
                name: "tests::cas",
                file: None,
                line_no: None,
                owner_team_ids: vec![],
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        // enabled -> muted succeeds.
        let ok = set_test_state(&pool, test.id, &[TestState::Enabled], TestState::Muted)
            .await
            .unwrap();
        assert!(ok);
        // muted -> muted from [Enabled] fails (guarded).
        let stale = set_test_state(&pool, test.id, &[TestState::Enabled], TestState::Skipped)
            .await
            .unwrap();
        assert!(!stale);
        // muted -> enabled succeeds.
        let recovered = set_test_state(&pool, test.id, &[TestState::Muted], TestState::Enabled)
            .await
            .unwrap();
        assert!(recovered);
    }

    #[serial_test::serial(db)]
    #[tokio::test]
    async fn closed_loop_disposal_hook_reports_the_quarantined_test() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (suite_id, _raw) = provision_suite(&pool).await;

        // Ingest a flaky test (fail then passes, >=3 executions) and a healthy
        // one, then run the reconcile — the closed loop: ingest -> flaky-mark.
        seed_runs(
            &pool,
            suite_id,
            "tests::flaky_one",
            &[Failed, Passed, Passed],
        )
        .await;
        seed_runs(&pool, suite_id, "tests::healthy", &[Passed, Passed, Passed]).await;
        crate::flaky::reconcile(&pool).await.unwrap();

        // The disposal hook (bktec-style skip/mute): query quarantined tests.
        let quarantined = quarantined_tests(&pool, suite_id).await.unwrap();
        assert!(quarantined.contains(&"tests::flaky_one".to_string()));
        assert!(!quarantined.contains(&"tests::healthy".to_string()));
    }

    // --- helpers ---

    /// Seeds `statuses` of executions for a named test/suite across distinct
    /// runs, each at a slightly later `captured_at`, so the flaky criterion
    /// sees them in window order.
    async fn seed_runs(pool: &PgPool, suite_id: Uuid, name: &str, statuses: &[ExecutionStatus]) {
        let mut tx = pool.begin().await.unwrap();
        let test = upsert_test(
            &mut tx,
            suite_id,
            &NewTest {
                name,
                file: None,
                line_no: None,
                owner_team_ids: vec![],
            },
        )
        .await
        .unwrap();

        for (i, status) in statuses.iter().enumerate() {
            let run_ref = format!("seed-{name}-{i}");
            let run_id = upsert_run(
                &mut tx,
                &NewRun {
                    suite_id,
                    installation_id: 1,
                    ci_provider: "seed",
                    run_ref: &run_ref,
                    invocation_id: None,
                },
            )
            .await
            .unwrap();
            insert_executions(
                &mut tx,
                run_id,
                &[NewExecution {
                    test_id: test.id,
                    status: *status,
                    duration_ms: 5,
                    stack: None,
                }],
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    async fn state_of(pool: &PgPool, suite_id: Uuid, name: &str) -> Option<TestState> {
        let (state,): (String,) =
            sqlx::query_as("SELECT state FROM tests WHERE suite_id = $1 AND name = $2")
                .bind(suite_id)
                .bind(name)
                .fetch_one(pool)
                .await
                .ok()?;
        Some(parse_test_state(&state))
    }
}

/// Pure, dependency-free unit tests for the test-engine parsing helpers —
/// the fast safety net @Quality's P1 asks for (no DB, no wiremock).
#[cfg(test)]
mod pure_tests {
    use super::*;

    #[test]
    fn parse_test_state_maps_each_label_and_defaults_unknown_to_enabled() {
        assert_eq!(parse_test_state("enabled"), TestState::Enabled);
        assert_eq!(parse_test_state("muted"), TestState::Muted);
        assert_eq!(parse_test_state("skipped"), TestState::Skipped);
        // Unknown / casing drift must fail safe to the default disposition.
        assert_eq!(parse_test_state(""), TestState::Enabled);
        assert_eq!(parse_test_state("ENABLED"), TestState::Enabled);
    }

    #[test]
    fn parse_execution_status_maps_every_known_status_and_ignores_unknown() {
        assert_eq!(parse_execution_status("passed"), ExecutionStatus::Passed);
        assert_eq!(parse_execution_status("failed"), ExecutionStatus::Failed);
        assert_eq!(parse_execution_status("skipped"), ExecutionStatus::Skipped);
        assert_eq!(parse_execution_status("errored"), ExecutionStatus::Errored);
        assert_eq!(parse_execution_status("flaky?"), ExecutionStatus::Errored);
    }

    #[test]
    fn test_state_as_str_round_trips() {
        for state in [TestState::Enabled, TestState::Muted, TestState::Skipped] {
            assert_eq!(parse_test_state(state.as_str()), state);
        }
    }

    #[test]
    fn set_test_state_placeholder_numbers_are_offset_for_id_and_to() {
        // Guarded CAS for a single `from` state: $1 id, $2 new-state, then the
        // predecessor states at $3+.
        assert_eq!(placeholders(3, 1), "$3");
        assert_eq!(placeholders(3, 2), "$3, $4");
    }

    #[test]
    fn hash_token_is_deterministic_and_32_bytes() {
        let a = hash_token("suite-token");
        let b = hash_token("suite-token");
        let c = hash_token("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn crypto_random_token_is_unique_and_parseable_uuid() {
        let a = crypto_random_token();
        let b = crypto_random_token();
        assert_ne!(a, b);
        assert!(uuid::Uuid::parse_str(&a).is_ok());
    }
}
