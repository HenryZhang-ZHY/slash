-- Read-only admin console support and normalized installation reconciliation.

ALTER TABLE deliveries ADD COLUMN processed_at TIMESTAMPTZ;

CREATE INDEX deliveries_received_at_idx ON deliveries (received_at DESC);

ALTER TABLE installations
    ADD COLUMN target_type TEXT NOT NULL DEFAULT 'Unknown',
    ADD COLUMN installed_at TIMESTAMPTZ,
    ADD COLUMN last_synced_at TIMESTAMPTZ;

CREATE INDEX installations_state_idx ON installations (state, updated_at DESC);

-- Singleton watermark for explicit, rate-limit-conscious GitHub App
-- installation reconciliation. Keeping it separate makes an empty snapshot
-- distinguishable from "never synchronized".
CREATE TABLE installation_sync_state (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    last_success_at TIMESTAMPTZ NOT NULL,
    installation_count BIGINT NOT NULL CHECK (installation_count >= 0)
);

ALTER TABLE invocations
    ADD COLUMN delivery_guid TEXT REFERENCES deliveries(delivery_guid) ON DELETE SET NULL;

CREATE INDEX invocations_created_at_idx ON invocations (created_at DESC);
CREATE INDEX invocations_delivery_guid_idx ON invocations (delivery_guid)
    WHERE delivery_guid IS NOT NULL;
