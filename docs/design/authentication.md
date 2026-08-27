---
outline:
  - trust domains and identity keys
  - credentials contacts and external identities
  - sign-in and explicit linking
  - provider adapters and enterprise SSO
  - security and lifecycle rules
---

# Authentication and external identities

Status: implemented baseline (2026-08-18)

## Problem and boundary

Slash must support password credentials, social identity providers, multiple
self-hosted GitLab instances, and tenant-specific enterprise SSO without
turning email addresses or provider names into global identity keys.

Authentication proves control of a credential in one configured trust domain.
It does not grant repository command authorization, merge accounts, or infer
organization membership. GitHub remains authoritative for command access.

## Data model

The internal account and every way to authenticate it are separate:

- `users` is the Slash account and lifecycle record. It owns no login name.
- `auth_connections` identifies a configured trust domain: provider kind,
  protocol, issuer, optional tenant, and non-secret configuration.
- `user_identities` links a provider-stable subject from one connection to a
  Slash user and caches mutable display profile data.
- `password_credentials` owns the normalized email login and password hash.
- `user_emails` stores optional contact or recovery addresses and their
  independent verification state. A contact is never an identity key.

An external identity is uniquely identified by:

```text
(connection_id, subject)
```

`provider`, username, display name, and email are not sufficient. The same
subject string may legitimately exist in gitlab.com, a private GitLab, Google,
and several enterprise IdPs. A connection identifies the issuer, tenant, and
application boundary in which its subject is meaningful.

Secrets remain file-only runtime configuration. They are never stored in the
`auth_connections.configuration` JSON document.

## Provider-neutral contract

Each protocol adapter validates its provider response and produces the same
internal value:

```text
AuthenticatedIdentity {
  connection_id,
  subject,
  username,
  display_name,
  profile
}
```

Only `connection_id` and `subject` participate in account resolution. Profile
fields are mutable presentation data. In the implemented baseline, raw tokens
are used during the callback and discarded. The proposed
[command invocation history](command-invocation-history.md) design defines one
narrow, short-lived exception for repository discovery without placing a token
in identity profile data or PostgreSQL.

## Sign-in and explicit linking

Sign-in and connection share transport helpers but never share account policy.

For sign-in:

1. authenticate with one enabled connection;
2. find `(connection_id, subject)`;
3. if linked to an active user, refresh profile data and create a session;
4. if new, create a credential-free user and the identity atomically;
5. route users with no team to onboarding.

For explicit connection:

1. require a live Slash session and bind the callback to that exact user;
2. authenticate the new external identity independently;
3. reject an identity already owned by another Slash user;
4. reject replacing a different identity already linked for that connection;
5. otherwise insert or refresh the identity atomically.

Email equality never links or merges accounts. A person who has a password
account must sign in to it and explicitly connect another identity. Account
merge and recovery are separate high-risk products and are not implicit login
fallbacks.

## Password credential management

An active user may manage their own password credential from a live browser
session. A personal access token is intentionally insufficient because it is
itself a long-lived credential and must not mint another login method.

- A user who already has a password credential keeps its normalized login
  email and must prove the current password before replacing the hash.
- A user whose external identity is their only login method supplies a new,
  unique email and password. Slash creates the password credential and records
  the email as a contact in one transaction.
- Creating a password never searches for or merges another account with the
  same email. A uniqueness conflict fails the operation.
- Password changes do not revoke existing sessions. Session revocation and
  account recovery remain separate products.

## GitHub App adapter

Slash uses the same GitHub App for repository automation and user
authentication. The user authorization flow uses OAuth web-flow transport,
state, and PKCE to obtain a GitHub App user access token. GitHub App user
tokens use fine-grained App permissions rather than OAuth scopes.

The adapter reads only `GET /user`. The stable numeric GitHub user id is the
subject. Slash does not request `user:email`, call `/user/emails`, or require
GitHub email permission. GitHub login and name are display profile fields.

### Proposed repository-discovery credential

The command activity console needs the GitHub App's user access token to list
only installations and repositories accessible to both the App and the signed
in user. Its owning design permits an encrypted, HttpOnly browser cookie for no
longer than eight hours. The credential remains bound to the authenticated
Slash user and GitHub subject, is never stored in PostgreSQL or exposed to
JavaScript, and has no retained refresh token. All other provider adapters and
uses continue to discard their raw tokens at the callback boundary.

## Future providers and enterprise SSO

- Google and standards-based enterprise OIDC use the validated `iss` trust
  domain and `sub` as the subject.
- Each GitLab SaaS or self-hosted instance is a separate connection even when
  it uses the same adapter.
- QQ and WeChat adapters normalize their provider-specific stable identifier
  only after validating it in the configured application boundary.
- Each enterprise OIDC or SAML setup is a tenant-scoped connection. JIT
  provisioning and claim-to-membership mapping are tenant policy layered after
  authentication.
- SCIM synchronizes lifecycle and membership; it is not a login protocol and
  does not bypass authentication or linking rules.

Adding a provider therefore adds a protocol adapter and connection
configuration, not provider columns or branches in account persistence.

## Security and lifecycle rules

- OAuth state is signed, unpredictable, expires after ten minutes, is bound to
  an HttpOnly SameSite cookie, and includes a PKCE verifier.
- A connection callback revalidates the current Slash session and initiating
  user before persistence.
- Provider tokens, secret contents, callback codes, and upstream bodies are
  never logged or returned in browser-readable data. Provider tokens are not
  persisted except for the proposed encrypted, HttpOnly GitHub discovery
  cookie owned by the command invocation history design.
- Disabled users cannot sign in through a still-linked identity.
- Database uniqueness constraints are authoritative for concurrent sign-in and
  linking races.
- Password login performs a fixed-cost dummy verification for unknown emails.
- Invalid password credentials return the same generic `401` response, while a
  database failure is logged without the login email and returns `503`; a
  dependency outage must never be presented as a credential rejection.
- User session cookies, including the cookie that clears a session, are
  `HttpOnly`, `Secure`, `SameSite=Lax`, and scoped to `/`. Authenticated browser
  traffic therefore requires HTTPS ingress.
- Password creation and replacement require an active user and a browser
  session; replacement additionally requires the current password.
- Contact verification, account recovery, identity replacement, and account
  merge require explicit products with their own audit and reauthentication
  policy.
