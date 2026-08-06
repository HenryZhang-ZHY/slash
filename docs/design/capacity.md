# Capacity and failure contract

Slash is sized around durable webhook admission and installation-scoped GitHub
API work. Process count is not a correctness boundary: every server replica
shares PostgreSQL, and delivery leases plus installation concurrency are
coordinated there.

## Supported baseline

The initial production baseline is three server replicas, eight workers per
replica, and no more than eight active deliveries for one GitHub App
installation across the complete deployment. It targets a burst of 64 command
deliveries spread across ten repositories and an ordinary load of roughly
1,000 commands per day. Caddy may send any request to any ready replica; sticky
sessions are neither required nor supported as a correctness mechanism.

The database-backed integration test exercises 64 concurrent durable inserts,
ten installation routes, 24 logical workers, 300–1,000 ms simulated upstream
latency, deterministic rate-limit retry, ambiguous 5xx termination, and one
replica abandoning eight leases. It requires admission under 250 ms per
delivery, a complete terminal drain, lease recovery, at most eight live leases
per installation, and exactly one effective dispatch for each successful
delivery.

This baseline is not a GitHub quota guarantee. Installation rate limits,
repository workflow duration, payload mix, database latency, and GitHub
secondary limits remain deployment-specific. Capacity must be reevaluated
before increasing the worker count, the installation limit, or sustained
traffic.

## Failure semantics

- A webhook is successful only after its unique delivery row commits.
- A crashed worker loses authority when its lease expires. A replacement
  worker receives a new fencing token; the stale worker cannot complete or fail
  the row.
- A known-safe failure, such as token minting before downstream work or a
  rate-limit rejection known not to have applied the request, may be retried
  with bounded delay.
- A timeout or 5xx after a non-idempotent GitHub write is ambiguous. Slash does
  not blindly repeat the write; durable invocation state and the sweeper must
  reconcile the result.
- If PostgreSQL is unavailable, `/readyz` returns 503. All replicas may become
  unready because the database is shared; GitHub remains the webhook retry
  source. The reverse proxy must not invent a second POST attempt.

Public operational checks, rollout order, failure exercises, alert meanings,
and rollback cautions live in the self-hosting operations guide.
