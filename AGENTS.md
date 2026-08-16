# Slash — agent working conventions

Slash is a GitHub-PR slash-command control plane (Rust workspace). These
conventions apply to every agent working in this repository. Design authority
lives in `docs/design/` (the `0.0.1-spec.md` is the source of truth for
architecture invariants); lane-specific ownership is documented in each
agent's own runtime, not here.

## Working language

English for all code, comments, commits, and docs. The team's chat happens in
Chinese, but nothing that lands in the repository is Chinese.

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

## Secrets are file-only

Every secret is configured via a `*_PATH` env var pointing at a file read
byte-for-byte at startup — never an inline env var, and never trimmed. There
is exactly one way to configure a secret: the secure one.

## Review gate

@SlashLead is the single merge gate. PRs are squash-merged through the GitHub
PR interface; do not local-merge. The merge rule and 0.0.1-spec invariants
(fail-closed permission gate, durable-record + level-triggered reconcile,
write-ahead dispatch) are non-negotiable.
