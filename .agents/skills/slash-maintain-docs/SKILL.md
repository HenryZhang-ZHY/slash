---
name: slash-maintain-docs
description: Use when creating, changing, moving, reviewing, or auditing documentation in the Slash repository; when code changes affect public behavior, REST APIs, configuration, authentication, administration, Web UI, deployment, or GitHub App operation; when maintaining Nimbus pages, English and Simplified Chinese content, routes, navigation, metadata, or links; or when diagnosing documentation build, lint, link, and localization drift.
---

# Maintaining Slash Documentation

Keep documentation aligned with shipped behavior without duplicating facts across architecture, design records, public guides, and source comments. Treat this skill as a routing and verification workflow, not a prose template.

## Establish authority and scope

Read `AGENTS.md`, `docs/architecture.md`, the relevant `docs/design/` owner, `site/README.md`, and `.agents/skills/pre-push-checks/SKILL.md`. For site implementation work, also read `site/AGENT.md`; its Nimbus examples are framework guidance, while `site/README.md` and `site/package.json` own Slash's npm commands and pinned runtime.

Compare the requested change or branch with its verified base. Follow changed concepts across Rust routes and configuration, migrations, Web UI/API clients and translations, workflows, examples, IaC, and both documentation locales. Read the implementation before asserting current behavior. If implementation and durable documentation disagree, determine which one the task intends to change and repair the contradiction in the same work.

Read [the documentation map](references/documentation-map.md) when choosing a document owner, changing a public route, or tracing a vertical slice with more than one documentation surface.

## Choose the document form before writing

Use a tutorial for ordered work that starts from stated prerequisites and ends in an observable result. Introduce only the concepts needed by each step and link optional detail to its owner. Use a reference for scoped lookup of current fields, endpoints, configuration, semantics, and failure behavior; do not make readers follow a teaching sequence to find one fact. Split substantial mixed forms, while a small secondary form may remain under a clearly named section.

Keep full detail about the document's own subject. Describe child topics only by purpose and high-level behavior, then link to their owning pages.

## Give each fact one owner

Place a fact where readers need its full detail and link to that owner elsewhere:

- Keep repository-wide invariants and the current system map in `docs/architecture.md`.
- Keep durable topic contracts and rationale in the existing `docs/design/<topic>.md`; extend an owner instead of creating a competing design file.
- Keep maintainer-only exercises in `docs/user/`.
- Keep public setup, operation, API, and console behavior under `site/src/content/docs/<locale>/`.
- Keep site authoring, Nimbus patch, and localization mechanics in `site/README.md` or `docs/design/documentation-internationalization.md`.
- Keep local caller obligations, failure behavior, security constraints, and surprising ownership in source comments or API docs. Do not restate code.
- Keep root `README.md` concise: product entry point, development bootstrap, and links to the owners above.

Do not put change history, PR narration, temporary status, test inventories, or implementation walkthroughs in durable documentation. State the current behavior and preserve non-obvious rationale only at its owner.

## Update the complete vertical slice

1. Name the changed behavior and its audiences: end user, REST client, self-hoster, operator, contributor, or maintainer.
2. Find every existing claim about that behavior with `rg`, including old names, routes, environment variables, JSON fields, UI labels, and translated phrases.
3. Update the owning source first, then summaries, examples, links, navigation, and translations that project it.
4. Keep security properties explicit: required permissions, authentication mode, file-only secret handling, failure behavior, rate limits, durability, and retry semantics must not become vague during condensation.
5. Remove stale claims rather than retaining compatibility prose for unsupported behavior.
6. Check the final diff for duplicate explanations and facts left in the wrong tier.

For an API change, verify the endpoint inventory, authentication table, request and response examples, error behavior, and the Web console page when it consumes the endpoint. For configuration or deployment changes, verify the environment-variable table, compose/IaC examples, secret-file rules, startup failure behavior, and self-hosting instructions. For UI changes, document user-visible capability and limitations rather than component structure.

## Maintain localization deliberately

Treat English under `site/src/content/docs/en/` as the source locale. Use the same relative path under `site/src/content/docs/zh-hans/`; never restore `/zh/` or create another Chinese locale directory.

- When an English page already has a Chinese counterpart, update both in the same change and preserve equivalent propositions, headings, links, code, warnings, and modality.
- A new English page may ship without Chinese content; the tested fallback supplies a non-indexable Chinese route. Do not create placeholder translation files.
- Never create a Chinese page without its English source.
- Keep technical slugs language-neutral. Use locale helpers for site URLs and UI strings; do not hard-code locale prefixes in components.
- Preserve BCP 47 `zh-Hans` in language metadata and lowercase `zh-hans` in URL paths. Preserve the separate Open Graph locale mapping owned by `site/src/lib/i18n.ts`.
- Translate prose naturally without translating identifiers, endpoint paths, configuration keys, code, or literal UI/API values that the product owns.

When moving or deleting a page, move or delete both locale files when present, repair all inbound links and index pages, and search for the retired route across the repository.

## Preserve Nimbus source ownership

Edit canonical MDX and site source only. Never edit or commit `site/dist/`, `site/.astro/`, generated Pagefind output, `node_modules/`, or other build artifacts. Change the source or patch that generates behavior.

Use frontmatter accepted by Nimbus and do not repeat the frontmatter title as a body H1. Keep internal links locale-correct. Register MDX components through the existing Nimbus mechanism instead of bypassing it. Re-evaluate the pinned Nimbus patch when an upstream upgrade replaces one of its fixes.

## Validate the result

Run the smallest checks that prove each changed surface, then run the complete documentation gate before pushing:

```sh
mise run check:site
git diff --check
```

`check:site` runs source-level localization tests, the Astro build, type checking, Nimbus link/frontmatter lint, and generated-output regression tests. Add a focused `rg` sweep for renamed or removed routes, settings, endpoints, and terminology. Run the relevant Rust or Web gate when documentation changes accompany product behavior changes; documentation passing does not prove the implementation.

Before handoff, report the owning documents changed, public routes affected, locale coverage, stale references removed, and exact checks run. State any deliberate English-only fallback or unresolved contradiction.
