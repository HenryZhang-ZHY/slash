import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import test from "node:test";

const siteRoot = new URL("../../", import.meta.url);
const docsRoot = new URL("src/content/docs/", siteRoot);

function collectFiles(root: URL, extension: string): string[] {
  const files: string[] = [];
  const rootPath = root.pathname;

  function visit(directory: string) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (path.endsWith(extension)) files.push(relative(rootPath, path));
    }
  }

  visit(rootPath);
  return files.sort();
}

test("requires every Simplified Chinese document to have an English source", () => {
  const english = collectFiles(new URL("en/", docsRoot), ".mdx");
  const simplifiedChinese = collectFiles(new URL("zh-hans/", docsRoot), ".mdx");

  assert.deepEqual(simplifiedChinese.filter((file) => !english.includes(file)), []);
  assert.equal(existsSync(new URL("zh/", docsRoot)), false);
});

test("does not retain links to the retired /zh/ routes", () => {
  const sourceRoot = new URL("src/", siteRoot);
  const files = collectFiles(sourceRoot, ".mdx");
  const legacyLinks = files.flatMap((file) => {
    const text = readFileSync(new URL(file, sourceRoot), "utf8");
    return text.includes("/zh/") ? [file] : [];
  });

  assert.deepEqual(legacyLinks, []);
});
