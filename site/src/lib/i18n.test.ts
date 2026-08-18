import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_LOCALE,
  getLocaleByEntryId,
  getLocaleByPath,
  getLocalePath,
  getTranslations,
} from "./i18n.ts";

test("uses short BCP 47 tags and a dedicated URL path", () => {
  assert.equal(DEFAULT_LOCALE, "en");
  assert.equal(getLocaleByEntryId("en/getting-started").languageTag, "en");
  assert.equal(getLocaleByEntryId("zh-hans/getting-started").languageTag, "zh-Hans");
  assert.equal(getLocaleByEntryId("zh-hans/getting-started").path, "zh-hans");
});

test("resolves locales behind the configured Astro base path", () => {
  assert.equal(getLocaleByPath("/slash/en/getting-started/", "/slash").languageTag, "en");
  assert.equal(getLocaleByPath("/slash/zh-hans/getting-started/", "/slash").languageTag, "zh-Hans");
  assert.equal(getLocaleByPath("/slash/", "/slash").languageTag, "en");
});

test("switches locale without losing the route, query, hash, base path, or trailing slash", () => {
  assert.equal(
    getLocalePath("/slash/en/api/?tab=tokens#authentication", "zh-Hans", "/slash"),
    "/slash/zh-hans/api/?tab=tokens#authentication",
  );
  assert.equal(
    getLocalePath("/slash/zh-hans/", "en", "/slash"),
    "/slash/en/",
  );
});

test("provides a complete localized chrome dictionary", () => {
  const en = getTranslations("en");
  const zh = getTranslations("zh-Hans");

  assert.equal(en.search.label, "Search documentation");
  assert.equal(zh.search.label, "搜索文档");
  assert.equal(zh.navigation.onThisPage, "本页内容");
  assert.equal(zh.pagination.previous, "上一页");
  assert.equal(zh.pageActions.viewMarkdown, "查看 Markdown");
});
