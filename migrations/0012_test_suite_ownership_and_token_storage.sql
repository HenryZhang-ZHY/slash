-- Scope Test Engine management to the account that created each suite and
-- retain an encrypted copy of collection tokens for authenticated UI display.
-- The existing sha256 token_hash remains the ingestion authentication index.

ALTER TABLE test_suites
    ADD COLUMN created_by_user_id UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX test_suites_created_by_user_idx
    ON test_suites (created_by_user_id, installation_id);

ALTER TABLE collection_tokens
    ADD COLUMN token_ciphertext BYTEA,
    ADD COLUMN token_nonce BYTEA,
    ADD CONSTRAINT collection_tokens_encrypted_value_pair
        CHECK (
            (token_ciphertext IS NULL AND token_nonce IS NULL)
            OR
            (token_ciphertext IS NOT NULL AND token_nonce IS NOT NULL
                AND octet_length(token_nonce) = 12)
        );