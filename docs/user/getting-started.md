# Getting started

Slash is a GitHub App: comment `/deploy staging` on a pull request, and Slash
triggers a `workflow_dispatch` on your own GitHub Actions and syncs its status
back to the PR as a check run.

## 1. Deploy the server

Slash is a single stateless container plus Postgres — see
[deployment.md](deployment.md) for the container image, environment
variables, and the reference single-replica deployment. You need a public
HTTPS URL for the webhook endpoint (`https://<your-host>/webhook`) before
creating the GitHub App in step 2.

## 2. Create the GitHub App

Create a GitHub App (either on your personal account or an organization)
with:

- **Webhook URL**: `https://<your-host>/webhook`
- **Webhook secret**: a random value — this becomes `SLASH_WEBHOOK_SECRET`
- **Permissions**: `checks: write`, `actions: write`, `contents: read`,
  `pull_requests: write`, `issues: write`, `metadata: read` (see
  [permissions.md](permissions.md) for why each one is needed —
  `issues: write` in particular is easy to under-scope and silently breaks
  everything)
- **Subscribe to events**: `issue_comment`, `workflow_run`, `check_run`,
  `pull_request`

Generate a private key for the App and save it as a `.pem` file — this
becomes `SLASH_GITHUB_PRIVATE_KEY_PATH`. Note the App ID — this becomes
`SLASH_GITHUB_APP_ID`.

Install the App on the repository (or repositories) you want it to run
commands in.

## 3. Configure a command

Add `.slash/echo.yml` to your repository's **default branch**:

```yaml
command: echo
permission: write
workflow: echo.yml
args:
  - name: message
    free_text: true
```

See [slash-config-reference.md](slash-config-reference.md) for the full
schema, and run `slash validate .slash/` in your own CI to catch mistakes
before they reach the default branch (see
[limitations.md](limitations.md) — Slash does not validate `.slash/` on
push itself in 0.0.1).

## 4. Add the workflow

Add `.github/workflows/echo.yml`, also on the default branch (GitHub only
resolves `workflow_dispatch` targets registered there — see
[workflow-requirements.md](workflow-requirements.md)):

```yaml
name: echo
on:
  workflow_dispatch:
    inputs:
      message: { required: false }
      slash_actor: { required: false }
jobs:
  echo:
    runs-on: ubuntu-latest
    steps:
      - name: Echo
        env:
          MESSAGE: ${{ inputs.message }}
          ACTOR: ${{ inputs.slash_actor }}
        run: echo "$ACTOR says: $MESSAGE"
```

A complete, copy-pasteable version of both files lives in
[`examples/`](../../examples/).

## 5. Try it

Open a pull request and comment:

```
/echo hello world
```

You should see a 🚀 reaction on the comment within a couple of seconds, a
`slash/echo` check run appear on the PR, and it reach a terminal conclusion
once the workflow run finishes, with a link to that run.

## Next steps

- [slash-config-reference.md](slash-config-reference.md) — the full
  `.slash/*.yml` schema
- [workflow-requirements.md](workflow-requirements.md) — injected inputs,
  the `env:`-not-`${{ }}` rule, required-status-check caveats
- [permissions.md](permissions.md) — who can invoke what
- [deployment.md](deployment.md) — running the server, key rotation, incident
  procedure
- [limitations.md](limitations.md) — what 0.0.1 deliberately does not do yet
