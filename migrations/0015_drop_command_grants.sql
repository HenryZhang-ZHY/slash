-- GitHub is the sole authorization source for slash commands. Remove the
-- disconnected Slash-local grant model and its unused installation mapping.
DROP TABLE grants;
DROP TABLE repos;
ALTER TABLE organizations DROP COLUMN installation_id;
