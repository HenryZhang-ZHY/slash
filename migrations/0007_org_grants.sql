-- Team/command permission grants (org & user management lane, M2).
--
-- Offline authorization: who (a user or a team) may drive which command or
-- repository at which permission tier. Read locally (never a live GitHub API
-- call in the authorize path). Fail-closed / deny-by-default is enforced in
-- code (slash-core grants), not by schema; the schema only stores the facts.
--
-- Historical two-tier grant semantics:
--   * a repo with no grants keeps current behavior (fall back to the GitHub
--     collaborator API is the *default fallback*, not an allow here);
--   * a repo with grants is strictly grants-only + deny-by-default, enabled
--     per-repo via an opt-in flag (grants-only repos take effect when the
--     flag is set and any grant row exists for that org+repo).
--
-- subject(user|team) -> scope(org|repository|command) -> permission tier.
-- `effect` is allow|deny, deny wins. granted_by is an audit link to users.id.

CREATE TABLE grants (
    id              UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    subject_type    TEXT NOT NULL CHECK (subject_type IN ('user', 'team')),
    subject_id      UUID NOT NULL,                 -- users.id or teams.id
    scope           TEXT NOT NULL CHECK (scope IN ('org', 'repository', 'command')),
    repository      TEXT,                          -- "owner/repo" (string, per design; no FK yet)
    command         TEXT,                          -- slash command name (scope=command)
    permission      TEXT NOT NULL CHECK (permission IN ('write', 'maintain', 'admin')),
    effect          TEXT NOT NULL DEFAULT 'allow' CHECK (effect IN ('allow', 'deny')),
    granted_by      UUID REFERENCES users(id),     -- audit: who granted it
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One grant per (subject, scope, repo, command).
    CONSTRAINT grants_unique UNIQUE (
        organization_id, subject_type, subject_id, scope, repository, command
    )
);

-- Scope-specific partial indexes keep the authorize-path lookups cheap.
CREATE INDEX grants_org_idx ON grants (organization_id);
CREATE INDEX grants_subject_idx ON grants (subject_type, subject_id);
-- Common query: team grants for a repo's command, most specific scope first.
CREATE INDEX grants_repo_cmd_idx ON grants (organization_id, repository, command)
    WHERE scope IN ('command', 'repository');
