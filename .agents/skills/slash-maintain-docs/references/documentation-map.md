# Slash documentation map

Use this map only after identifying the changed concept and audience. Keep one detailed owner for each fact and link from the other surfaces.

## Repository tiers

| Location | Audience and job | Avoid |
| --- | --- | --- |
| `AGENTS.md` | Short standing orders for every repository agent | Procedures, examples, product contracts |
| `README.md` | Product entry point, project status, development bootstrap, owner links | Complete setup guides, API inventories, architecture rationale |
| `docs/architecture.md` | Current system composition, trust boundaries, lifecycle and release invariants | Per-endpoint detail, UI walkthroughs, implementation plans |
| `docs/design/*.md` | Durable topic contract, rationale, constraints, and accepted tradeoffs | Competing owners, temporary milestones, PR history |
| `docs/test-engine.md` | Test Engine architecture | General product setup or UI instructions |
| `docs/user/*.md` | Maintainer exercises and operational smoke procedures | Public product reference that belongs on the site |
| `site/src/content/docs/en/` | Canonical public documentation | Maintainer process and implementation internals |
| `site/src/content/docs/zh-hans/` | Reviewed Simplified Chinese counterparts | Chinese-only pages, translated identifiers, `/zh/` routes |
| `site/README.md` | Site development, localization layout, Nimbus patch maintenance | Product behavior already owned by public pages |
| `docs/design/documentation-internationalization.md` | Locale, routing, fallback, metadata, and discovery contract | Per-page translations or routine authoring instructions |
| Source comments and Rust/TypeScript API docs | Local obligations, invariants, failure and ownership behavior | Restated control flow or repository-wide rationale |

## Product-to-document routing

| Changed surface | Inspect these public owners | Inspect these durable owners |
| --- | --- | --- |
| Command YAML, parsing, inputs, trusted fields | `getting-started.mdx`, `slash-config-reference.mdx`, `workflow-requirements.mdx` | `docs/architecture.md` and the owning design |
| GitHub permissions and authorization | `permissions.mdx`, `api/platform.mdx` | `docs/design/github-command-authorization.md` |
| Registration, sessions, GitHub identity, teams | `api/authentication.mdx`, `web-console.mdx` | `docs/design/authentication.md` |
| Personal access tokens | `api/access-tokens.mdx`, `api/index.mdx`, `web-console.mdx` | `docs/design/personal-access-tokens.md` |
| Admin console, webhook inspection, installation reconciliation | `web-console.mdx`, `self-host/deployment.mdx`, relevant API index text | `docs/design/admin-console.md`, `docs/architecture.md` |
| Test result ingestion and quarantine | `api/ingestion.mdx`, `api/test-engine.mdx`, `test-engine.mdx`, `web-console.mdx` | `docs/test-engine.md`, `docs/architecture.md` |
| REST route, authentication, body, or error behavior | Owning `api/*.mdx` page and `api/index.mdx` inventory | Relevant design only when the durable contract changes |
| Deployment, secrets, environment, image, release, IaC | `self-host/index.mdx`, `self-host/deployment.mdx`, `limitations.mdx` | `docs/architecture.md` and relevant design |
| Console capability or user workflow | `web-console.mdx` and linked API page | Relevant design when trust or lifecycle changes |
| Documentation locale, route, search, metadata, Nimbus patch | `site/README.md` and affected locale pages | `docs/design/documentation-internationalization.md` |

Every public owner above means the same relative file under both `en/` and `zh-hans/` when a Chinese counterpart exists.

## Rename and removal sweep

Search before and after a rename or removal:

```sh
rg -n 'old-name|old-route|old-setting' . \
  --glob '!site/dist/**' \
  --glob '!site/node_modules/**' \
  --glob '!web/node_modules/**' \
  --glob '!target/**'
```

Check Markdown and MDX links, navigation indexes, examples, Web UI labels, API clients, environment templates, workflows, and IaC. A route move is complete only when both locale paths and every inbound reference agree.

## Documentation gate

Run from the repository root:

```sh
mise run check:site
git diff --check
```

The mise task is the authoritative aggregate. Use individual commands from `site/package.json` only for focused iteration. The repository uses npm; do not copy the pnpm commands in the generic Nimbus `site/AGENT.md` examples.
