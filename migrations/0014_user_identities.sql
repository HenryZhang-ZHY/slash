-- External login identities are account connections, not user profile
-- columns. Keep provider subjects rename-stable and provider-scoped.
CREATE TABLE user_identities (
    id               UUID PRIMARY KEY,
    user_id          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider         TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    provider_login   TEXT NOT NULL,
    provider_email   TEXT NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT user_identities_provider_subject_unique
        UNIQUE (provider, provider_subject),
    CONSTRAINT user_identities_user_provider_unique
        UNIQUE (user_id, provider)
);

-- Existing development links are moved once; the old ambiguous profile
-- columns are removed immediately after the new identity records exist.
INSERT INTO user_identities (
    id, user_id, provider, provider_subject, provider_login, provider_email
)
SELECT
    gen_random_uuid(),
    id,
    'github',
    github_user_id::TEXT,
    COALESCE(NULLIF(github_login, ''), github_user_id::TEXT),
    email
FROM users
WHERE github_user_id IS NOT NULL;

ALTER TABLE users
    DROP COLUMN github_user_id,
    DROP COLUMN github_login;

CREATE INDEX user_identities_user_idx ON user_identities (user_id);
