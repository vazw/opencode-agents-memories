// Issue 75 - upstream tests/plugin-host-config.test.ts:177,
// "registers a step-capped least-privilege agent". The port's seam is the
// host-hook surface: drive `hooks.config` with a config object and assert the
// STRUCTURED_OUTPUT_AGENT entry field-for-field.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");
const repoRoot = path.resolve(pkgRoot, "..");
const AGENT = "opencode-mem-structured";

test("test_config_hook_registers_the_step_capped_structured_output_agent", { timeout: 10000 }, async () => {
  const distEntry = pathToFileURL(path.join(pkgRoot, "dist", "plugin.js")).href;
  const plugin = (await import(distEntry)).default;
  const { hooks } = await plugin.server({ client: null, project: null, directory: pkgRoot, worktree: null, $: null });

  assert.equal(typeof hooks.config, "function", "the wrapper must register a config hook");

  const cfg = { agent: { build: { mode: "primary" } } };
  await hooks.config(cfg);

  assert.deepEqual(cfg.agent.build, { mode: "primary" }, "the neighbour entry survives (spread, not replace)");
  assert.deepEqual(cfg.agent[AGENT], {
    description: "Internal least-privilege agent for opencode-mem structured output",
    mode: "subagent",
    steps: 2,
    maxSteps: 2,
    tools: { "*": false, StructuredOutput: true },
    permission: { "*": "deny", StructuredOutput: "allow" },
    options: { thinking: { type: "disabled" } },
  });

  // Cross-process contract pin: the engine's request body names the same agent
  // and the same step cap (cannot be executed from node; asserted as source).
  const rust = readFileSync(
    path.join(repoRoot, "crates", "mem-ai", "src", "provider", "opencode_host.rs"),
    "utf8"
  );
  assert.ok(rust.includes('pub const STRUCTURED_OUTPUT_AGENT: &str = "opencode-mem-structured";'));
  assert.ok(rust.includes("pub const STRUCTURED_OUTPUT_MAX_STEPS: u32 = 2;"));
});
