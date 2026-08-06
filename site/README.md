# Slash documentation site

The documentation site uses [Nimbus](https://nimbus-docs.com/) and deploys
to GitHub Pages at <https://henryzhang-zhy.github.io/slash/>.

Published Markdown lives in `src/content/docs/`, following Nimbus's standard
project layout.

## Internationalization

English source documents live in `src/content/docs/en/`. Simplified Chinese
translations live at the same relative path in `src/content/docs/zh-hans/`.
For example:

```text
src/content/docs/en/api/authentication.mdx
src/content/docs/zh-hans/api/authentication.mdx
```

English pages may ship before their translation. The build generates a
non-indexable `/zh-hans/` fallback that clearly displays the English source;
adding the matching translation replaces it automatically. A Chinese page
without an English source fails `npm test`.

Site chrome and homepage strings are centralized in `src/lib/i18n.ts`. Do not
hard-code locale prefixes or user-facing navigation strings in components.
Use its URL helpers so the `/slash` base path, query strings, and fragments are
preserved.

## Development

Node.js 24 and npm are required.

```sh
npm ci
npm run dev
```

The local site is served below `/slash/`, matching the GitHub Pages project
path. Before opening a pull request, run:

```sh
npm run typecheck
npm test
npm run lint:docs
npm run build
npm run test:output
```

## Nimbus patch

Nimbus is pinned to `0.10.0`. `patch-package` applies
`patches/@cloudflare+nimbus-docs+0.10.0.patch` after installation to fix:

- Pagefind startup on Windows with Node.js 24.
- Favicon and Shiki stylesheet URLs under an Astro base path.
- Home-page metadata detection under an Astro base path.
- Per-page BCP 47, Open Graph locale, and canonical overrides needed by the
  localized layout and generated translation fallbacks.

Re-evaluate and remove the patch when upgrading Nimbus. A clean `npm ci`
followed by `npm run build` verifies that the patch still applies.
