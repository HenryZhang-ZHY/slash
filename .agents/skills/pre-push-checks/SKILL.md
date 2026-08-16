---
name: pre-push-checks
description: Run before pushing any branch so CI does not fail on something catchable locally. Covers the Rust workspace, the web SPA, and the docs site.
---

# Pre-push checks

Run the relevant gate for the crates/dirs you changed before pushing. All of
these must be clean — do not push red.

## Rust workspace

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets    # zero warnings
SLASH_TEST_DATABASE_URL=postgres://slash:slash@127.0.0.1:5433/slash cargo test --workspace
```

`cargo` is via mise (`mise.toml` pins `rust = 1.94.1`); the test database is
the local `slashtest-pg` container.

## Web SPA (`web/`)

```sh
npm run build    # tsc -b && vite build
npm run lint     # oxlint (one pre-existing button.tsx fast-refresh warning is known)
npm test         # vitest run
```

Node via `mise use node@26.4.0`.

## Docs site (`site/`)

```sh
npm run build       # astro build
npm run typecheck   # astro check
npm run lint:docs   # nimbus-docs lint
```

## Note

Clippy on test code uses `#[allow(clippy::unwrap_used, …)]` per module — a
missing `allow` is a real lint regression, not something to push around.
