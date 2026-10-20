// W5.1 `js` shard — bundle boundary, mirrored from
// example/opencode-mem/tests/plugin-bundle-boundary.test.ts. The shipped
// artifact is the plugin-host bundle: dist/ must contain exactly plugin.js, and
// both the source and the built artifact may import ONLY node builtins plus the
// package.json manifest (no npm dependencies, no local modules). index.js is
// excluded from the import rule on purpose — it re-exports the bundle.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");

test("test_bundle_only_plugin_js", () => {
  const entries = readdirSync(path.join(pkgRoot, "dist"));
  assert.deepEqual(
    entries.sort(),
    ["plugin.js"],
    "plugin/dist must contain exactly plugin.js — no node_modules, no .ts, no .node"
  );
});

test("test_bundle_no_external_imports", () => {
  for (const rel of ["dist/plugin.js", "src/plugin.js"]) {
    const text = readFileSync(path.join(pkgRoot, rel), "utf8");
    const specifiers = [];
    const re = /(?:\bfrom\s+|\bimport\s*\(?\s*)(["'])([^"']+)\1/g;
    let match;
    while ((match = re.exec(text)) !== null) specifiers.push(match[2]);
    assert.ok(specifiers.length > 0, `${rel}: expected at least one import`);
    for (const spec of specifiers) {
      assert.ok(
        spec.startsWith("node:") || spec === "../package.json",
        `${rel}: imports non-builtin module ${spec}`
      );
    }
  }
});
