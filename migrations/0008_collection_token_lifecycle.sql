ALTER TABLE collection_tokens
    ADD COLUMN expires_at TIMESTAMPTZ NOT NULL
        DEFAULT now() + interval '90 days',
    ADD COLUMN last_used_at TIMESTAMPTZ;

WITH ranked_active AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY suite_id ORDER BY created_at DESC, id DESC
           ) AS position
    FROM collection_tokens
    WHERE status = 'active'
)
UPDATE collection_tokens AS token
SET status = 'revoked', revoked_at = now()
FROM ranked_active
WHERE token.id = ranked_active.id AND ranked_active.position > 1;

CREATE UNIQUE INDEX collection_tokens_one_active_per_suite_idx
    ON collection_tokens (suite_id) WHERE status = 'active';

CREATE INDEX collection_tokens_suite_created_idx
    ON collection_tokens (suite_id, created_at DESC);

-- Collection secrets are show-once from this release onward. Existing token
-- hashes remain valid until rotation, revocation, or expiry, but the server no
-- longer retains a recoverable copy of the credential.
ALTER TABLE collection_tokens
    DROP CONSTRAINT collection_tokens_encrypted_value_pair,
    DROP COLUMN token_ciphertext,
    DROP COLUMN token_nonce;
