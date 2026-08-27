CREATE TABLE team_invitations (
    id UUID PRIMARY KEY,
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    normalized_email TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member'
        CHECK (role IN ('member', 'maintainer')),
    token_digest BYTEA NOT NULL UNIQUE,
    invited_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    invited_by_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    accepted_by_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_sent_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX team_invitations_one_pending_email
    ON team_invitations (team_id, normalized_email)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;

CREATE INDEX team_invitations_team_pending
    ON team_invitations (team_id, created_at)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
