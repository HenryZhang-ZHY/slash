-- Personal access tokens authenticate REST API clients as their owning user.
-- Plaintext token material is never persisted; rows retain only a keyed digest.
CREATE TABLE user_access_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    token_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    CHECK (expires_at IS NULL OR expires_at > created_at)
);

CREATE INDEX user_access_tokens_user_idx
    ON user_access_tokens (user_id, created_at DESC)
    WHERE revoked_at IS NULL;
