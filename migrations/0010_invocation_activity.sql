-- Keyset pagination for repository-scoped command activity. This index is
-- additive and remains compatible with old replicas during a rolling update.
CREATE INDEX invocations_repository_activity_idx
    ON invocations (installation_id, repository_id, created_at DESC, id DESC);
