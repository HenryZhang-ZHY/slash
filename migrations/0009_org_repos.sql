-- Repositories belonging to an organization (org/user management lane, M3).
--
-- Grants M2-4 currently resolves a repo's org via `organizations.installation_id`
-- (a temporary stand-in). This table is the proper repo->org association: it
-- scopes repository-level grants and (later) the pipeline's org resolution.
--
-- A repo lives under exactly one org tenant. `owner/name` is unique per org;
-- `installation_id` is the GitHub installation the repo is reachable under.

CREATE TABLE repos (
    id              UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    installation_id BIGINT NOT NULL,
    owner           TEXT NOT NULL,   -- GitHub owner login, e.g. "acme"
    name            TEXT NOT NULL,   -- GitHub repo name, e.g. "widgets"
    state           TEXT NOT NULL DEFAULT 'active'
                    CHECK (state IN ('active', 'removed', 'suspended')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT repos_org_name_unique UNIQUE (organization_id, owner, name),
    CONSTRAINT repos_name_unique UNIQUE (owner, name)
);

-- Grant scope may reference a repo; add the column additively so existing
-- org/repo/command grants keep working. Nullable: org-scope grants don't set it.
ALTER TABLE grants ADD COLUMN repository_id UUID NULL REFERENCES repos(id) ON DELETE CASCADE;

CREATE INDEX repos_org_idx ON repos (organization_id);
CREATE INDEX repos_name_idx ON repos (owner, name);
CREATE INDEX grants_repo_id_idx ON grants (repository_id)
    WHERE repository_id IS NOT NULL;
