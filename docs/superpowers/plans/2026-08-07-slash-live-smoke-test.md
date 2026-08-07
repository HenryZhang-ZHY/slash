# Slash Live Smoke Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two cheap, deterministic Slash commands that exercise successful and failed GitHub Actions dispatches.

**Architecture:** Two argument-free `.slash` command files map one-to-one to two `workflow_dispatch` workflows. Both use `ubuntu-latest`; one exits 0 and the other exits 1.

**Tech Stack:** GitHub Actions YAML, Slash command YAML

---

### Task 1: Add the fake passing workflow and command

**Files:**
- Create: `.github/workflows/fake-ci-test-pass.yml`
- Create: `.slash/fake-ci-test-pass.yml`

- [ ] **Step 1: Create the passing workflow**

```yaml
name: Fake CI Test Pass

on:
  workflow_dispatch:
    inputs:
      slash_run_id: { required: false }
      slash_pr_number: { required: false }
      slash_head_sha: { required: false }
      slash_actor: { required: false }
      slash_actor_id: { required: false }

jobs:
  fake-ci:
    runs-on: ubuntu-latest
    steps:
      - name: Simulate passing CI
        env:
          ACTOR: ${{ inputs.slash_actor }}
          PR_NUMBER: ${{ inputs.slash_pr_number }}
        run: |
          echo "Starting fake CI for PR #${PR_NUMBER} requested by ${ACTOR}"
          echo "Running fake checks..."
          echo "All fake checks passed."
```

- [ ] **Step 2: Create the passing Slash command**

```yaml
command: fake-ci-test-pass
description: Run a lightweight fake CI workflow that succeeds
permission: write
workflow: fake-ci-test-pass.yml
```

### Task 2: Add the fake failing workflow and command

**Files:**
- Create: `.github/workflows/fake-ci-test-failure.yml`
- Create: `.slash/fake-ci-test-failure.yml`

- [ ] **Step 1: Create the failing workflow**

```yaml
name: Fake CI Test Failure

on:
  workflow_dispatch:
    inputs:
      slash_run_id: { required: false }
      slash_pr_number: { required: false }
      slash_head_sha: { required: false }
      slash_actor: { required: false }
      slash_actor_id: { required: false }

jobs:
  fake-ci:
    runs-on: ubuntu-latest
    steps:
      - name: Simulate failing CI
        env:
          ACTOR: ${{ inputs.slash_actor }}
          PR_NUMBER: ${{ inputs.slash_pr_number }}
        run: |
          echo "Starting fake CI for PR #${PR_NUMBER} requested by ${ACTOR}"
          echo "Running fake checks..."
          echo "A fake check failed."
          exit 1
```

- [ ] **Step 2: Create the failing Slash command**

```yaml
command: fake-ci-test-failure
description: Run a lightweight fake CI workflow that fails
permission: write
workflow: fake-ci-test-failure.yml
```

### Task 3: Validate and commit

**Files:**
- Verify: `.github/workflows/fake-ci-test-pass.yml`
- Verify: `.github/workflows/fake-ci-test-failure.yml`
- Verify: `.slash/fake-ci-test-pass.yml`
- Verify: `.slash/fake-ci-test-failure.yml`

- [ ] **Step 1: Validate Slash configuration**

Run:

```powershell
cargo run -q -p slash-cli -- validate .slash
```

Expected: exit code 0.

- [ ] **Step 2: Validate workflow structure**

Run a YAML parser over all four files and assert:

- each workflow has only `workflow_dispatch`;
- each workflow declares all five injected inputs;
- both jobs use `ubuntu-latest`;
- the failure workflow contains `exit 1`;
- command names and workflow filenames match.

- [ ] **Step 3: Commit**

```powershell
git diff --check
git add .github/workflows/fake-ci-test-pass.yml .github/workflows/fake-ci-test-failure.yml .slash/fake-ci-test-pass.yml .slash/fake-ci-test-failure.yml
git commit -m "test: add Slash live smoke commands"
```

After these files reach `main`, test them from a pull request with separate
comments containing `/fake-ci-test-pass` and `/fake-ci-test-failure`.
