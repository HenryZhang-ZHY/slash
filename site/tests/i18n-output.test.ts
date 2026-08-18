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

test("emits locale-correct social, structured, and alternate metadata", () => {
  const html = builtPage("zh-hans/getting-started");

  assert.match(html, /<meta property="og:locale" content="zh_CN">/);
  assert.match(html, /"inLanguage":"zh-Hans"/);
  assert.match(html, /rel="alternate" hreflang="en" href="https:\/\/henryzhang-zhy\.github\.io\/slash\/en\/getting-started\/"/);
  assert.match(html, /rel="alternate" hreflang="zh-Hans" href="https:\/\/henryzhang-zhy\.github\.io\/slash\/zh-hans\/getting-started\/"/);
  assert.match(html, /rel="alternate" hreflang="x-default" href="https:\/\/henryzhang-zhy\.github\.io\/slash\/en\/getting-started\/"/);
  assert.doesNotMatch(html, /property="og:locale" content="en"/);
});

test("renders an accessible root language gateway instead of a meta refresh", () => {
  const html = builtPage("");

  assert.match(html, /^<!DOCTYPE html><html/);
  assert.match(html, /href="\/slash\/en\/"/);
  assert.match(html, /href="\/slash\/zh-hans\/"/);
  assert.match(html, /docs-locale/);
  assert.match(html, /navigator\.languages/);
  assert.doesNotMatch(html, /http-equiv="refresh"/);
});

test("uses localized homepage metadata and copy", () => {
  const html = builtPage("zh-hans");

  assert.match(html, /<meta name="description" content="正确实现 GitHub Pull Request 的 Slash Commands。">/);
  assert.match(html, />安装并连接 GitHub App</);
  assert.doesNotMatch(html, /Slash commands for GitHub pull requests, done right\./);
});
