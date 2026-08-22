ALTER TABLE test_runs
    DROP CONSTRAINT test_runs_identity_unique;

ALTER TABLE test_runs
    ADD CONSTRAINT test_runs_identity_unique
    UNIQUE (suite_id, installation_id, ci_provider, run_ref);
