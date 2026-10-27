// W5.2 `js` shard — bundle boundary, mirrored from
// example/opencode-mem/tests/plugin-bundle-boundary.test.ts and inverted in
// place by W5.2 (blueprint D5(b), same intent, widened bound). The shipped
// artifact is the `dist/` tree the host imports: no npm dependencies; relative
// modules inside the package are allowed; `index.js` re-exports the bundle and
// stays outside the import rule. (Before W5.2 this file pinned `dist/` to
// exactly `plugin.js`; the shipped set is now the entries and their
// byte-identical twins, so the assertion walks the tree recursively, keeps
// files only, and normalises separators.)

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");

function filesUnder(rootName) {
  const root = path.join(pkgRoot, rootName);
  return readdirSync(root, { recursive: true })
    .map((entry) => `${rootName}/${String(entry).split(path.sep).join("/")}`)
    .filter((relative) => statSync(path.join(pkgRoot, relative)).isFile())
    .sort();
}

test("test_bundle_only_shipped_entries", () => {
  const shipped = filesUnder("dist").map((relative) => relative.slice("dist/".length));
  assert.deepEqual(
    shipped,
    [
      "plugin.js",
      "tui.js",
      "v2/adapter.js",
      "v2/legacy-client.js",
      "v2/plugin.js",
      "v2/toast-rpc.js",
    ],
    "plugin/dist must ship exactly the entrypoints and their byte-identical twins — no node_modules, no .ts, no .node"
  );
});

test("test_bundle_no_external_imports", () => {
  const modules = [...filesUnder("src"), ...filesUnder("dist")].filter((relative) =>
    relative.endsWith(".js")
  );
  assert.ok(modules.length > 0, "the scan must have found the package's modules");

  for (const rel of modules) {
    const text = readFileSync(path.join(pkgRoot, rel), "utf8");
    const specifiers = [];
    const re = /(?:\bfrom\s+|\bimport\s*\(?\s*)(["'])([^"']+)\1/g;
    let match;
    while ((match = re.exec(text)) !== null) specifiers.push(match[2]);
    for (const spec of specifiers) {
      assert.ok(
        spec.startsWith("node:") || spec.startsWith("./") || spec.startsWith("../"),
        `${rel}: imports non-builtin, non-relative module ${spec}`
      );
    }
  }
});
