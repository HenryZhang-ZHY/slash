ALTER TABLE tests
    ADD COLUMN state_source TEXT NOT NULL DEFAULT 'default'
        CHECK (state_source IN ('default', 'manual', 'monitor')),
    ADD COLUMN state_reason TEXT,
    ADD COLUMN state_changed_by_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN state_changed_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE TABLE test_state_events (
    id UUID PRIMARY KEY,
    test_id UUID NOT NULL REFERENCES tests(id) ON DELETE CASCADE,
    from_state TEXT NOT NULL
        CHECK (from_state IN ('enabled', 'muted', 'skipped')),
    to_state TEXT NOT NULL
        CHECK (to_state IN ('enabled', 'muted', 'skipped')),
    source TEXT NOT NULL
        CHECK (source IN ('manual', 'monitor')),
    reason TEXT,
    actor_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX test_state_events_test_created_idx
    ON test_state_events (test_id, created_at DESC);
