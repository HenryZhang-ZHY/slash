---
outline:
  - control-plane boundary
  - durable data model
  - ingestion and token security
  - flaky reconciliation
  - ownership and extension rules
---

# Test Engine architecture

Status: implemented architecture baseline (2026-08-18)

Test Engine is Slash's test-results control plane. CI systems still execute
tests; Slash ingests normalized results, stores their history, exposes current
state, and reconciles flaky-test disposition. This boundary prevents test
execution concerns from becoming server orchestration concerns.

## Durable model

The PostgreSQL model follows `suite -> test -> run -> execution`:

- `test_suites` identifies a repository suite by installation, owner,
  repository, and suite key;
- `tests` identifies a named test within a suite and holds its current
  `enabled`, `muted`, or `skipped` state, the decision's provenance, labels,
  and owner team ids;
- `test_runs` identifies a collector batch by suite, installation, provider,
  and provider run reference;
- `test_executions` is the append-only record of an observed result.

Unique constraints make repeated suite, test, and run discovery idempotent. A
replayed collector batch returns the existing run without appending duplicate
executions; separate jobs, shards, and attempts must use distinct run
references.
Every state change records its source, reason, actor when applicable, and an
append-only event. Manual decisions are authoritative and automatic
reconciliation cannot overwrite them. There is no
implemented execution-retention job; adding one requires an explicit product
retention decision and an operationally verified cleanup path.

Suites are owned by the Slash user who creates or claims them. Team UUIDs on a
test express ownership metadata inside that account boundary; they do not grant
permission to invoke GitHub commands.

## Ingestion and collection tokens

The collection endpoint accepts normalized JSON and supported collector
formats including JUnit, Cargo, and Vitest results. A run is normalized into
the durable model before downstream flaky decisions are made. Provider-specific
details do not leak into the reconciliation algorithm.

Each suite has independently rotatable collection tokens. Slash stores a
SHA-256 hash for bearer-token lookup and separately stores the raw value
encrypted for the authenticated console. Tokens have an explicit active or
revoked lifecycle; revoked tokens fail ingestion. Tokens and uploaded test
output must never appear in logs.

## Flaky reconciliation

Flaky state is derived from durable execution history rather than from a single
upload event. The current rule requires at least three executions for the same
test in a rolling seven-day window and an observed failed-or-errored result
followed by a later pass.

The level-triggered reconciler pages through tests in bounded batches:

- an `enabled` flaky test becomes `muted`;
- a monitor-muted test returns to `enabled` after failures leave the window;
- `skipped` is a manual state and is never automatically enabled.

The reconciler ignores every test whose current state source is `manual`.
Operators can therefore deliberately mute, skip, or enable a test without the
next sweep replacing that decision.

The existing server sweeper supplies the reconciliation heartbeat. Re-running
a pass is safe, and missed process-local events do not lose work because the
decision is reconstructed from PostgreSQL.

## Extension rules

New collectors should normalize into the existing execution vocabulary rather
than add provider-specific state tables. New automated dispositions must define
their durable evidence, guarded transitions, recovery behavior, and bounded
scan strategy before adding event handlers.

Cross-agent scheduling, retry orchestration, general analytics, and a dedicated
MCP server are not current architecture. They should be proposed only when a
real consumer demonstrates the need; this document intentionally does not
preserve speculative milestone roadmaps.

The authenticated API and setup examples are documented in the Test Engine
user guides in
[English](../site/src/content/docs/en/test-engine/index.mdx) and
[Chinese](../site/src/content/docs/zh-hans/test-engine/index.mdx).
