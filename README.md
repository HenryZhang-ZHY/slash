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

See [docs/user/getting-started.md](docs/user/getting-started.md), or copy the
working example in [`examples/`](examples/) straight into your repository.

## Status

Pre-release. Version 0.0.1 is implemented; the remaining step before tagging
a release is demonstrating the design's success criteria end-to-end against
a live GitHub App on a real repository (see
[docs/user/limitations.md](docs/user/limitations.md)).

- [Product & technical spec](docs/design/0.0.1-spec.md)
- [Implementation plan](docs/design/0.0.1-implementation-plan.md)
- [User documentation](docs/user/)

## License

See [LICENSE](LICENSE).
