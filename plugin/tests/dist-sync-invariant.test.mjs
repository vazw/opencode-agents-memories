// W5.2 `js` shard — the dist-sync invariant, widened from the W5.1 single-file
// pin (`dist-sync.test.mjs` still owns `plugin.js`). The host imports `dist/`,
// so `dist/` must be a byte-identical mirror of `src/`: every module has a twin
// at the same relative path, the two file sets are equal (no extra file, no
// `.ts`, no `.node`, no `node_modules`), and each pair compares equal as raw
// Buffers — dist drift is a silent outage, not a lint failure.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");

// Files only, at any depth, with forward-slash relative paths so the two trees
// are comparable on every platform.
function walkFiles(root) {
  const found = [];
  for (const entry of readdirSync(root, { recursive: true })) {
    const relative = String(entry).split(path.sep).join("/");
    if (statSync(path.join(root, relative)).isFile()) found.push(relative);
  }
  return found.sort();
}

test("test_every_src_module_has_a_byte_identical_dist_twin", () => {
  const srcFiles = walkFiles(path.join(pkgRoot, "src"));
  const distFiles = walkFiles(path.join(pkgRoot, "dist"));

  assert.deepEqual(
    distFiles,
    srcFiles,
    "dist/ must mirror src/ exactly — a missing twin ships a stale wrapper"
  );
  for (const relative of srcFiles) {
    assert.ok(
      relative.endsWith(".js"),
      `src/${relative}: only .js modules are shipped (no .ts, no .node, no node_modules)`
    );
    const src = readFileSync(path.join(pkgRoot, "src", relative));
    const dist = readFileSync(path.join(pkgRoot, "dist", relative));
    assert.ok(dist.equals(src), `dist/${relative} must be byte-identical to src/${relative}`);
  }
});
