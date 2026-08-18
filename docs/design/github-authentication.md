# GitHub authentication and account connection

Status: accepted for implementation (2026-08-18)

## Problem

The first GitHub authentication implementation mixes three separate product
decisions inside one callback:

- whether a GitHub identity signs in an existing user or creates a new user;
- whether an email match is allowed to merge two accounts;
- whether the user should enter onboarding or the application.

That makes sign-in and account connection behave differently for accidental
reasons. In particular, an email match silently changes account ownership,
the account-connection callback trusts a user id carried in OAuth state
without revalidating the browser session, and GitHub users with a private
email receive a fabricated address.

GitHub calls this protocol the OAuth 2.0 web application flow. It is not
OpenID Connect: GitHub returns an access token and the application retrieves
the user identity from the REST API; there is no ID token.

## Product rules

Slash has two explicit GitHub flows. They share transport code but never
share account-resolution policy.

### Sign in with GitHub

1. An unauthenticated user starts GitHub sign-in from `/login`.
2. Slash completes an OAuth Authorization Code flow protected by state and
   PKCE, then retrieves the GitHub user and their verified primary email.
3. The stable numeric GitHub user id is the only automatic sign-in key.
4. If that identity is already connected, Slash creates a session for its
   user. Mutable GitHub profile fields are refreshed on the identity record.
5. If the identity is new and no Slash account uses its verified email, Slash
   creates a user and connects the identity in one transaction.
6. If an existing Slash account uses the email, Slash does not merge it.
   The user is sent back to `/login` and told to sign in with their existing
   credentials, then connect GitHub from Settings.
7. A user with at least one team enters `/`; a user with no team enters
   `/onboarding`. This destination is the single definition of onboarding
   completion and applies to every authentication method.

### Connect GitHub to an existing account

1. A signed-in user starts connection from Settings.
2. OAuth state records the `connect` intent and the initiating Slash user id.
3. The callback must have a valid current Slash session for that same user.
   Logging out, changing browser, or switching Slash accounts invalidates the
   connection attempt.
4. If the GitHub identity belongs to another Slash user, connection fails
   without changing either account.
5. If the Slash user already has a different GitHub identity, connection
   fails without replacing it. Replacing identity is a separate, explicit
   recovery product and is out of scope.
6. Connecting the same identity again is idempotent and refreshes its mutable
   profile fields.
7. Success returns to Settings. Connection never creates a new Slash session
   and never changes onboarding state.

## State and conflict table

| Intent | GitHub identity | Email / current user | Result |
| --- | --- | --- | --- |
| Sign in | connected | any email | Sign in connected user |
| Sign in | new | unused verified email | Create user and sign in |
| Sign in | new | email used by Slash user | Stop; ask for password sign-in and explicit connection |
| Connect | connected to current user | same signed-in user | Refresh identity; success |
| Connect | connected to another user | same signed-in user | Stop; identity conflict |
| Connect | new | current user has no GitHub identity | Connect; success |
| Connect | new or different | current user has another GitHub identity | Stop; account conflict |
| Connect | any | missing or different Slash session | Stop; authentication expired |

Email is profile and contact data, not an external identity key. GitHub login
and rename stability come from the numeric GitHub user id.

## Persistence and API surface

External identities live in `user_identities`, separate from Slash-owned
credentials and profile fields:

- `provider`: currently `github`;
- `provider_subject`: GitHub's stable numeric user id represented as text;
- `provider_login`: the mutable GitHub login shown in Settings;
- `provider_email`: the verified primary email observed at the last OAuth
  completion;
- `user_id`: the owning Slash user.

There is at most one identity for a provider per Slash user, and one provider
subject can belong to only one Slash user.

`GET /api/auth/me` returns the connected GitHub identity (or `null`) so the
Settings page renders actual server state. Query parameters only carry a
one-time success or error notice; they are never treated as connection state.

## Security and failure behavior

- OAuth state is unpredictable, signed, expires after ten minutes, is bound
  to an HttpOnly SameSite cookie, and is cleared after every callback.
- PKCE uses `S256`; the verifier stays inside the signed state cookie.
- The callback sends the same `redirect_uri` during token exchange.
- A connection callback revalidates the Slash session and the initiating
  user id before changing persistence.
- The GitHub access token is used only to retrieve identity data during the
  callback and is never persisted.
- Browser callbacks redirect to a stable application page with a bounded
  error code. GitHub response bodies, tokens, and internal database errors
  never reach the browser.
- Database uniqueness constraints are authoritative for races. Application
  checks exist to return a useful product conflict, not to replace them.

## Non-goals

- GitHub organization membership is not Slash authentication or
  authorization.
- GitHub email does not merge accounts.
- Disconnecting or replacing GitHub identity is not included.
- Persisting GitHub access tokens is not included.
- Generalizing the UI to multiple OAuth providers is not included, although
  the persistence model does not prevent it.
