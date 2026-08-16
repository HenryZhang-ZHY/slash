-- Organization membership with coarse roles (org/user lane, M3 #22).
--
-- De-specializes onboarding: a user who creates their org becomes an org
-- **owner**, not just the maintainer of their first team. This is the
-- standard SaaS org model (parallel to team_members), so org ownership and
-- administration are explicit and queryable instead of being implied by
-- "the first user to create a team".
--
-- role is heavily-inspired coarse (owner|admin|member), matching Buildkite's
-- coarse org roles; finer grants live in the `grants` table, not here.

CREATE TABLE org_members (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT NOT NULL DEFAULT 'member'
                    CHECK (role IN ('owner', 'admin', 'member')),
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, user_id)
);

-- Onboarding: seed the org's creator as its owner. Backfills from team
-- maintainership for orgs created before this migration: the user who holds
-- the team-maintainer role on the org's first/default team is treated as
-- the org owner (an organic backfill, not a real membership history).
INSERT INTO org_members (organization_id, user_id, role)
SELECT t.organization_id, tm.user_id, 'owner'
FROM team_members tm
JOIN teams t ON t.id = tm.team_id
WHERE tm.role = 'maintainer'
  AND NOT EXISTS (
      SELECT 1 FROM org_members om
      WHERE om.organization_id = t.organization_id
        AND om.user_id = tm.user_id
  );

CREATE INDEX org_members_user_idx ON org_members (user_id);
