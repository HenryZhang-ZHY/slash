# Instance admin console

Status: implemented in 0.5.2

## Boundary

The instance admin console is a small operational surface at `/admin`. It is
separate from Slash user, organization, and team authorization: no user role
implicitly grants instance administration.

`SLASH_ADMIN_SECRET_PATH` is the sole enablement switch. When it is absent,
the admin page and every `/api/admin/*` endpoint return `404`. When it is set,
the referenced file must be readable and non-empty or the server refuses to
start. A successful login creates a separate HttpOnly, SameSite=Strict admin
session lasting 20 minutes. Its set and clear cookies are also always
`Secure`, so the admin surface requires HTTPS ingress. Rotating the secret
invalidates existing admin sessions.

The first version is observational. It exposes installation lifecycle state,
recent durable webhook deliveries and payloads, related slash invocations,
and queue diagnostics. It does not delete or retry deliveries.

## Installation reconciliation

Webhook lifecycle events maintain the installation table continuously. An
admin may explicitly refresh it from GitHub's App installation endpoint to
recover events missed while Slash was offline. There is no timer or browser
polling. A database advisory lock permits one refresh across all replicas and
a five-minute database-backed cooldown avoids repeated upstream requests.

Every GitHub page must be fetched successfully before Slash applies the
snapshot. Applying the snapshot and its success watermark is one database
transaction, so an upstream or database failure retains the preceding known
state. The UI shows the last successful refresh time rather than implying
that an unrefreshed count is current.

An installation is an account-level GitHub App installation, not a count of
natural people. The console therefore labels personal and organization
installations separately from registered Slash users.

## Data handling

Webhook payloads are already retained in the durable inbox. The admin API
returns them only behind the admin session, and the UI renders them as escaped
JSON text. Invocation rows retain an optional origin delivery identifier;
later workflow and check deliveries are also correlated by their authenticated
GitHub identifiers for display.

Migration `0002_admin_console.sql` is append-only. Existing delivery,
installation, and invocation rows remain valid; new observational fields are
nullable or have compatibility defaults.
