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
cargo test --workspace                    # database-backed tests skip
```

`cargo` is via mise (`mise.toml` pins `rust = 1.94.1`). Database-backed tests
are deliberately excluded from the automatic pre-push gate so local database
availability or schema drift cannot block unrelated pushes. Run the opt-in
`mise run check:rust-db` task when changing persistence or migrations; it uses
the local `slashtest-pg` container.

## Web SPA (`web/`)

```sh
npm run build    # tsc -b && vite build
npm run lint     # oxlint (one pre-existing button.tsx fast-refresh warning is known)
npm test         # vitest run
```

Node is pinned in `mise.toml` and installed with `mise install`.

## Docs site (`site/`)

```sh
npm test            # source-level Node tests
npm run build       # astro build
npm run typecheck   # astro check
npm run lint:docs   # nimbus-docs lint
npm run test:output # generated-site regression tests
```

## Automation

These gates are defined as mise tasks and selected automatically by the
repository's prek pre-push hook. After cloning, run `mise install` followed by
`mise run hooks:install`. The explicit task equivalents are
`mise run check:rust`, `mise run check:web`, and `mise run check:site`.

## Note

Clippy on test code uses `#[allow(clippy::unwrap_used, …)]` per module — a
missing `allow` is a real lint regression, not something to push around.
