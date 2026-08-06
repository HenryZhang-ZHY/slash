-- The transactional inbox (spec §7.2, §7.3). PK on the delivery GUID makes a
-- redelivery (GitHub redelivers only manually, never automatically) a no-op.
CREATE TABLE deliveries (
    delivery_guid TEXT PRIMARY KEY,
    event TEXT NOT NULL,
    payload BYTEA NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'done', 'failed')),
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT
);

-- Workers claim with `SELECT ... FOR UPDATE SKIP LOCKED`; this index keeps
-- that scan cheap as the table grows.
CREATE INDEX deliveries_pending_idx ON deliveries (received_at) WHERE state = 'pending';
