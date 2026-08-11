-- Test Engine durable record (docs/design/1.0-test-engine.md §3, §3.1),
-- extending the spec §7.2 state model: a durable record plus level-triggered
-- reconciliation. Test results are events from CI; slash normalizes them into
-- these tables and reconciles derived state (quarantine) from the record, never
-- from the events directly.
--
-- Schema lands in M1 (task M1-1); the flaky detector / ingestion that populate
-- it are M1-2/M1-3.

-- A named collection of tests, scoped to a tenancy + repo (design §3).
CREATE TABLE test_suites (
    id UUID PRIMARY KEY,
    installation_id BIGINT NOT NULL,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    suite_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Tenancy + suite identity (design §3.1).
    CONSTRAINT test_suites_tenancy_unique
        UNIQUE (installation_id, owner, repo, suite_key)
);

-- A single named test within a suite. `state` carries the current disposition
-- (enabled/muted/skipped); transitions are guarded compare-and-swap (design
-- §3.1). `owner_team_ids` is the `owners[].team_id` slot agreed with the
-- org/user lane (design §8 Q3): nullable set of uuids referencing that lane's
-- `team.id`, backfillable cleanly later.
CREATE TABLE tests (
    id UUID PRIMARY KEY,
    suite_id UUID NOT NULL REFERENCES test_suites(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    file TEXT,
    line_no INT,
    labels TEXT[] NOT NULL DEFAULT '{}',
    owner_team_ids UUID[] NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'enabled' CHECK (state IN ('enabled', 'muted', 'skipped')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set manually by `set_test_state` (guarded CAS), not by a trigger. No
    -- `updated_at` index in M1: the flaky reconcile sweeps `test_executions`
    -- by (test_id, captured_at), not `tests.updated_at`. Revisit (trigger +
    -- index) only if that sweep becomes a hot path (SlashLead review note).
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Suite + name identity (design §3).
    CONSTRAINT tests_suite_name_unique UNIQUE (suite_id, name)
);

-- One CI invocation that produced a batch of executions (design §3). A run may
-- be a slash-commanded GH Actions workflow run (`invocation_id` set, nullable
-- when the upload comes from a non-slash CI). `ci_provider` participates in the
-- unique key so the same `run_ref` across providers never collides (design
-- §3.1, review note).
CREATE TABLE test_runs (
    id UUID PRIMARY KEY,
    suite_id UUID NOT NULL REFERENCES test_suites(id) ON DELETE CASCADE,
    installation_id BIGINT NOT NULL,
    ci_provider TEXT NOT NULL,
    run_ref TEXT NOT NULL,
    invocation_id UUID,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    CONSTRAINT test_runs_identity_unique
        UNIQUE (installation_id, ci_provider, run_ref)
);

CREATE INDEX test_runs_suite_idx ON test_runs (suite_id);

-- One observed result of a test, append-only (design §3). Immutable once
-- recorded; retention purges after 120 days (Buildkite parity, design §3.1).
CREATE TABLE test_executions (
    id UUID PRIMARY KEY,
    test_id UUID NOT NULL REFERENCES tests(id) ON DELETE CASCADE,
    run_id UUID NOT NULL REFERENCES test_runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'skipped', 'errored')),
    duration_ms BIGINT NOT NULL DEFAULT 0,
    stack TEXT,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The flaky detection reconcile (design §5) reads per-test execution history in
-- window order; this index keeps both the window scan and the
-- fail->pass-recovery detection cheap.
CREATE INDEX test_executions_test_captured_idx
    ON test_executions (test_id, captured_at);

-- Per-suite collection tokens (design §4, §8 Q2): the ingestion endpoint is
-- authenticated by a Bearer token scoped to exactly one suite (Buildkite
-- `TEST_ENGINE_*` parity). Only the sha256 hash of the token is stored, so a
-- DB leak never leaks live tokens; the raw value is issued once and discarded.
CREATE TABLE collection_tokens (
    id UUID PRIMARY KEY,
    suite_id UUID NOT NULL REFERENCES test_suites(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL,
    -- One suite may have several tokens (rotation); each is unique to keep
    -- lookup by presented hash unambiguous.
    CONSTRAINT collection_tokens_hash_unique UNIQUE (token_hash),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX collection_tokens_suite_idx ON collection_tokens (suite_id);
