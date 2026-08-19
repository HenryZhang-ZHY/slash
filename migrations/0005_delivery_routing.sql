-- Persist verified webhook routing hints so PostgreSQL can coordinate
-- installation-level concurrency across every server replica.

ALTER TABLE deliveries
    ADD COLUMN installation_id BIGINT,
    ADD COLUMN repository_id BIGINT;

CREATE INDEX deliveries_installation_processing_idx
    ON deliveries (installation_id, lease_expires_at)
    WHERE state = 'processing' AND installation_id IS NOT NULL;

CREATE INDEX deliveries_installation_pending_idx
    ON deliveries (installation_id, received_at)
    WHERE state = 'pending' AND installation_id IS NOT NULL;
