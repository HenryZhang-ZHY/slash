-- Organization/user onboarding (org & user management lane, 1.0).

-- A user is a first-class slash account. AuthN is slash-owned (account +
-- password); GitHub identity is only a thin mapping key for webhook actors
-- (rename-stable numeric id; spec §4.2's position on slash_actor_id).
-- No GitHub-sourced authorization: org/team semantics are self-owned.
CREATE TABLE users (
    id            UUID PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,            -- login handle (B2B SaaS norm)
    password_hash TEXT NOT NULL,                   -- argon2 PHC string
    display_name  TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active', 'invited', 'disabled')),
    github_user_id BIGINT UNIQUE,                  -- nullable thin mapping key
    github_login   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- An organization is the tenant/namespace boundary (Buildkite-style: a thin
-- container that scopes teams). In slash it corresponds to one GitHub
-- installation/account. It is deliberately light: the objects a user works
-- with (teams, and later repos/commands) hang off the team, not off a
-- heavyweight org management surface.
CREATE TABLE organizations (
    id              UUID PRIMARY KEY,
    slug            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    installation_id BIGINT UNIQUE,                 -- thin GitHub install link
    state           TEXT NOT NULL DEFAULT 'active'
                    CHECK (state IN ('active', 'suspended', 'deleted')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A team is the access-control center (Buildkite-style): a group of users
-- within an organization tenant. `id` is a UUID (rename-stable, referenced
-- by Test Engine's owner_team_ids); `slug` is org-unique text identity;
-- `organization_id` scopes the team to its tenant.
CREATE TABLE teams (
    id              UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    privacy         TEXT NOT NULL DEFAULT 'visible'
                    CHECK (privacy IN ('visible', 'secret', 'public')),
    is_default_team BOOLEAN NOT NULL DEFAULT false,
    default_member_role TEXT NOT NULL DEFAULT 'member'
                    CHECK (default_member_role IN ('member', 'maintainer')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, slug)
);

-- A user's membership in a team, with a coarse role (member/maintainer).
-- PRIMARY KEY (organization_id, team_id, user_id) so a user appears once
-- per team; the granted-by pairs are explicit.
CREATE TABLE team_members (
    team_id   UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role      TEXT NOT NULL DEFAULT 'member'
              CHECK (role IN ('member', 'maintainer')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);

CREATE INDEX teams_organization_idx ON teams (organization_id);
CREATE INDEX team_members_user_idx ON team_members (user_id);
