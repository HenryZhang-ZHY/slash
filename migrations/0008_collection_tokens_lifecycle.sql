-- Test Engine collection-token lifecycle (docs/test-engine.md). Adds an
-- active/revoked status so a token can
-- be rotated/revoked (the mint+revoke admin surface), while keeping the
-- existing `token_hash` unique and the sha256-hash-only storage invariant.
--
-- Migration numbering: 0008 is the Test Engine's next after 0005_test_engine
-- (0006/0007/0009 belong to the org/user lane, per the cross-lane convention).

-- Existing tokens are implicitly active; new tokens default to active (the
-- only insert path is `issue_collection_token`, which mints fresh tokens).
ALTER TABLE collection_tokens
    ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'revoked'));
ALTER TABLE collection_tokens
    ADD COLUMN revoked_at TIMESTAMPTZ;

-- The ingestion auth path looks up by hash and must reject revoked tokens
-- cheaply; this index keeps that filter fast alongside the hash lookup.
CREATE INDEX collection_tokens_active_idx
    ON collection_tokens (suite_id) WHERE status = 'active';
