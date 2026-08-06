-- Slash pre-release schema baseline. Runtime coordination and product state
-- live in PostgreSQL; constraints and indexes below are part of correctness.

-- Durable GitHub webhook inbox.
CREATE TABLE deliveries (
    delivery_guid TEXT PRIMARY KEY,
    event TEXT NOT NULL,
    payload BYTEA NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'done', 'failed')),
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX deliveries_pending_idx
    ON deliveries (received_at) WHERE state = 'pending';

-- Durable command invocation and GitHub Actions correlation state.
CREATE TABLE invocations (
    id UUID PRIMARY KEY,
    installation_id BIGINT NOT NULL,
    repository_id BIGINT NOT NULL,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    comment_id BIGINT NOT NULL,
    attempt INT NOT NULL DEFAULT 1,
    pr_number BIGINT NOT NULL,
    head_sha TEXT NOT NULL,
    head_branch TEXT NOT NULL,
    actor TEXT NOT NULL,
    actor_id BIGINT NOT NULL,
    command TEXT NOT NULL,
    raw_comment_line TEXT NOT NULL,
    args JSONB NOT NULL DEFAULT '{}'::jsonb,
    check_run_id BIGINT,
    workflow_file TEXT NOT NULL,
    workflow_run_id BIGINT,
    status TEXT NOT NULL CHECK (status IN (
        'claimed', 'dispatched', 'correlated', 'completed',
        'aborted', 'dispatch_failed', 'correlation_timeout', 'superseded'
    )),
    conclusion TEXT,
    failure_reason TEXT,
    last_reported_status TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    dispatched_at TIMESTAMPTZ,
    correlated_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    deadline_at TIMESTAMPTZ,
    dispatch_attempts INT NOT NULL DEFAULT 0,
    CONSTRAINT invocations_comment_attempt_unique
        UNIQUE (installation_id, comment_id, attempt),
    CONSTRAINT invocations_workflow_run_unique
        UNIQUE (installation_id, owner, repo, workflow_run_id)
);

CREATE INDEX invocations_status_idx ON invocations (status);
CREATE INDEX invocations_check_run_idx
    ON invocations (installation_id, owner, repo, head_sha)
    WHERE check_run_id IS NOT NULL;

-- GitHub App installation lifecycle.
CREATE TABLE installations (
    installation_id BIGINT PRIMARY KEY,
    account TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'suspended', 'deleted')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Authentication separates the internal account, login credentials, contact
-- data, configured trust domains, and external identities. Provider subjects
-- are meaningful only inside one auth connection.
CREATE TABLE users (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'invited', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE password_credentials (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    normalized_email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE user_emails (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    normalized_email TEXT NOT NULL,
    verified_at TIMESTAMPTZ,
    verification_source TEXT,
    purpose TEXT NOT NULL DEFAULT 'contact'
        CHECK (purpose IN ('contact', 'recovery')),
    is_primary BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT user_emails_user_email_unique UNIQUE (user_id, normalized_email)
);

CREATE UNIQUE INDEX user_emails_one_primary_per_user
    ON user_emails (user_id) WHERE is_primary;

CREATE TABLE auth_connections (
    id UUID PRIMARY KEY,
    connection_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    protocol TEXT NOT NULL,
    issuer TEXT NOT NULL,
    tenant_id UUID,
    configuration JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO auth_connections
    (id, connection_key, kind, protocol, issuer)
VALUES
    ('00000000-0000-0000-0000-000000000001', 'github', 'github_app', 'oauth2', 'https://github.com');

CREATE TABLE user_identities (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES auth_connections(id),
    subject TEXT NOT NULL,
    username TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    profile JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_authenticated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT user_identities_connection_subject_unique
        UNIQUE (connection_id, subject),
    CONSTRAINT user_identities_user_connection_unique
        UNIQUE (user_id, connection_id)
);

CREATE INDEX user_identities_user_idx ON user_identities (user_id);

-- Slash-native organization and team ownership. GitHub command authorization
-- is intentionally independent of these tables.
CREATE TABLE organizations (
    id UUID PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'suspended', 'deleted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE teams (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL
        REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    slug TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    privacy TEXT NOT NULL DEFAULT 'visible'
        CHECK (privacy IN ('visible', 'secret', 'public')),
    is_default_team BOOLEAN NOT NULL DEFAULT false,
    default_member_role TEXT NOT NULL DEFAULT 'member'
        CHECK (default_member_role IN ('member', 'maintainer')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, slug)
);

CREATE TABLE team_members (
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member'
        CHECK (role IN ('member', 'maintainer')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);

CREATE TABLE org_members (
    organization_id UUID NOT NULL
        REFERENCES organizations(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member'
        CHECK (role IN ('owner', 'admin', 'member')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, user_id)
);

CREATE INDEX teams_organization_idx ON teams (organization_id);
CREATE INDEX team_members_user_idx ON team_members (user_id);
CREATE INDEX org_members_user_idx ON org_members (user_id);

-- Test Engine durable record.
CREATE TABLE test_suites (
    id UUID PRIMARY KEY,
    installation_id BIGINT NOT NULL,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    suite_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    CONSTRAINT test_suites_tenancy_unique
        UNIQUE (installation_id, owner, repo, suite_key)
);

CREATE TABLE tests (
    id UUID PRIMARY KEY,
    suite_id UUID NOT NULL REFERENCES test_suites(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    file TEXT,
    line_no INT,
    labels TEXT[] NOT NULL DEFAULT '{}',
    owner_team_ids UUID[] NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'enabled'
        CHECK (state IN ('enabled', 'muted', 'skipped')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tests_suite_name_unique UNIQUE (suite_id, name)
);

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

CREATE TABLE test_executions (
    id UUID PRIMARY KEY,
    test_id UUID NOT NULL REFERENCES tests(id) ON DELETE CASCADE,
    run_id UUID NOT NULL REFERENCES test_runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL
        CHECK (status IN ('passed', 'failed', 'skipped', 'errored')),
    duration_ms BIGINT NOT NULL DEFAULT 0,
    stack TEXT,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE collection_tokens (
    id UUID PRIMARY KEY,
    suite_id UUID NOT NULL REFERENCES test_suites(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'revoked')),
    revoked_at TIMESTAMPTZ,
    token_ciphertext BYTEA,
    token_nonce BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT collection_tokens_hash_unique UNIQUE (token_hash),
    CONSTRAINT collection_tokens_encrypted_value_pair CHECK (
        (token_ciphertext IS NULL AND token_nonce IS NULL)
        OR
        (token_ciphertext IS NOT NULL AND token_nonce IS NOT NULL
            AND octet_length(token_nonce) = 12)
    )
);

CREATE INDEX test_suites_created_by_user_idx
    ON test_suites (created_by_user_id, installation_id);
CREATE INDEX test_runs_suite_idx ON test_runs (suite_id);
CREATE INDEX test_executions_test_captured_idx
    ON test_executions (test_id, captured_at);
CREATE INDEX collection_tokens_suite_idx ON collection_tokens (suite_id);
CREATE INDEX collection_tokens_active_idx
    ON collection_tokens (suite_id) WHERE status = 'active';
