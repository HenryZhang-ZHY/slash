---
outline:
  - product and trust boundaries
  - command configuration and execution
  - durable delivery and invocation lifecycle
  - GitHub integration and failure semantics
  - repository layout and release discipline
---

# Slash architecture

Status: implemented architecture baseline (2026-08-18)

This document records the system properties that current code depends on. It
replaces milestone specifications and implementation plans as the stable
architecture reference. Topic-specific decisions may refine it, but they must
not silently weaken its trust and recovery boundaries.

## Product boundary

Slash is a GitHub App control plane for pull-request slash commands. It parses
commands, authorizes actors, creates checks, dispatches GitHub Actions, and
reconciles outcomes. Repository-owned GitHub Actions runners remain the
execution plane; Slash does not execute repository code.

Only pull requests whose head branch belongs to the same repository are
supported. Fork pull requests are rejected because their workflow and secret
boundaries differ materially from same-repository branches.

GitHub is authoritative for repository access. Private repositories allow any
current collaborator with read access to invoke every configured command.
Public repositories enforce the command's declared minimum GitHub role. The
complete policy is in [GitHub command authorization](design/github-command-authorization.md).

## Command configuration and parsing

Commands are declared in `.slash/*.yml` or `.slash/*.yaml`. Slash resolves the
repository's default branch to an immutable commit SHA and loads every command
file from that same SHA. Pull-request code and workflow bodies come from the PR
head, but policy comes from the default branch so a PR cannot approve its own
permissions.

The catalog is atomic. Loading produces one of four outcomes:

- `Loaded`: every file was fetched and validated;
- `NotConfigured`: the directory has no command configuration;
- `Invalid`: at least one configuration is malformed or semantically invalid;
- `Unavailable`: GitHub or another required dependency could not provide a
  trustworthy answer.

Slash never treats an error as an empty catalog and never serves a persisted
stale catalog. Partial catalogs are rejected because omitting one policy file
would change authorization behavior.

The built-in `/slash help` command reads that same validated default-branch
catalog and posts its commands, usage, required GitHub role, argument details,
and an example directly on the pull request. It never creates an invocation,
check run, or workflow dispatch.

Only the first line of a comment can be a command. Command and input names are
bounded and validated, the `slash_` prefix is reserved, unknown configuration
keys fail validation, and untrusted input must not reach a panic. Before
dispatch, Slash re-reads the PR branch tip and aborts if it differs from the
authorized SHA.

Every dispatch injects the trusted inputs `slash_run_id`, `slash_pr_number`,
`slash_head_sha`, `slash_actor`, and `slash_actor_id`. Repository configuration
cannot redefine them.

## Durable delivery inbox

Webhook handling verifies the signature against the raw request body before
JSON parsing. After verification, it extracts best-effort installation and
repository identifiers for queue routing; these untrusted hints never make an
authorization or policy decision. A valid delivery is inserted into PostgreSQL
under the unique `X-GitHub-Delivery` identifier before Slash returns success.
Workers claim
eligible rows with `FOR UPDATE SKIP LOCKED` and commit an expiring lease before
running the GitHub pipeline. Each lease has a unique fencing token: completion
and failure updates succeed only for the current token, so an expired worker
cannot overwrite a newer owner's result. A crashed worker holds no database
transaction or connection; another replica can reclaim the delivery after the
lease expires. This makes redelivery and concurrent replicas safe without an
in-memory queue or transaction-spanning external I/O.

Each server starts a bounded delivery-worker pool. Active workers renew their
leases while the pipeline runs. A known-safe transient failure may return a
delivery to the inbox with a future attempt time; failures whose side effects
could be ambiguous are not blindly replayed.

Installation-level concurrency is coordinated in PostgreSQL, not process
memory. Claimers briefly serialize on an installation advisory lock and count
its live delivery leases before committing new work. At most eight deliveries
for one installation are active across all replicas; installations with fewer
active leases are preferred so a busy tenant does not block an idle one.

PostgreSQL is a deliberate coordination dependency: uniqueness constraints,
transactions, guarded updates, and row claiming are part of correctness rather
than replaceable storage details. Terminal deliveries are retained for 30 days
by default before the sweeper removes them.

The supported deployment baseline and failure-validation contract are recorded
in [`docs/design/capacity.md`](design/capacity.md).

## Invocation lifecycle

An invocation is the durable record connecting a parsed command, its check run,
the workflow dispatch, and the eventual workflow run. Its normal lifecycle is:

```text
claimed -> dispatched -> correlated -> completed
```

Terminal alternatives are `aborted`, `dispatch_failed`,
`correlation_timeout`, and `superseded`. State transitions use guarded
compare-and-swap updates; terminal state absorbs duplicate or out-of-order
events.

The `dispatched` transition is committed before the GitHub
`workflow_dispatch` request. This write-ahead step matters because dispatch is
not idempotent. A connection failure or rate limit may be retried only when the
request is known not to have been sent. A timeout or 5xx response is ambiguous:
Slash leaves the invocation dispatched, polls GitHub to correlate a run, and
never blindly posts a second dispatch.

The level-triggered sweeper repairs work from durable state on every replica.
Current defaults are:

| Reconciliation boundary | Default |
| --- | --- |
| Sweep interval | 60 seconds |
| Stale `claimed` lease | 60 seconds |
| Ambiguous dispatch correlation | 10 minutes |
| Correlated run deadline | 72 hours |
| Terminal delivery retention | 30 days |

GitHub check-run names are stable API. A required check that never runs remains
in GitHub's expected state, and each new push needs a new completed check.
Workflow authors must account for that branch-protection behavior.

## Security and ownership

GitHub App installation tokens are minted per repository with the least
permissions needed for the operation. Secrets are configured only through
`*_PATH` variables and are read byte-for-byte from files. Tokens, secret
contents, webhook bodies, and upstream error bodies must not be logged or
returned to browsers.

`slash-core` owns pure policy and state decisions. Network, database, clock,
and GitHub effects stay at crate boundaries where they can be tested through
explicit interfaces. Identities for deliveries, invocations, workflow runs,
and check runs remain tenant-scoped and idempotent.

Authentication separates the internal user, configured trust domains,
external identities, password credentials, and contact addresses. External
accounts resolve only by `(connection_id, subject)`; email and mutable provider
profile fields never merge accounts. Protocol adapters normalize provider
responses before the provider-neutral persistence layer. The complete contract
is in [Authentication and external identities](design/authentication.md).
Browser user and instance-admin sessions are carried in HttpOnly, Secure
cookies, so every deployment that enables those surfaces requires HTTPS ingress
and must redirect plaintext HTTP before it reaches authenticated routes.

Personal access tokens authenticate REST API clients as their owning user and
never copy authorization state. Plaintext tokens are shown once and only a
keyed digest is persisted. Credential management remains browser-session-only;
the complete lifecycle is in [Personal access tokens](design/personal-access-tokens.md).

Team membership uses explicit `member` and `maintainer` roles and never permits
an active Team to lose its last maintainer. Email invitations are single-use,
seven-day bearer credentials whose plaintext appears only in the email URL
fragment; PostgreSQL stores only a digest. SMTP credentials remain outside
Slash behind a trusted private relay. The complete lifecycle is in
[Team management and invitations](design/team-management.md).

Repository command activity is visible to browser users only after a live
GitHub collaborator check for the selected repository. Discovery uses a
short-lived encrypted GitHub user credential bound to the Slash session and
external subject; the token is never stored in PostgreSQL or exposed to
JavaScript. History queries remain tenant-scoped by stable installation and
repository ids and omit raw command arguments and diagnostic strings. The
complete boundary is in
[Command invocation history](design/command-invocation-history.md).

Instance administration is a separate secret-gated trust boundary. It is
disabled when its file-backed secret is absent and grants no repository or
organization role. The operational surface and installation reconciliation
rules are in [Instance admin console](design/admin-console.md).

## Repository and release discipline

The Rust workspace contains pure domain crates, GitHub/configuration adapters,
and the server orchestration edge. `web/` is the authenticated console and
`site/` is public documentation. Database evolution is append-only through
numbered migrations; historical migrations are not rewritten after release.

Releases are manual GitHub Actions runs. The workflow reads the
`slash-server` Cargo version, builds and publishes versioned and `latest` GHCR
images with provenance and an SBOM, then creates the matching GitHub Release
and tag. A version whose tag already exists is rejected.

For an end-to-end non-production exercise, see the
[fake CI smoke test](user/fake-ci-smoke-test.md). Test-result governance has a
separate [Test Engine architecture](test-engine.md).
