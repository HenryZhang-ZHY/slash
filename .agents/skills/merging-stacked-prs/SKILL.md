---
name: merging-stacked-prs
description: Use when landing a stack of dependent GitHub PRs (A ← B ← C, each based on the one below) onto main, or whenever a request mentions "stacked PRs", "PR stack", or "dependent PRs". Use GitHub's official stack object and `gh stack` — do not merge-and-retarget individual PRs by hand.
---

# Landing a GitHub PR stack

Land dependent PRs through GitHub's native stack object and `gh stack`. Manual
`squash-merge + rebase the next PR` produces diff pollution (the next PR
re-includes the already-merged commits) and retargeting mistakes — do not do
it.

## Prerequisites

- `gh stack` must be available (`gh stack --version`). Hard-stop if not.
- Every head branch in the stack must live in this repository (no cross-fork
  stacks).

## Establish the official stack

If the PRs are not yet linked into one official stack, link them bottom-to-top
(all authors must match):

```sh
gh stack link --base main <bottom-pr> <next-pr> ... <top-pr>
```

`gh stack link` is additive — never dissolve, reorder, or rebuild an existing
stack automatically.

## Refresh only when needed

Do not rewrite branches unless the live merge state requires it. Prefer the
native cascading flow:

```sh
gh stack checkout <pr-or-stack>   # when not tracked locally
gh stack sync                     # may rebase + force-push active layers
```

If `sync` reports a conflict, resolve via `gh stack rebase`, validate, then
`gh stack push`. After any history rewrite, re-fetch exact heads and re-audit
review state, approvals, and checks.

## Preflight, then merge

Re-query the official stack immediately before merging. Require every selected
PR to be open, non-draft, in expected order, and passing review + checks. Then
merge the whole stack by its stack number:

```sh
gh stack merge <stack-number> --yes --merge
```

GitHub merges bottom-up and retargets/re-bases any remaining upper layers. Do
not pass `--delete-branch` or issue per-PR merge commands. If the native merge
reports a blocker, resolve it or stop — never fall back to `gh pr merge`.

## Verify

Wait for every selected PR to report `MERGED` (a queued request is not a
landing). Delete branches only after their PRs are `MERGED` and no open PR
still uses them as a base:

```sh
gh pr list --state open --base <branch> --json number --jq length
```
