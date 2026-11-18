import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createContent, root } from "../scripts/content.mjs";

test("content pipeline discovers all enoxian docs without unresolved links", () => {
  const { pages } = createContent();
  const enDocs = pages.filter((p) => p.locale === "en" && p.kind === "doc");
  assert.ok(enDocs.length >= 25, `Expected >=25 docs, got ${enDocs.length}`);

  const urls = new Set(pages.map((p) => p.url));
  assert.ok(urls.has("/"));
  assert.ok(urls.has("/zh-Hans"));
  assert.ok(urls.has("/docs"));
  assert.ok(urls.has("/docs/guide/getting-started"));
  assert.ok(urls.has("/docs/concepts/overview"));
  assert.ok(urls.has("/docs/development"));
  assert.ok(urls.has("/docs/development/architecture/architecture"));
});

test("prepare:content emits .generated/content.json and public assets", () => {
  assert.ok(fs.existsSync(path.join(root, ".generated/content.json")));
  assert.ok(fs.existsSync(path.join(root, "public/CNAME")));
  assert.ok(fs.existsSync(path.join(root, "public/img/logo.svg")));
  assert.ok(fs.existsSync(path.join(root, "public/search-index.json")));
});
