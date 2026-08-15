//! Postgres-backed integration tests for the Test Engine durable record
//! (docs/design/1.0-test-engine.md §3, §4, §5). These exercise the real
//! repository + flaky reconcile against a local Postgres and are gated on
//! `SLASH_TEST_DATABASE_URL` — they skip cleanly when it is absent (plan M4),
//! mirroring the unit-test `test_pool` convention.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use slash_server::auth::AuthSecret;
use slash_server::db;
use slash_server::test_engine::ExecutionStatus::{Failed, Passed};
use slash_server::test_engine::{
    ExecutionStatus, NewExecution, NewRun, NewSuite, NewTest, TestState, find_suite_for_token,
    insert_executions, issue_recoverable_collection_token, list_suites, parse_test_state,
    quarantined_tests, revoke_collection_token, set_test_state, upsert_owned_suite, upsert_run,
    upsert_suite, upsert_test,
};

/// `None` when `SLASH_TEST_DATABASE_URL` is unset — callers skip cleanly
/// (plan M4).
async fn test_pool() -> Option<PgPool> {
    let url = slash_server::test_support::test_database_url()?;
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

    let secret = AuthSecret(Arc::from("test-auth-secret"));
    let raw = issue_recoverable_collection_token(pool, suite_id, &secret)
        .await
        .unwrap();
    (suite_id, raw)
}

#[serial_test::serial(db)]
#[tokio::test]
async fn lists_only_suites_owned_by_the_user() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, display_name, status)
         VALUES ($1, $2, 'unused', 'Owner', 'active')",
    )
    .bind(user_id)
    .bind(format!("{user_id}@example.test"))
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let suite_id = upsert_owned_suite(
        &mut tx,
        &NewSuite {
            installation_id: 1,
            owner: "acme",
            repo: "widgets",
            suite_key: "web",
        },
        user_id,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();

    let suites = list_suites(&pool, 1, user_id).await.unwrap();

    assert_eq!(suites.len(), 1);
    assert_eq!(suites[0].id, suite_id);
    assert_eq!(suites[0].total_tests, 0);
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

    let transitions = slash_server::flaky::reconcile(&pool).await.unwrap();
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

    slash_server::flaky::reconcile(&pool).await.unwrap();

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
    slash_server::flaky::reconcile(&pool).await.unwrap();

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
