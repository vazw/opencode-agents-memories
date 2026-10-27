// W5.2 `js` shard — the V2 plugin-loader contract, ported (and merged) from
// example/opencode-mem/tests/plugin-v2-loader-contract.test.ts. Five tests:
// the two upstream export-map cases merge into one, the two entry-shape cases
// port 1:1, and the fifth is the port's own addition — upstream's loader
// contract requires the installable entry to expose BOTH `server` and `setup`,
// which the port satisfies with a delegating `setup` (blueprint D1).
//
// These cases import `dist/`, like `loader-contract.test.mjs` and for the same
// reason: `dist/` is what the host loads. Upstream's `types` keys are N-A (the
// port has no TypeScript build); the dist byte-identity invariant
// (`dist-sync-invariant.test.mjs`) is the port's replacement for the build step.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");
const pkg = JSON.parse(readFileSync(path.join(pkgRoot, "package.json"), "utf8"));
const distEntry = path.join(pkgRoot, "dist", "plugin.js");
const v2Entry = path.join(pkgRoot, "dist", "v2", "plugin.js");
const tuiEntry = path.join(pkgRoot, "dist", "tui.js");

test("test_v2_loader_contract_exports_map_declares_v2_and_tui", () => {
  assert.ok(pkg.exports, "package.json must carry an exports map");
  // Same `import` + `default` shape as `./server`; no `types` key — the port
  // has no TypeScript build.
  assert.deepEqual(pkg.exports["./v2"], {
    import: "./dist/v2/plugin.js",
    default: "./dist/v2/plugin.js",
  });
  assert.deepEqual(pkg.exports["./tui"], {
    import: "./dist/tui.js",
    default: "./dist/tui.js",
  });
  assert.deepEqual(pkg.exports["./server"], {
    import: "./dist/plugin.js",
    default: "./dist/plugin.js",
  });
  assert.ok(pkg.files.includes("dist"), "the published files must carry the shipped bundle");
});

test("test_v2_loader_contract_v2_entry_is_a_plugin_definition", async () => {
  const mod = await import(v2Entry);
  assert.equal(mod.default.id, "opencode-mem");
  assert.equal(typeof mod.default.setup, "function");
  assert.equal(mod.id, "opencode-mem", "the id is also a named export");
  assert.equal(typeof mod.setup, "function", "setup is also a named export");
});

test("test_v2_loader_contract_tui_entry_is_a_companion_definition", async () => {
  const mod = await import(tuiEntry);
  assert.equal(mod.default.id, "opencode-mem-tui");
  assert.equal(typeof mod.default.setup, "function");
  assert.equal(mod.id, "opencode-mem-tui", "the id is also a named export");
  assert.equal(typeof mod.setup, "function", "setup is also a named export");
});

test("test_v2_loader_contract_main_entry_exposes_server_and_setup", async () => {
  const mod = await import(distEntry);
  assert.equal(mod.default.id, "opencode-mem");
  assert.equal(typeof mod.default.server, "function", "the V1 host contract is preserved");
  assert.equal(typeof mod.default.setup, "function", "the V2 host contract is exposed");
});

test("test_v2_loader_contract_setup_delegates_to_the_v2_entry", async () => {
  const main = await import(distEntry);
  const v2 = await import(v2Entry);

  assert.equal(typeof main.setup, "function");
  assert.notEqual(
    main.setup,
    v2.setup,
    "the main entry delegates through a deferred import; it does not re-export the V2 module"
  );
  // A host that is not V2 is refused by the V2 adapter's own precondition, so
  // the rejection proves the delegation actually executed the V2 entry.
  await assert.rejects(
    () => main.setup({ session: {} }),
    /v2 adapter requires ctx\.session\.hook/,
    "main.setup must reach dist/v2/plugin.js's setup"
  );

  // A valid-enough V2 ctx through the MAIN entry: the delegation must return
  // the adapter's cleanup — a same-message local throw does not satisfy this —
  // and a registry that refuses the toast descriptor must degrade, not fail.
  const ctx = {
    location: { directory: "/workspace/project" },
    session: { hook: async () => {} },
    rpc: {
      register: async () => {
        throw new Error("registry refused the descriptor");
      },
    },
  };
  const cleanup = await main.setup(ctx);
  assert.equal(typeof cleanup, "function", "a V2 host gets the adapter's cleanup");
  await cleanup();
});
