-- grants permission tiers rename: 'write|maintain|admin' -> 'read|write|admin'
-- (org/user M3 #24). `maintain` is removed (was a GitHub-collaborator-role
-- holdover); `read` is added. Pre-launch (no grant rows), so drop/re-add the
-- CHECK constraint safely; the Rust tier model in slash-config/slash-core is
-- updated in lockstep.
ALTER TABLE grants DROP CONSTRAINT IF EXISTS grants_permission_check;
ALTER TABLE grants ADD CONSTRAINT grants_permission_check
    CHECK (permission IN ('read', 'write', 'admin'));
