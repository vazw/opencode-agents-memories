// W5.1 `js` shard — OpenCode plugin-loader contract, mirrored from
// example/opencode-mem/tests/plugin-loader-contract.test.ts:
//   type PluginModule = { id?: string; server: Plugin }
// The wrapper must load WITHOUT an engine (fail-open minimum, D-38): server()
// returns the two hooks it implements and never spawns a child at load time.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");
const distEntry = path.join(pkgRoot, "dist", "plugin.js");
const pkg = JSON.parse(readFileSync(path.join(pkgRoot, "package.json"), "utf8"));

function loadPlugin() {
  return import(distEntry).then((mod) => mod.default);
}

test("test_loader_default_export_object", async () => {
  const plugin = await loadPlugin();
  assert.equal(typeof plugin, "object");
  assert.notEqual(plugin, null);
});

test("test_loader_server_is_function", async () => {
  const plugin = await loadPlugin();
  assert.equal(typeof plugin.server, "function");
});

test("test_loader_id_matches_package_name", async () => {
  const plugin = await loadPlugin();
  assert.ok(pkg.name && pkg.name.length > 0, "package.json must carry a name");
  assert.equal(plugin.id, pkg.name);
});

test("test_loader_exports_server", () => {
  assert.ok(pkg.exports, "package.json must carry an exports map");
  assert.ok(pkg.exports["./server"] !== undefined, 'exports["./server"] must be defined');
});

test("test_loader_source_regex", () => {
  const src = readFileSync(path.join(pkgRoot, "src", "plugin.js"), "utf8");
  assert.ok(
    src.includes('import pkg from "../package.json" with { type: "json" }'),
    "src/plugin.js must import package.json with a JSON import attribute"
  );
});

test("test_loader_server_returns_hooks_without_engine", { timeout: 10000 }, async () => {
  const plugin = await loadPlugin();
  const result = await plugin.server({
    client: null,
    project: null,
    directory: pkgRoot,
    worktree: null,
    $: null,
  });
  assert.ok(result && typeof result === "object");
  assert.equal(typeof result.hooks["chat.message"], "function");
  assert.equal(typeof result.hooks["event"], "function");
});

// W5.5 (issue 33): the published capability list names the one new hook and
// NOT `experimental.compaction.autocontinue` — that hook is a recorded non-use
// (ADR-0017 DECISION 10), not a silent omission.
test("test_loader_manifest_declares_the_compacting_hook_and_not_autocontinue", () => {
  assert.ok(
    pkg.opencode.hooks.includes("experimental.session.compacting"),
    "package.json opencode.hooks must declare the pre-compaction hook"
  );
  assert.ok(
    !pkg.opencode.hooks.includes("experimental.compaction.autocontinue"),
    "experimental.compaction.autocontinue is deliberately not registered"
  );
});
