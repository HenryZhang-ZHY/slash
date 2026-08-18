# Slash

Slash commands for GitHub pull requests, done right.

Comment `/deploy staging` on a PR — Slash triggers your GitHub Actions workflow
and syncs its status back to the PR as a check run, so it shows up in the PR
checks section and works with branch protection. No dispatcher workflows, no
PAT plumbing, nothing to deploy: Slash is a hosted GitHub App control plane;
execution happens on your own GitHub Actions runners.

```yaml
# .slash/deploy.yml
command: deploy
permission: write
workflow: deploy.yml
args:
  - name: env
    required: true
    choices: [staging, production]
```

## Getting started

Read the [Slash documentation](https://henryzhang-zhy.github.io/slash/), or
copy the working example in [`examples/`](examples/) straight into your
repository. The documentation source lives in
[`site/src/content/docs/`](site/src/content/docs/).

## Status

In development. Command dispatch, GitHub-backed authorization, the web console, and the
Test Engine (test result collection, flaky detection, and auto-quarantine)
are implemented. The remaining step is demonstrating the design's success
criteria end-to-end against a live GitHub App on a real repository (see
[site/src/content/docs/limitations.mdx](site/src/content/docs/limitations.mdx)).

- [Product & technical spec](docs/design/0.0.1-spec.md)
- [Implementation plan](docs/design/0.0.1-implementation-plan.md)
- [User documentation](site/src/content/docs/)

## License

See [LICENSE](LICENSE).
