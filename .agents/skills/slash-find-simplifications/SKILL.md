---
name: slash-find-simplifications
description: Use when working in the Slash repository to find, verify, document, or implement non-obvious simplifications; especially for dead or duplicated Rust APIs, mirrored state, speculative configuration, redundant database schema, obsolete Web UI/API surface, over-built GitHub lifecycle handling, or hand-rolled code that a stable dependency can replace.
---

# Finding Slash Simplifications

Turn broad simplification requests into a small set of evidence-backed changes that reduce real product and maintenance surface. Prefer deleting concepts over rearranging them, and reject attractive ideas that weaken Slash's load-bearing invariants.

## Establish the constraints

Read `AGENTS.md`, `docs/architecture.md`, the relevant files in `docs/design/`, and `.agents/skills/pre-push-checks/SKILL.md`. Treat the architecture document as the default system authority, but compare it with shipped code and newer accepted design decisions before assuming every detail is current.

Preserve these invariants unless the user explicitly requests a redesign and the replacement is documented:

- untrusted input is panic-free and authorization fails closed;
- dispatch has a durable record before GitHub mutation and reconciliation is level-triggered;
- installation tokens are repository-scoped and least-privileged;
- secrets are file-only and never logged;
- `slash-core` stays pure while network and database IO remain at the edges;
- webhook delivery, invocation, workflow-run, and check-run identities remain idempotent and tenant-scoped.

Do not infer permission for a breaking product decision merely because the repository is pre-1.0. State the compatibility and migration cost explicitly.

## Recognize strong candidates

A strong candidate removes or collapses a concept whose ongoing cost exceeds its demonstrated value:

- a public function, endpoint, config key, event, metric, database column/table, route, component, or translation has no production consumer;
- tests or docs are the only consumers and do not protect a current invariant;
- Rust, SQL, and TypeScript keep separate representations of the same fact;
- two lifecycle flags, timestamps, statuses, or retry paths encode one transition;
- every implementation supports a trait method that no caller uses;
- a compatibility or fallback path cannot be reached by the supported product flow;
- a package, module, page, or API exists for speculative future generality;
- hand-rolled parsing, retry, encoding, diffing, or collection code can be replaced by a Rust/Node standard facility or a healthy dependency with net code deletion;
- an added-then-removed feature left migrations, types, metrics, docs, tests, or UI fragments behind.

Reject thin findings such as naming preferences, isolated formatting, or “this looks complex” without call-site and behavior evidence. Group related residue into one concept-level candidate.

## Survey the whole vertical slice

Start with large or frequently changed production files, then follow concepts across layers:

- command/config/core: `crates/slash-command`, `slash-config`, and `slash-core`;
- GitHub edge and orchestration: `slash-github` and `slash-server` pipeline, worker, correlation, sweepers, OAuth, and installation lifecycle;
- persistence: migrations, SQL queries, constraints, indexes, and retained compatibility data;
- Web console: routes, API clients, types, state, components, translations, and tests;
- operational surface: Dockerfile, workflows, metrics, examples, deployment docs, and IaC assumptions;
- documentation: product spec, accepted designs, implementation plans, English and Chinese user docs.

Use subagents for breadth only when the user or active session instructions explicitly allow delegation. Assign disjoint domains and require raw evidence and rejected candidates, not a target count.

## Prove or reject each candidate

Use `rg` first. Search exact Rust symbols, SQL identifiers, JSON/YAML keys, route paths, metric labels, webhook strings, TypeScript types, translation keys, and filenames. Search both direct calls and dynamic forms.

Classify every consumer:

- **production:** crate `src/`, Web `src/`, runtime scripts, migrations, workflow and configuration loaders;
- **operational:** Docker, GitHub Actions, deployment configuration, dashboards and alerts;
- **non-production:** tests, fixtures, docs, examples, snapshots, and comments;
- **historical:** old migrations and design records that may intentionally preserve why a decision existed.

Then read the call sites and answer:

1. What product behavior or invariant does this surface own today?
2. Is there a production or operational consumer, including dynamic dispatch?
3. What code, schema, tests, docs, and configuration disappear together?
4. Does the proposal delete state or merely move complexity behind a wrapper?
5. What behavior, compatibility, observability, or recovery capability is lost?
6. Which focused test can demonstrate the simpler contract before implementation?

Downgrade or reject a candidate when it has a real caller, protects a documented invariant, requires unrelated churn without reducing concepts, or depends on guessing future product intent.

## Audit trust and lifecycle machinery

For every validator, defensive copy, retry, timeout, sentinel, lock, and callback capture, identify the trust boundary and owner. Signed webhooks, GitHub API JSON, user YAML, HTTP bodies, database rows, durable queues, and process boundaries require validation. Same-process typed calls usually do not require hostile-object defenses unless mutation or concurrency makes ownership ambiguous.

For asynchronous flows, sketch the state transition and map each flag, timestamp, lease, sweeper, and terminal outcome to one owner. Mirrored liveness facts are simplification candidates; independent protections for write-ahead publication, atomic claiming, retry ambiguity, cancellation, or terminal-outcome arbitration are not automatically redundant.

For schema simplification, inspect all later migrations and live SQL before proposing a drop. Historical migrations normally remain immutable; remove current objects through a new migration. Prove a table or column is absent from Test Engine and installation lifecycle before treating it as command-only residue.

## Evaluate dependency swaps

Prefer the standard library or an existing workspace dependency when it covers the required semantics. Before adding a dependency, verify maintenance, adoption, transitive footprint, supported Rust/Node floor, security posture, and net deletion. Count the wrapper and residual edge cases against the proposal. Replacing one local implementation with equally complex glue is not simplification.

## Choose the output

Match the artifact to the user's request:

- **Audit/report:** return a ranked list with file/call-site evidence, deletion scope, risks, and rejected near-misses. Do not edit code.
- **Small local cleanup:** implement directly with a focused regression test. Add a tagged `TODO`, `FIXME`, or `XXX` only when the user asked for notes and the action is local and unambiguous.
- **Durable cross-module decision:** when asked to document it, add or update one English design document under `docs/design/`; consolidate with the existing owner rather than creating a competing proposal.
- **Implementation:** use TDD, remove the complete vertical slice, update both English and Chinese user docs where applicable, and make atomic Conventional Commits.

For each accepted candidate, record:

- the duplicated or unnecessary concept;
- production, operational, test-only, and historical consumers;
- exact deletion or consolidation boundary;
- strongest reason to keep it;
- behavior and compatibility given up;
- acceptance criteria and validation commands.

## Fold work from another branch or PR

Diff the source branch against `origin/main`, not the current feature branch, to isolate its contribution. Port only independently proven simplifications. Consolidate overlapping proposals into the existing design owner, update the PR description to reflect the true scope, and do not close or merge another PR without authorization.

## Validate and hand off

Run the gates for every changed surface from `.agents/skills/pre-push-checks/SKILL.md`; run `git diff --check` for all changes. A schema deletion requires tests against a freshly migrated PostgreSQL database. A removed route or Web feature requires both server and Web validation. A documentation removal requires the docs build, typecheck, link-aware lint, and an `rg` sweep for stale references.

Before pushing, summarize:

- candidates accepted, rejected, consolidated, and implemented;
- areas surveyed and intentionally excluded;
- concepts and lines/files removed versus glue added;
- behavior or compatibility deliberately given up;
- checks passed and any known baseline warning.

Open or update the requested PR after validation; do not merge unless the user
explicitly asks for it.
