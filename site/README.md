# Slash documentation site

The documentation site uses [Nimbus](https://nimbus-docs.com/) and deploys
to GitHub Pages at <https://henryzhang-zhy.github.io/slash/>.

Published Markdown lives in `src/content/docs/`, following Nimbus's standard
project layout.

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
npm run lint:docs
npm run build
```

## Nimbus patch

Nimbus is pinned to `0.10.0`. `patch-package` applies
`patches/@cloudflare+nimbus-docs+0.10.0.patch` after installation to fix:

- Pagefind startup on Windows with Node.js 24.
- Favicon and Shiki stylesheet URLs under an Astro base path.
- Home-page metadata detection under an Astro base path.

Re-evaluate and remove the patch when upgrading Nimbus. A clean `npm ci`
followed by `npm run build` verifies that the patch still applies.
