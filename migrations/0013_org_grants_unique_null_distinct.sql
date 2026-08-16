-- Org/user lane (#68): the `grants_unique` constraint treats NULL
-- `repository`/`command` as distinct, so org-scope grants (both NULL) could
-- be inserted twice and the admin API's 409 (ON CONFLICT DO NOTHING) never
-- fired. PG 15+ `UNIQUE NULLS NOT DISTINCT` makes NULL compare equal, so one
-- grant per (subject, scope, repo, command) holds for org/repo/command scopes
-- alike.

ALTER TABLE grants DROP CONSTRAINT grants_unique;
ALTER TABLE grants ADD CONSTRAINT grants_unique UNIQUE NULLS NOT DISTINCT (
    organization_id, subject_type, subject_id, scope, repository, command
);
