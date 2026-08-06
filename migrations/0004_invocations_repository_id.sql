-- The sweeper (plan M6) needs to mint a per-repository installation token
-- (spec §7.5) for invocations it finds outside of any active webhook event,
-- where the numeric repository id isn't otherwise available. `owner`/`repo`
-- alone aren't enough to scope a token mint.
ALTER TABLE invocations ADD COLUMN repository_id BIGINT NOT NULL DEFAULT 0;
ALTER TABLE invocations ALTER COLUMN repository_id DROP DEFAULT;
