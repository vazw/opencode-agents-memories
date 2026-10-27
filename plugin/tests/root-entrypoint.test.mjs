// W5.1 `js` shard — root entrypoint contract, mirrored from
// example/opencode-mem/tests/plugin-root-entrypoint.test.ts: OpenCode loads a
// directory plugin by falling back to `<dir>/index`, so a local checkout needs
// index.js resolving to the same default export as dist/plugin.js.
// Both host contracts are exposed: the V1 `server` and the delegating V2
// `setup` (see dist/v2/plugin.js).

import test from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");
const indexPath = path.join(pkgRoot, "index.js");
const distEntry = path.join(pkgRoot, "dist", "plugin.js");

test("test_root_entrypoint_index_exists", () => {
  assert.ok(existsSync(indexPath), "index.js must exist at the package root");
});

test("test_root_entrypoint_id_matches_dist", async () => {
  const root = await import(indexPath);
  const dist = await import(distEntry);
  assert.ok(root.default !== undefined, "index.js must have a default export");
  assert.ok(dist.default !== undefined, "dist/plugin.js must have a default export");
  assert.equal(root.default.id, dist.default.id);
  assert.equal(root.default.id, "opencode-mem");
  assert.equal(typeof root.default.server, "function");
});
