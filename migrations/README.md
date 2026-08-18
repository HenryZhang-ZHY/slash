# Database migrations

This directory holds the ordered SQL migrations embedded and applied by
`sqlx::migrate!` in `crates/slash-server/src/db.rs`.

`0001_initial.sql` is the pre-release schema baseline. It was compacted before
Slash had a production database, so it contains only the current tables,
constraints, and indexes; discarded product models and intermediate backfills
are intentionally absent.

## Naming convention

After this baseline, migrations are numbered by the order they land on `main`.
The filename suffix carries a short topic for readability, not ownership.

Rules:
1. Use the next sequential number after the current `main` head migration. If
   two PRs are in flight, the later one must renumber before merge.
2. Never reuse or skip a number.
3. Once a migration reaches a deployed release, never edit, delete, reorder,
   or compact it. Every later schema change ships as a new migration.
4. A migration that drops or changes live data must document its deployment
   assumptions and recovery plan in the owning PR.

One global sequence keeps `sqlx migrate` deterministic across the repository.
