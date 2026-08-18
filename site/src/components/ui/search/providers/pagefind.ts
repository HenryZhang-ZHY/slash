import type { SearchProvider, SearchResult } from "@cloudflare/nimbus-docs/types";
import { config } from "virtual:nimbus/config";
import { withBasePath } from "@/lib/urls";
import { getLocaleByPath, getTranslations } from "@/lib/i18n";

interface PagefindSubResult {
  title?: string;
  url?: string;
}

interface PagefindResultData {
  url: string;
  excerpt?: string;
  meta?: { title?: string };
  sub_results?: PagefindSubResult[];
}

interface PagefindSearchResponse {
  results: Array<{ data(): Promise<PagefindResultData> }>;
}

interface PagefindFilters {
  [key: string]: string | string[] | { none?: string | string[]; any?: string | string[] };
}

interface PagefindApi {
  init(): Promise<void>;
  search(query: string, options?: { filters?: PagefindFilters }): Promise<PagefindSearchResponse>;
}

let pagefind: PagefindApi | undefined;

/**
 * Default Pagefind filters applied to every search.
 *
 * Versioning: when the site has a `versions.deprecated` list, the
 * layout emits `data-pagefind-filter="status:deprecated"` on every
 * deprecated-version page. Search defaults to excluding those results
 * (readers searching for "auth" want the current version's auth page,
 * not the deprecated one). Future UI work can expose a "include
 * deprecated" toggle; for now the default is current + non-deprecated.
 *
 * Versions are still searchable individually — readers on a v0 page
 * who explicitly search from there can opt the UI into a version-scoped
 * filter. The default exclusion is just for the top-level search.
 *
 * Computed at module-import time so we don't pay the config lookup on
 * every keystroke.
 */
function defaultFilters(): PagefindFilters {
  const locale = getLocaleByPath(window.location.pathname, import.meta.env.BASE_URL);
  const filters: PagefindFilters = { language: locale.path };
  if (config.versions?.deprecated?.length) filters.status = { none: "deprecated" };
  return filters;
}

export const provider: SearchProvider = {
  async init() {
    if (pagefind) return;
    const pagefindUrl = new URL(
      withBasePath("/pagefind/pagefind.js"),
      window.location.origin,
    );
    pagefind = (await import(/* @vite-ignore */ pagefindUrl.href)) as PagefindApi;
    await pagefind.init();
  },

  async search(query) {
    if (!pagefind) await this.init?.();
    if (!pagefind) return [];

    const search = await pagefind.search(
      query,
      { filters: defaultFilters() },
    );
    const results = await Promise.all(search.results.slice(0, 10).map((result) => result.data()));
    return results.map((result): SearchResult => ({
      title: result.meta?.title ?? getTranslations(
        getLocaleByPath(window.location.pathname, import.meta.env.BASE_URL).languageTag,
      ).search.untitled,
      url: withBasePath(result.url),
      snippet: result.excerpt,
      subResults: result.sub_results
        ?.filter((sub): sub is Required<PagefindSubResult> => Boolean(sub.title && sub.url))
        .map((sub) => ({ title: sub.title, url: withBasePath(sub.url) })),
    }));
  },
};
