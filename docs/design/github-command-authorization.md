---
outline:
  - repository trust boundary
  - private and public defaults
  - authorization flow
  - removed grants model
  - security and migration
---

# GitHub command authorization

Status: accepted for implementation (2026-08-18)

## Problem

Slash currently performs two unrelated authorization checks for one command.
It first resolves the comment author's live GitHub repository role, then ignores
that result for dispatch and requires a matching grant from Slash's database.
Both failures use the same message about GitHub access.

The grants path cannot be configured reliably in the product. Onboarding
creates a Slash organization without connecting it to a GitHub App
installation, while command authorization resolves the organization only by
that missing installation id. Its tests manufacture this relationship directly
in the database, so they pass without exercising the user-visible flow.

This model also asks administrators of private repositories to recreate trust
decisions that GitHub already enforces. A person who can read and participate in
a private repository has already crossed that repository's collaboration
boundary. Requiring a second account, team, and grant in Slash adds failure
modes without adding a meaningful security boundary.

## Product rules

GitHub is the source of truth for command authorization. A Slash account is not
required to invoke a command from GitHub.

### Private repositories

Any current collaborator with at least `read` access may run every configured
Slash command. The command's `permission` field is intentionally ignored.

This is the default and has no Slash Web UI configuration. Removing a user from
the private repository immediately removes their ability to run commands
because Slash resolves collaborator access for every invocation.

### Public repositories

The command's `permission` field sets the minimum live GitHub repository role:

| Command permission | Accepted GitHub roles |
| --- | --- |
| `read` | read, triage, write, maintain, admin |
| `write` | write, maintain, admin |
| `admin` | admin |

The default command permission remains `write`. This prevents arbitrary public
participants from dispatching workflows unless a repository deliberately
defines a read-tier command.

## Authorization flow

For every issue comment or check-run re-request, Slash:

1. verifies the webhook and identifies its repository and visibility;
2. mints a repository-scoped GitHub App token;
3. resolves the actor using GitHub's collaborator-permission API;
4. denies if the API fails, the role is unrecognized, or the role is below
   `read`;
5. allows a `read+` actor immediately when the repository is private;
6. otherwise compares the actor's role with the public command's required
   permission;
7. records the policy, resolved role, required permission, repository
   visibility, and decision in structured logs.

Repository visibility comes from the signed webhook repository object. Missing
visibility is not guessed and cannot become an allow.

## Feedback

Authorization feedback describes the policy that actually rejected the actor:

- private repository: the user is no longer a repository collaborator;
- public repository: the command requires a specific GitHub role;
- resolution failure: react only and log the internal failure, preserving the
  existing anti-abuse rule that Slash does not comment for an unknown actor.

The phrase "write access" must never describe a failed Slash database lookup.

## Removed model

The `grants` table, grants decision core, grants loader, grants administration
API, and Grants Web UI are removed. Repository and command grants do not remain
as a hidden fallback or compatibility mode.

The `repos` table was introduced only to support grants and is also removed.
The unused `organizations.installation_id` authorization link is removed;
installation lifecycle remains in the dedicated `installations` table.

Slash organizations and teams continue to own Slash-native products such as
Test Engine data. They are no longer a prerequisite for GitHub ChatOps.

## Security properties

- Authorization is fail-closed on webhook, token, visibility, collaborator,
  and role-resolution failures.
- A webhook's `author_association` is never an allow signal.
- Access is checked live for every invocation and re-request; stale Slash
  membership cannot preserve access.
- Private-repository allow-all never extends to a public repository.
- GitHub Actions environments, branch rules, and workflow design remain the
  final controls for sensitive deployment effects.

## Non-goals

- Mirroring GitHub collaborators into Slash users or teams.
- Per-user or per-team command grants in Slash.
- Cross-repository inheritance or organization-wide policy.
- Supporting commands on pull requests from forks.
