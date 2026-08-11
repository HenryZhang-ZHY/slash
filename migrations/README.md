# Database migrations

This directory holds the ordered SQL migrations applied by `sqlx migrate`
(`crates/slash-server/src/db.rs`). The numbers are a **single, global,
increasing sequence** shared by every lane (org/user, test-engine):

- `0001`–`0004` — 0.0.1 control plane.
- `0005_test_engine.sql` — Test Engine.
- `0006_org_users.sql` — org/user onboarding (users/organizations/teams/team_members).
- `0007_org_grants.sql` — org/user grants (subject→scope→permission tier).

## Naming convention

Migrations are numbered by **the order they land on `main`**, not claimed
up-front per lane. Each lane, when adding a table/schema change, uses the
next free number at the time it is merged. The `.sql` filename suffix carries
a short topic (`_test_engine`, `_org_users`, `_org_grants`) purely for
readability — it is not an ownership marker.

Rules:
1. In a PR, use the next sequential number after the current `main` head
   migration. If two PRs are in flight, coordinate the number so neither
   collides — the lower-merged PR keeps its number, the other bumps.
2. Never reuse or skip a number.
3. Changes to an **already-merged** migration are not allowed going forward;
   a schema change that would alter a landed table ships as a new migration.
4. Additive migrations should never drop/alter columns that other lanes'
   merged code depends on without cross-lane sign-off.

This convention is deliberately simple: one global counter, applied in merge
order. It keeps `sqlx migrate` deterministic and avoids per-lane prefixes.
