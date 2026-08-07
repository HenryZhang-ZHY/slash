# Slash Live Smoke Test Design

## Goal

Add a minimal, low-cost end-to-end test surface for the deployed Slash GitHub
App in `HenryZhang-ZHY/slash`.

## Commands

The default branch will define two argument-free Slash commands:

- `/fake-ci-test-pass`
- `/fake-ci-test-failure`

Both require `write` permission. Each command maps to a separate workflow with
the same base filename, keeping the success and failure paths independent and
easy to diagnose.

## Workflows

Both workflows use GitHub-hosted `ubuntu-latest` runners and only support
`workflow_dispatch`. They declare all five inputs injected by Slash:
`slash_run_id`, `slash_pr_number`, `slash_head_sha`, `slash_actor`, and
`slash_actor_id`.

The passing workflow prints a short fake-CI transcript and exits successfully.
The failing workflow prints a short fake-CI transcript and explicitly exits
with status 1. Neither workflow checks out code, installs dependencies, starts
services, or consumes secrets.

## Expected End-to-End Behavior

After the four files are on `main`, create or use a pull request and add one
command per comment:

- `/fake-ci-test-pass` should produce a `slash/fake-ci-test-pass` check that
  concludes `success`.
- `/fake-ci-test-failure` should produce a
  `slash/fake-ci-test-failure` check that concludes `failure`.

Each command should receive Slash's launch reaction, dispatch exactly one
GitHub Actions run, and link the check run to that workflow run.

## Validation

Validate both `.slash` files with the repository's `slash validate` CLI and
parse all four files as YAML. Do not trigger the live test until the files are
present on the default branch, because GitHub resolves `workflow_dispatch`
targets there.
