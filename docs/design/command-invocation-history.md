---
outline:
  - product and authorization boundary
  - GitHub repository discovery
  - history API and pagination
  - Web console behavior
  - security failure and retention rules
---

# Command invocation history

Status: implemented (2026-08-27)

## Problem and product boundary

Slash already keeps a durable `invocations` row for every configured command
that passes parsing, repository authorization, and argument binding. Repository
users can inspect an individual check or workflow run on GitHub, and instance
administrators can inspect recent invocation rows at `/admin`, but a normal
Slash user cannot browse those rows in the Web console.

The user-facing console adds **Activity** at `/activity`. It is a repository
activity view over the existing invocation lifecycle, not an instance audit
log and not a second command-execution system.

An activity row begins when Slash claims an invocation. Consequently, the
first version includes accepted invocations in every lifecycle state but does
not include malformed comments, unknown commands, `/slash help`, permission
denials, configuration failures, or argument-binding failures. Those paths do
not create invocation rows today. Recording rejected attempts would require a
separate security-audit product with its own retention and abuse controls.

No special visibility comes from being the person who installed the GitHub
App. An installation may belong to an organization, and installation ownership
does not prove current access to every repository in it.

## Authorization boundary

GitHub remains authoritative for repository access. Reading command activity
requires all of the following:

1. an active Slash browser session;
2. a GitHub identity explicitly linked to that Slash user; and
3. current GitHub collaborator access of at least `read` to the selected
   repository.

The third check applies to both private and public repositories. Public
visibility alone does not expose a repository's Slash activity to every signed
in GitHub user. Slash-native organizations, teams, and the invocation actor id
do not grant access.

Every history request performs the collaborator check against GitHub before
returning rows. The server mints an installation token scoped to the requested
`repository_id`, calls the same collaborator-permission API used by command
authorization, and fails closed on an unknown role or upstream failure. A
request uses `(installation_id, repository_id)` as its database tenant key;
`owner` and `repo` are current GitHub routing names and never replace the
stable ids in the SQL predicate.

Missing repositories and repositories the viewer cannot access both return
`404`. The API must not reveal which installation, repository, command, actor,
or invocation ids exist before authorization succeeds.

## Repository discovery

The console needs to discover repositories before it can ask for one
repository's history. App installation tokens cannot do this safely because
they describe everything the App can access, not the intersection with one
user's access.

GitHub App user access tokens provide that intersection. GitHub exposes the
installations accessible to the authenticated user and the repositories that
user can access within an installation. Slash uses these endpoints only for
the repository selectors:

- `GET /api/github/installations`
- `GET /api/github/installations/{installation_id}/repositories`

Both endpoints normalize GitHub pagination into an opaque `next_cursor` and
return only ids, account or repository names, visibility, and selector labels.
They do not persist a repository-access snapshot.

### Short-lived GitHub credential

The existing GitHub sign-in and connection callbacks already receive a user
access token and validate it with `GET /user`. For repository discovery, Slash
retains only that access token in a separate encrypted browser cookie:

- the cookie is `HttpOnly`, `Secure`, `SameSite=Lax`, and scoped to
  `/api/github`;
- its authenticated plaintext binds the token to the Slash user id and stable
  GitHub subject that completed the callback;
- its lifetime is the shorter of GitHub's reported access-token lifetime and
  eight hours;
- the encryption key is domain-separated from the existing file-backed Slash
  authentication secret, so the feature introduces no second runtime secret;
- JavaScript, PostgreSQL, logs, redirects, and API response bodies never
  receive the token; and
- refresh tokens are discarded rather than stored. Expiry or revocation asks
  the user to authorize GitHub again.

GitHub sign-in and explicit connection both issue the credential cookie. A
signed-in password user, or a user whose cookie expired, uses a repository
access authorization flow that requires the current Slash session and accepts
only the GitHub subject already linked to that user. It cannot silently replace
or merge identities. Slash logout clears both the Slash session and GitHub
credential cookies.

This bounded credential deliberately avoids a durable OAuth-token table,
refresh-token rotation, encryption-key configuration, and a mirrored
repository membership cache. A GitHub API `401` clears the cookie and returns
a reauthorization-required response. Other upstream failures retain the cookie
and return `503`.

The credential cookie is required for discovery only. A bookmarked repository
history page can still be authorized through the live collaborator check after
the discovery credential expires.

## History API

`GET /api/invocations` accepts:

| Field | Meaning |
| --- | --- |
| `installation_id` | Required GitHub App installation id |
| `repository_id` | Required stable GitHub repository id |
| `owner`, `repo` | Required current routing names used for the GitHub permission check |
| `status` | Optional exact lifecycle status |
| `command` | Optional exact configured command name |
| `cursor` | Optional opaque keyset cursor |
| `limit` | Page size, default 50 and maximum 100 |

The response has `items` and `next_cursor`; it does not calculate a total.
Rows are ordered by `(created_at DESC, id DESC)`, and the cursor carries those
two values. A new append-only migration adds the matching
`(installation_id, repository_id, created_at DESC, id DESC)` index. Offset
pagination is not exposed because new invocations would otherwise move rows
between pages.

Each item contains only repository-user-safe fields:

- invocation id, command, actor login, PR number, attempt, and head SHA;
- lifecycle status and GitHub conclusion;
- creation, dispatch, correlation, and completion timestamps when present;
- check-run and workflow-run links when correlation has produced their ids;
  and
- enough lifecycle state for the Web console to derive a bounded user-facing
  outcome label from status and conclusion.

The endpoint does not return `args`, `raw_comment_line`, `failure_reason`,
`delivery_guid`, webhook payloads, or upstream error text. Arguments may contain
accidentally supplied secrets, and persisted failure strings are diagnostic
data that may contain upstream details. The row instead links back to the
GitHub pull request, comment, check, or workflow run, where repository access
and GitHub's own presentation rules apply. Raw diagnostic fields remain
instance-admin-only.

The status shown to users preserves the lifecycle rather than inventing a
second state machine:

| Invocation state | Activity presentation |
| --- | --- |
| `claimed`, `dispatched`, `correlated` | Running, with the exact stage available as secondary text |
| `completed` | The persisted GitHub conclusion |
| `aborted` | Aborted before dispatch completion |
| `dispatch_failed` | Dispatch failed |
| `correlation_timeout` | Workflow correlation timed out |
| `superseded` | Superseded by a newer invocation |

## Web console

Activity is a top-level item in the authenticated console navigation. The page
uses an installation selector followed by a repository selector, both backed
by the current GitHub user credential. The selection is represented in the URL
so a repository view can be bookmarked; no authorization result is stored in
browser local storage.

The responsive, horizontally scrollable table shows status, command, pull
request, actor, trigger time, and elapsed or final duration. Rows link to
GitHub rather than adding a second invocation-detail page in the first version.
The page supports status and command filters, **Load more** keyset pagination,
and an explicit refresh action. It does not poll in the background.

Empty and failure states are distinct:

- no linked GitHub identity: **Connect GitHub**;
- missing or expired discovery credential: **Refresh GitHub access**;
- no accepted invocation rows in the repository: explain what counts as an
  activity row;
- repository access removed: retain the selectors, explain the lost access,
  and offer a fresh authorization check;
- GitHub unavailable or rate limited: retain the selection and offer retry;
  and
- Slash unavailable: show the normal console retry state.

Repository and command filters are independent of the Slash team/workspace
switcher. Teams own Slash-native products such as Test Engine data; they are
not a proxy for GitHub repository authorization.

## Retention, consistency, and non-goals

The feature reads the existing invocation record and does not introduce a
second event table or history-specific retention setting. It displays every
matching invocation still retained by Slash. A future invocation-retention
policy would therefore apply equally to reconciliation storage, admin
diagnostics, and this page and must be designed at that owner rather than as a
Web preference.

Invocation updates may race with a page read. Each row is internally
consistent with one committed database state, and a refresh observes later
transitions. GitHub links may eventually disappear under GitHub's own
retention rules without changing Slash's durable status.

The first version does not provide CSV export, aggregate analytics, rejected
attempt auditing, rerun or cancellation controls, free-text search, saved
filters, cross-repository aggregation, or public REST-token access. These can
be added only when their authorization, rate, and retention contracts are
explicit; none requires changing the invocation lifecycle defined here.
