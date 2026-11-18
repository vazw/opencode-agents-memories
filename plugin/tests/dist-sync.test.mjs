// W5.1 `js` shard — the committed artifact `dist/plugin.js` is what the OpenCode
// host actually imports, so it must be a byte-identical copy of `src/plugin.js`.
// Enforced with a raw Buffer compare (equivalent to running `cmp`).

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");

test("test_dist_plugin_byte_identical_to_src", () => {
  const src = readFileSync(path.join(pkgRoot, "src", "plugin.js"));
  const dist = readFileSync(path.join(pkgRoot, "dist", "plugin.js"));
  assert.ok(dist.equals(src), "dist/plugin.js and src/plugin.js must be byte-identical");
});

// Issue 149: the hand-maintained adapter pair must stay byte-identical. This
// passes against unmodified HEAD and must keep passing after the copy.
test("test_dist_v2_adapter_byte_identical_to_src", () => {
  const src = readFileSync(path.join(pkgRoot, "src", "v2", "adapter.js"));
  const dist = readFileSync(path.join(pkgRoot, "dist", "v2", "adapter.js"));
  assert.ok(dist.equals(src), "dist/v2/adapter.js and src/v2/adapter.js must be byte-identical");
});
