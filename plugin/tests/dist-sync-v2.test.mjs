// W5.1 `js` shard — the committed artifact `dist/v2/adapter.js` is what the
// OpenCode host actually imports, so it must be a byte-identical copy of
// `src/v2/adapter.js`. Enforced with a raw Buffer compare (equivalent to running `cmp`).
// Mirrors dist-sync.test.mjs's plugin.js check for AC3 of issue 99.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");

test("test_dist_v2_adapter_byte_identical_to_src", () => {
  const src = readFileSync(path.join(pkgRoot, "src", "v2", "adapter.js"));
  const dist = readFileSync(path.join(pkgRoot, "dist", "v2", "adapter.js"));
  assert.ok(dist.equals(src), "dist/v2/adapter.js and src/v2/adapter.js must be byte-identical");
});
