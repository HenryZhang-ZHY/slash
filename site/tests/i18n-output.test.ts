import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

function builtPage(path: string): string {
  return readFileSync(new URL(`../dist/${path}/index.html`, import.meta.url), "utf8");
}

test("renders fully localized Simplified Chinese document chrome", () => {
  const html = builtPage("zh-hans/getting-started");

  assert.match(html, /<html lang="zh-Hans">/);
  assert.match(html, />跳到正文</);
  assert.match(html, /aria-label="搜索文档"/);
  assert.match(html, /placeholder="筛选…"/);
  assert.match(html, />本页内容</);
  assert.match(html, />下一页</);
  assert.match(html, /href="\/slash\/zh-hans\/"[^>]*class="group flex items-center gap-2"/);
  assert.doesNotMatch(html, />Skip to content</);
  assert.doesNotMatch(html, /aria-label="Search documentation"/);
  assert.doesNotMatch(html, />On this page</);
});

test("renders a compact same-page language menu", () => {
  const html = builtPage("zh-hans/getting-started");

  assert.match(html, /aria-label="选择语言"/);
  assert.match(html, /href="\/slash\/en\/getting-started\/"/);
  assert.match(html, /data-locale-option="en"/);
  assert.match(html, /data-locale-option="zh-Hans"/);
});

test("scopes the search index to the page language", () => {
  const english = builtPage("en/getting-started");
  const simplifiedChinese = builtPage("zh-hans/getting-started");

  assert.match(english, /data-pagefind-filter="[^"]*language:en/);
  assert.match(simplifiedChinese, /data-pagefind-filter="[^"]*language:zh-hans/);
});
