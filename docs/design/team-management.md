# Team management and invitations

This document defines the implemented Team membership lifecycle in Slash.
It complements the identity and tenancy invariants in `docs/architecture.md`.

## Roles and authorization

A Team has `member` and `maintainer` roles. Every member can read the Team
roster. Maintainers can invite people, resend or revoke pending invitations,
change member roles, and remove members. Slash refuses any operation that
would leave an active Team without a maintainer.

Team membership also implies ordinary membership of the owning Organization.
Accepting an invitation therefore inserts both records in one transaction.

## Invitation lifecycle

A maintainer invites a normalized email address as either a member or a
maintainer. Slash does not reveal whether that email already has an account.
At most one pending invitation exists for a Team and email pair; inviting it
again rotates the credential and sends a fresh message.

Invitation links contain a random 256-bit bearer credential in the URL
fragment. Browsers do not send fragments in HTTP requests, so reverse-proxy
access logs do not capture the credential. The Web App submits it in a POST
body for preview and acceptance. PostgreSQL stores only a SHA-256 digest.
Credentials expire after seven days, are single-use, and become invalid when
an invitation is revoked or replaced.

If the invited email belongs to an existing active Slash account when the
invitation is created, the invitation is bound to that account. Otherwise the
first authenticated account that presents the credential may accept it. This
supports both existing users and people who register after receiving mail;
possession of the mailbox-delivered credential is the proof of invitation.

Database state is committed before SMTP delivery. If the relay rejects a
message, the API reports delivery failure while preserving the pending
invitation so a maintainer can safely resend it.

## Email delivery

Email delivery is optional at process startup and uses an unauthenticated SMTP
relay on a trusted network. It requires `SLASH_SMTP_HOST`, `SLASH_EMAIL_FROM`,
and `SLASH_BASE_URL`; `SLASH_SMTP_PORT` defaults to `25` and
`SLASH_EMAIL_FROM_NAME` defaults to `Slash`. Partial configuration is rejected
at startup. Invitation creation is unavailable when email is not configured.

Slash deliberately does not accept inline SMTP credentials. Deployments that
need upstream authentication should place a private relay beside Slash and
keep those credentials in the relay's file-backed secret configuration.

## HTTP surface

- `GET /api/teams/{team_id}/members` lists members and, for maintainers,
  pending invitations.
- `POST /api/teams/{team_id}/invitations` creates or replaces an invitation.
- `POST /api/teams/{team_id}/invitations/{invitation_id}/resend` rotates and
  resends an invitation.
- `DELETE /api/teams/{team_id}/invitations/{invitation_id}` revokes it.
- `PATCH /api/teams/{team_id}/members/{user_id}` changes a role.
- `DELETE /api/teams/{team_id}/members/{user_id}` removes a member.
- `POST /api/team-invitations/preview` previews a bearer invitation without a
  session.
- `POST /api/team-invitations/accept` accepts one using a browser session.
