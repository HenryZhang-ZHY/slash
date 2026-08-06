-- Commit delivery ownership before external I/O so workers and replicas do
-- not hold a PostgreSQL connection for the complete GitHub pipeline.

ALTER TABLE deliveries
    DROP CONSTRAINT deliveries_state_check,
    ADD CONSTRAINT deliveries_state_check
        CHECK (state IN ('pending', 'processing', 'done', 'failed')),
    ADD COLUMN lease_token UUID,
    ADD COLUMN lease_expires_at TIMESTAMPTZ,
    ADD COLUMN next_attempt_at TIMESTAMPTZ,
    ADD CONSTRAINT deliveries_lease_shape_check CHECK (
        (state = 'processing' AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR
        (state <> 'processing' AND lease_token IS NULL AND lease_expires_at IS NULL)
    );

DROP INDEX deliveries_pending_idx;

CREATE INDEX deliveries_pending_idx
    ON deliveries (received_at)
    WHERE state = 'pending';

CREATE INDEX deliveries_expired_lease_idx
    ON deliveries (lease_expires_at, received_at)
    WHERE state = 'processing';
