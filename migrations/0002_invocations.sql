-- The invocation lifecycle (spec §7.2): claimed -> dispatched -> correlated
-- -> completed, plus terminal aborted/dispatch_failed/correlation_timeout/
-- superseded. Schema lands in M4; the state machine that populates it is
-- M5/M6 — this migration only needs to exist and hold every constraint spec
-- §7.2 requires.
CREATE TABLE invocations (
    id UUID PRIMARY KEY,

    installation_id BIGINT NOT NULL,
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

    -- The §5/§7.2 idempotency key: the comment path claims attempt 1, each
    -- re-run (§6.5) claims the next attempt against the same comment.
    CONSTRAINT invocations_comment_attempt_unique
        UNIQUE (installation_id, comment_id, attempt),

    -- Correlation is exact-match by run id (§6.3); this is what makes a
    -- double claim of the same run impossible even across replicas.
    CONSTRAINT invocations_workflow_run_unique
        UNIQUE (installation_id, owner, repo, workflow_run_id)
);

CREATE INDEX invocations_status_idx ON invocations (status);
CREATE INDEX invocations_check_run_idx ON invocations (installation_id, owner, repo, head_sha)
    WHERE check_run_id IS NOT NULL;
