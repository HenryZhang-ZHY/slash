# Slash — agent working conventions

Slash is a GitHub-PR slash-command control plane (Rust workspace). These
conventions apply to every agent working in this repository. System invariants
live in `docs/architecture.md`; current topic-specific decisions live in
`docs/design/`. Lane-specific ownership is documented in each agent's own
runtime, not here.

## Working language

English for all code, comments, commits, and canonical documentation. The
team's chat happens in Chinese. Reviewed public translations under
`site/src/content/docs/zh-hans/` are the only repository prose authored in
Chinese.

## Small, independently-verifiable PRs

Plan first, then split work into small, regularly-deliverable PRs — each
independently verifiable and mergeable. Frequent small PRs surface mistakes
early. Do not batch a large feature into one PR.

When work naturally decomposes into dependent PRs (A ← B ← C), use GitHub's
official stacked-PR flow — see
[`.agents/skills/merging-stacked-prs/SKILL.md`](.agents/skills/merging-stacked-prs/SKILL.md).
Never merge-and-retarget stacked PRs by hand.

## Pre-push checks

Run the full local gate before pushing, so CI does not fail on something you
could have caught locally — see
[`.agents/skills/pre-push-checks/SKILL.md`](.agents/skills/pre-push-checks/SKILL.md).

## Documentation ownership

Keep stable system invariants in `docs/architecture.md`, implemented
topic-specific decisions in `docs/design/`, and operator or user procedures in
their audience-specific directories. Do not commit review transcripts,
temporary implementation plans, or milestone checklists as durable product
documentation. Use
[`slash-maintain-docs`](.agents/skills/slash-maintain-docs/SKILL.md) to route
facts, synchronize existing English and Simplified Chinese pages, and validate
the result. Link code comments to a stable document by name instead of to
section numbers that will drift.

## Secrets are file-only

Every secret is configured via a `*_PATH` env var pointing at a file read
byte-for-byte at startup — never an inline env var, and never trimmed. There
is exactly one way to configure a secret: the secure one.
