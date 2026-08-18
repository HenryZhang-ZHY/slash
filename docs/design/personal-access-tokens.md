# Personal access tokens

Status: implemented (2026-08-18)

## Boundary

A personal access token authenticates a REST API request as the Slash user who
created it. It does not snapshot permissions or add a new authorization model:
every handler applies the same current user, team, and repository checks used
for a browser session.

Credential management is browser-session-only. A personal access token cannot
list, create, or revoke personal access tokens, which prevents one leaked token
from minting replacement credentials.

## Token lifecycle

Users give each token a name and may choose an expiry from one to 365 days or
leave it without an expiry. The plaintext starts with `slash_pat_` and is
returned exactly once. Slash stores only an HMAC-SHA256 digest keyed by the
instance authentication secret. Rotating that secret therefore invalidates
all sessions and personal access tokens.

Revocation is immediate and retained as a timestamp for auditability. Listing
omits revoked rows but includes expired rows so users can identify stale
credentials. `last_used_at` is updated at most once every five minutes to avoid
turning high-volume agent traffic into a database write per request.

An explicit `Authorization` header takes precedence over the session cookie.
Malformed, expired, revoked, disabled-user, or digest-mismatched credentials
fail closed with `401`; Slash never falls back to a valid cookie after a bad
Bearer credential.
