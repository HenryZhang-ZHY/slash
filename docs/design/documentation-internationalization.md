# Documentation internationalization

Status: implemented (2026-08-19)

## Locale model

The documentation site remains a Nimbus site. A small site-owned adapter in
`site/src/lib/i18n.ts` is the single source of truth for locale identity,
localized UI strings, URL generation, and fallback planning.

English uses the BCP 47 tag `en` and the `/en/` path. Simplified Chinese uses
the BCP 47 tag `zh-Hans` and the lowercase `/zh-hans/` path. Open Graph's
separate locale syntax maps these to `en_US` and `zh_CN`. Technical document
slugs stay language-neutral and match across locale directories.

The root path is an `x-default` language gateway. It honors an explicit stored
choice first, then browser language preferences, and otherwise selects
English. Every locale page stays at an explicit, crawlable URL.

## Content lifecycle

English is the source locale. A translation uses the same relative path under
`site/src/content/docs/zh-hans/`. A Chinese document without an English source
fails the content inventory test.

An English document may ship before its translation. The static route planner
generates the missing Chinese URL with a localized notice and English content.
The fallback page is `noindex`, marks the article as English, canonicals to the
English source, and advertises only the locale versions that actually exist.
Adding the matching Chinese file replaces the fallback automatically.

## User and discovery surfaces

Nimbus chrome reads from the current page locale, including navigation,
search, table of contents, page actions, pagination, mobile controls, dates,
and accessibility labels. Search is filtered to the current Pagefind language
index. The compact language menu preserves the current document, query, and
fragment while remembering an explicit choice.

Each translated page emits a self canonical, reciprocal `hreflang` links, an
English `x-default`, a BCP 47 JSON-LD `inLanguage`, and the matching Open Graph
locale. The static 404 page is intentionally bilingual because GitHub Pages
serves one build-time 404 document for every missing URL.
