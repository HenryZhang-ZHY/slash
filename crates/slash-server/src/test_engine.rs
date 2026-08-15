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

/// Creates or claims an unowned suite for a console user. A suite already
/// owned by another user is not returned and cannot be taken over.
pub async fn upsert_owned_suite(
    tx: &mut Transaction<'_, Postgres>,
    suite: &NewSuite<'_>,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO test_suites
            (id, installation_id, owner, repo, suite_key, created_by_user_id)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (installation_id, owner, repo, suite_key) DO UPDATE
            SET created_by_user_id = COALESCE(test_suites.created_by_user_id, EXCLUDED.created_by_user_id)
            WHERE test_suites.created_by_user_id IS NULL
               OR test_suites.created_by_user_id = EXCLUDED.created_by_user_id
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(suite.installation_id)
    .bind(suite.owner)
    .bind(suite.repo)
    .bind(suite.suite_key)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id,)| id))
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
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SuiteSummary {
    pub id: Uuid,
    pub suite_key: String,
    pub owner: String,
    pub repo: String,
    pub total_tests: i64,
    pub muted: i64,
    pub skipped: i64,
    pub run_count: i64,
    pub execution_count: i64,
    pub passed_executions: i64,
    pub failed_executions: i64,
    pub skipped_executions: i64,
    pub errored_executions: i64,
    pub average_duration_ms: Option<f64>,
    pub last_captured: Option<chrono::DateTime<chrono::Utc>>,
}

/// Lists suites for a tenancy, each with test counts by disposition — the data
/// the Test Engine console UI renders.
pub async fn list_suites(
    conn: &PgPool,
    installation_id: i64,
    user_id: Uuid,
) -> Result<Vec<SuiteSummary>, sqlx::Error> {
    sqlx::query_as::<_, SuiteSummary>(
        "SELECT ts.id, ts.suite_key, ts.owner, ts.repo,\n\
            count(DISTINCT t.id) FILTER (WHERE t.id IS NOT NULL) AS total_tests,\n\
            count(DISTINCT t.id) FILTER (WHERE t.state = 'muted') AS muted,\n\
            count(DISTINCT t.id) FILTER (WHERE t.state = 'skipped') AS skipped,\n\
            count(DISTINCT te.run_id) AS run_count,\n\
            count(te.id) AS execution_count,\n\
            count(te.id) FILTER (WHERE te.status = 'passed') AS passed_executions,\n\
            count(te.id) FILTER (WHERE te.status = 'failed') AS failed_executions,\n\
            count(te.id) FILTER (WHERE te.status = 'skipped') AS skipped_executions,\n\
            count(te.id) FILTER (WHERE te.status = 'errored') AS errored_executions,\n\
            avg(te.duration_ms)::float8 AS average_duration_ms,\n\
            max(te.captured_at) AS last_captured\n\
         FROM test_suites ts\n\
         LEFT JOIN tests t ON t.suite_id = ts.id\n\
         LEFT JOIN test_executions te ON te.test_id = t.id\n\
         WHERE ts.installation_id = $1 AND ts.created_by_user_id = $2\n\
         GROUP BY ts.id ORDER BY ts.suite_key",
    )
    .bind(installation_id)
    .bind(user_id)
    .fetch_all(conn)
    .await
}

/// A test row for the console read API.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TestSummary {
    pub id: Uuid,
    pub name: String,
    pub state: String,
    pub file: Option<String>,
    pub line_no: Option<i32>,
    pub labels: Vec<String>,
    pub owner_team_ids: Vec<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_status: Option<String>,
    pub last_captured: Option<chrono::DateTime<chrono::Utc>>,
    pub last_run_ref: Option<String>,
    pub last_ci_provider: Option<String>,
    pub execution_count: i64,
    pub passed_count: i64,
    pub failed_count: i64,
    pub skipped_count: i64,
    pub errored_count: i64,
    pub average_duration_ms: Option<f64>,
}

/// Lists a suite's tests with current disposition and latest execution.
pub async fn list_tests(
    conn: &PgPool,
    suite_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<TestSummary>, sqlx::Error> {
    sqlx::query_as::<_, TestSummary>(
        "SELECT t.id, t.name, t.state, t.file, t.line_no, t.labels, t.owner_team_ids,\n\
                t.created_at, t.updated_at, e.status AS last_status,\n\
                e.captured_at AS last_captured, e.run_ref AS last_run_ref,\n\
                e.ci_provider AS last_ci_provider, count(te.id) AS execution_count,\n\
                count(te.id) FILTER (WHERE te.status = 'passed') AS passed_count,\n\
                count(te.id) FILTER (WHERE te.status = 'failed') AS failed_count,\n\
                count(te.id) FILTER (WHERE te.status = 'skipped') AS skipped_count,\n\
                count(te.id) FILTER (WHERE te.status = 'errored') AS errored_count,\n\
                avg(te.duration_ms)::float8 AS average_duration_ms\n\
             FROM tests t\n\
             JOIN test_suites ts ON ts.id = t.suite_id\n\
             LEFT JOIN LATERAL (\n\
               SELECT execution.status, execution.captured_at, run.run_ref, run.ci_provider\n\
               FROM test_executions execution\n\
               JOIN test_runs run ON run.id = execution.run_id\n\
               WHERE execution.test_id = t.id\n\
               ORDER BY execution.captured_at DESC LIMIT 1\n\
             ) e ON true\n\
             LEFT JOIN test_executions te ON te.test_id = t.id\n\
             WHERE t.suite_id = $1 AND ts.created_by_user_id = $2\n\
             GROUP BY t.id, e.status, e.captured_at, e.run_ref, e.ci_provider\n\
             ORDER BY t.name",
    )
    .bind(suite_id)
    .bind(user_id)
    .fetch_all(conn)
    .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TestExecutionSummary {
    pub id: Uuid,
    pub status: String,
    pub duration_ms: i64,
    pub stack: Option<String>,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub run_id: Uuid,
    pub run_ref: String,
    pub ci_provider: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub invocation_id: Option<Uuid>,
}

pub struct TestExecutionPage {
    pub total: i64,
    pub items: Vec<TestExecutionSummary>,
}

/// Lists the latest executions for one test, scoped through its suite owner.
pub async fn list_test_executions(
    conn: &PgPool,
    test_id: Uuid,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<TestExecutionPage, sqlx::Error> {
    let total = sqlx::query_scalar(
        "SELECT count(te.id)
         FROM test_executions te
         JOIN tests t ON t.id = te.test_id
         JOIN test_suites ts ON ts.id = t.suite_id
         WHERE te.test_id = $1 AND ts.created_by_user_id = $2",
    )
    .bind(test_id)
    .bind(user_id)
    .fetch_one(conn)
    .await?;

    let items = sqlx::query_as::<_, TestExecutionSummary>(
        "SELECT te.id, te.status, te.duration_ms, te.stack, te.captured_at,
                tr.id AS run_id, tr.run_ref, tr.ci_provider, tr.started_at,
                tr.finished_at, tr.invocation_id
         FROM test_executions te
         JOIN tests t ON t.id = te.test_id
         JOIN test_suites ts ON ts.id = t.suite_id
         JOIN test_runs tr ON tr.id = te.run_id
         WHERE te.test_id = $1 AND ts.created_by_user_id = $2
         ORDER BY te.captured_at DESC
         LIMIT $3 OFFSET $4",
    )
    .bind(test_id)
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(conn)
    .await?;

    Ok(TestExecutionPage { total, items })
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

pub fn parse_test_state(state: &str) -> TestState {
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

#[derive(Debug, thiserror::Error)]
pub enum RecoverableTokenError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Crypto(#[from] slash_core::TokenCryptoError),
}

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

/// Revokes a collection token for a suite (marks it `revoked`, stopping
pub async fn issue_recoverable_collection_token(
    pool: &PgPool,
    suite_id: Uuid,
    secret: &crate::auth::AuthSecret,
) -> Result<String, RecoverableTokenError> {
    let raw = slash_core::crypto_random_token();
    let hash = slash_core::hash_token(&raw);
    let encrypted = slash_core::encrypt_collection_token(&raw, secret.0.as_bytes())?;
    sqlx::query(
        "INSERT INTO collection_tokens
            (id, suite_id, token_hash, status, token_ciphertext, token_nonce)
         VALUES ($1, $2, $3, 'active', $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(suite_id)
    .bind(&hash[..])
    .bind(encrypted.ciphertext)
    .bind(encrypted.nonce)
    .execute(pool)
    .await?;
    Ok(raw)
}

pub async fn suite_owned_by(
    pool: &PgPool,
    suite_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM test_suites WHERE id = $1 AND created_by_user_id = $2
         )",
    )
    .bind(suite_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn test_owned_by(
    pool: &PgPool,
    test_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM tests t
            JOIN test_suites ts ON ts.id = t.suite_id
            WHERE t.id = $1 AND ts.created_by_user_id = $2
         )",
    )
    .bind(test_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn latest_collection_token(
    pool: &PgPool,
    suite_id: Uuid,
    user_id: Uuid,
    secret: &crate::auth::AuthSecret,
) -> Result<Option<String>, RecoverableTokenError> {
    let encrypted: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT ct.token_ciphertext, ct.token_nonce
         FROM collection_tokens ct
         JOIN test_suites ts ON ts.id = ct.suite_id
         WHERE ct.suite_id = $1
           AND ts.created_by_user_id = $2
           AND ct.status = 'active'
           AND ct.token_ciphertext IS NOT NULL
           AND ct.token_nonce IS NOT NULL
         ORDER BY ct.created_at DESC
         LIMIT 1",
    )
    .bind(suite_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    encrypted
        .map(|(ciphertext, nonce)| {
            slash_core::decrypt_collection_token(
                &slash_core::EncryptedCollectionToken { ciphertext, nonce },
                secret.0.as_bytes(),
            )
        })
        .transpose()
        .map_err(RecoverableTokenError::from)
}

/// Resolves a presented collection token to its suite identity + tenancy, or
/// `None` if the token is unknown **or revoked**. Auth for the ingestion
/// endpoint (design §4) — fail-closed: a revoked token must not authenticate.
pub async fn find_suite_for_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<SuiteTokenIdentity>, sqlx::Error> {
    let hash = slash_core::hash_token(raw_token);
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
    let hash = slash_core::hash_token(raw_token);
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

/// Pure, dependency-free unit tests for the test-engine parsing helpers —
/// the fast safety net @Quality's P1 asks for (no DB, no wiremock).
#[cfg(test)]
mod pure_tests {
    use super::*;
    use crate::auth::AuthSecret;
    use std::sync::Arc;

    #[test]
    fn collection_token_encryption_round_trips() {
        let secret = AuthSecret(Arc::from("correct-auth-secret"));
        let encrypted =
            slash_core::encrypt_collection_token("collector-token", secret.0.as_bytes()).unwrap();

        assert_ne!(encrypted.ciphertext, b"collector-token");
        assert_eq!(
            slash_core::decrypt_collection_token(&encrypted, secret.0.as_bytes()).unwrap(),
            "collector-token"
        );
    }

    #[test]
    fn collection_token_decryption_rejects_wrong_secret() {
        let secret = AuthSecret(Arc::from("correct-auth-secret"));
        let wrong_secret = AuthSecret(Arc::from("wrong-auth-secret"));
        let encrypted =
            slash_core::encrypt_collection_token("collector-token", secret.0.as_bytes()).unwrap();

        assert!(
            slash_core::decrypt_collection_token(&encrypted, wrong_secret.0.as_bytes()).is_err()
        );
    }

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
}
