# On-Demand Command Catalog Loading

## Goal

Slash must evaluate every valid command comment against the repository's current
command configuration. Configuration changes on the default branch must take
effect without reinstalling the GitHub App, restarting Slash, or waiting for a
cache refresh.

## Decision

Load the command catalog on demand for each strictly parsed command comment.
Do not maintain the catalog from installation or `push` webhooks, and do not
use a persisted or stale configuration cache.

The authoritative configuration is the `.slash` directory on the repository's
canonical default branch. Slash resolves that branch to a commit SHA before
loading the catalog, then reads every configuration file from the same SHA.
This provides a consistent snapshot even if the default branch moves while the
request is being processed.

## Trigger

Slash attempts command discovery only when all of these conditions hold:

1. The webhook is a newly created `issue_comment`.
2. The author is not a bot.
3. The comment belongs to a pull request.
4. The existing command parser accepts the entire comment as one valid,
   single-line `/command ...` invocation.

Comments that fail these guards do not mint an installation token or make
GitHub API requests. In particular, merely containing a slash is not enough to
trigger configuration loading.

## Data Flow

1. Build repository context from the webhook's installation and repository
   identifiers.
2. Mint a least-privilege installation token scoped to that repository.
3. Resolve the comment author's repository role.
4. Fetch the pull request and reject closed pull requests and forks.
5. Determine the canonical default-branch name:
   - Prefer `base.repo.default_branch` from the pull request response.
   - If that optional field is absent, fetch repository metadata.
   - Do not fall back to a hard-coded branch such as `main`.
6. Resolve the default branch to its current commit SHA.
7. List `.slash` at that SHA and load every YAML file from the same SHA.
8. Parse and validate the complete catalog, including duplicate-command
   detection.
9. Match the parsed command, check its required role, bind arguments, claim the
   invocation, and dispatch its workflow.

Configuration from the pull request head is never trusted. A pull request that
changes `.slash` does not affect command execution until those changes reach
the repository's default branch.

## Components

### `DefaultBranchResolver`

The resolver returns the canonical branch name and immutable commit SHA. It
encapsulates the fallback from the pull request's repository data to the
repository metadata API. Missing or inaccessible branch information is an
explicit error.

### `CommandCatalogLoader`

The loader accepts a repository client and commit SHA. It has no responsibility
for permissions, comments, reactions, invocation state, or workflow dispatch.
It returns one of these typed outcomes:

- `Loaded(catalog)`: every configuration file was fetched and validated.
- `NotConfigured`: `.slash` does not exist at the selected SHA.
- `Invalid(details)`: a file cannot be decoded, parsed, or validated, or the
  catalog contains duplicate command names.
- `Unavailable(source)`: GitHub denied or could not complete a required API
  request.

The loader never turns an error into an empty catalog and never returns a
partially valid catalog.

### Pipeline orchestration

`handle_issue_comment` owns guards, authorization, pull-request safety checks,
user feedback, invocation claiming, and dispatch. It maps catalog outcomes to
the appropriate response before performing any invocation side effect.

Check-run rerequests use the same resolver and loader so initial execution and
rerequest cannot interpret repository configuration differently.

## Error Handling and User Feedback

An absent `.slash` directory is a valid repository state. Slash tells the user
that the repository has no configured commands.

GitHub authentication, authorization, rate-limit, network, and server failures
are operational failures, not unknown commands. Slash adds a confused reaction,
posts a generic retry-later message when the actor is trusted to receive
comments, and records structured diagnostics containing the stage, repository,
ref or SHA, path, HTTP status, and GitHub request identifier when available.
Secrets and installation tokens are never logged.

Any invalid configuration file invalidates the complete catalog snapshot.
Slash identifies the affected file and reports the safe validation details.
This prevents commands from appearing or disappearing depending on which files
happened to load successfully.

After user-visible feedback is attempted, the delivery finishes without
dispatching. Slash does not automatically retry the delivery or use an older
configuration. The user can retry the command after the repository or GitHub
API problem is corrected.

Unknown-command and permission-denied responses are possible only after a
catalog has loaded successfully. Each catalog outcome and failure stage has a
structured log event and metric so production behavior is not silent.

## Consistency and Freshness

Each accepted comment observes a fresh default-branch lookup. Resolving the
branch to a SHA defines the configuration snapshot for that invocation:

- A commit merged before SHA resolution is visible.
- A commit merged after SHA resolution is visible to the next command.
- Directory listing and file reads cannot mix revisions.

This design deliberately accepts the GitHub API cost of loading the catalog for
every command. A cache may be considered later only as a performance
optimization backed by measurement; it must not change failure semantics or
serve stale configuration.

## Tests

Automated tests cover:

1. Non-command, multiline, bot, and non-PR comments make no configuration API
   requests.
2. A repository whose default branch is not `main` loads the correct catalog.
3. Missing `base.repo.default_branch` falls back to repository metadata.
4. Failure to determine or resolve the default branch is explicit.
5. A branch moving during loading does not change the SHA used for any content
   request.
6. A missing `.slash` directory produces `NotConfigured`.
7. Directory and individual-file 401, 403, 429, 5xx, and transport failures
   produce `Unavailable`, not an empty catalog or unknown command.
8. Decode, YAML, semantic-validation, and duplicate-command failures produce
   `Invalid` and block the complete catalog.
9. Unknown-command handling occurs only after successful catalog loading.
10. A valid command still passes role checks, claims one invocation, and
    dispatches once.
11. Check-run rerequests and initial issue comments use the same catalog
    resolution rules.

## Out of Scope

- Installation-time command discovery.
- `push` webhook subscriptions for configuration maintenance.
- Persisted or stale configuration caches.
- Trusting `.slash` or workflow changes from a pull request head.
- Changing the command syntax or configuration schema.
