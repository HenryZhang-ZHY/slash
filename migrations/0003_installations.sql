-- Installation state (spec §7.2, §7.5), updated by `installation*` webhooks
-- and by the 401 path, so the signal does not depend on a webhook having
-- been delivered at all.
CREATE TABLE installations (
    installation_id BIGINT PRIMARY KEY,
    account TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'suspended', 'deleted')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
