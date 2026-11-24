import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { assertReaped, runDriver, stagePackageTree, stageStubEngine, tempRoot } from "./harness/bridge.mjs";

const DRIVER = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(path.join(process.argv[2], "dist/plugin.js")));
const { hooks } = await server({ client: {}, directory: "/work/project" });
const decisions = [];
const turn = () => hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [] } });
await turn(); // starts the child and runs hello
decisions.push(await hooks.shouldRefresh({ turnIndex: 1, hasCache: true }));
// Kill only fixture child. Replacement advertises no policy and must forget first.
fs.writeFileSync(process.env.HELLO_CONFIG, "{}");
const { pid } = JSON.parse(fs.readFileSync(process.env.MANIFEST, "utf8"));
process.kill(pid, "SIGTERM");
await new Promise(resolve => setTimeout(resolve, 100));
await turn(); // respawns: the replacement's hello advertises nothing
decisions.push(await hooks.shouldRefresh({ turnIndex: 2, hasCache: true }));
fs.writeFileSync(process.env.RESULT, JSON.stringify(decisions));
process.exit(0);
`;

test("test_hello_advertises_inject_on_outside_capabilities", { timeout: 30000 }, async (t) => {
  for (const inject_on of ["first", "always", undefined, "invalid"]) {
    const tmp = tempRoot("om-inject-delivery-");
    const pkgDir = stagePackageTree(tmp);
    const manifestPath = path.join(tmp, "manifest.json");
    const helloConfigPath = path.join(tmp, "hello.json");
    const resultPath = path.join(tmp, "result.json");
    fs.writeFileSync(helloConfigPath, JSON.stringify({ inject_on }));
    stageStubEngine(pkgDir, {
      requestLogPath: path.join(tmp, "requests.ndjson"), manifestPath,
      helloConfigPath, injectMode: "success",
      // Wrong location must never influence the policy.
      capabilities: { inject_on: "first" },
    });
    t.after(() => assertReaped(manifestPath));
    const driverPath = path.join(tmp, "driver.mjs");
    fs.writeFileSync(driverPath, DRIVER);
    const run = await runDriver(driverPath, [pkgDir], {
      ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      HELLO_CONFIG: helloConfigPath, MANIFEST: manifestPath, RESULT: resultPath,
    });
    assert.equal(run.code, 0, run.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(resultPath, "utf8")),
      [inject_on !== "first", true], String(inject_on));
  }
});

const ADAPTER_MODULE = "../src/v2/adapter.js";

function createAdapterHarness() {
  const hooks = new Map();
  const ctx = {
    location: { directory: "/workspace/project", project: { id: "project", directory: "/workspace/project" } },
    session: { hook: async (name, cb) => hooks.set(name, cb), get: async () => ({ location: { directory: "/workspace/project" } }) },
    event: { async *subscribe({ signal }) { await new Promise(r => signal.addEventListener("abort", r, { once: true })); } },
  };
  const contextOf = (sessionID) => {
    const ev = { sessionID, system: [] };
    return hooks.get("context")(ev).then(() => ev.system.map(p => p.text));
  };
  const prompt = (sessionID, messageID, text = "t") => hooks.get("prompt")({ sessionID, messageID, prompt: { text } });
  return { ctx, hooks, contextOf, prompt };
}

test("test_v2_context_refreshes_only_when_inject_on_says_so", { timeout: 10000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);

  // 1) first mode: two prompts same session -> one V1 call, block retained; distinct session isolated.
  {
    const h = createAdapterHarness();
    const calls = [];
    const blocks = ["s1-first", "s2-first"];
    const legacy = {
      "chat.message": async (inp, out) => {
        calls.push(inp.sessionID);
        const b = blocks.shift();
        if (b) out.message.parts.push({ type: "text", text: b, synthetic: true });
      },
      event: async () => {},
    };
    const seenArgs = [];
    Object.defineProperty(legacy, "shouldRefresh", {
      enumerable: false,
      value: (arg) => {
        seenArgs.push(arg);
        return !arg.hasCache || arg.turnIndex === 0;
      },
    });
    const cleanup = await registerV2Adapter(h.ctx, legacy);
    await h.prompt("ses-1", "m1");
    assert.deepEqual(await h.contextOf("ses-1"), ["s1-first"]);
    await h.prompt("ses-1", "m2");
    assert.deepEqual(await h.contextOf("ses-1"), ["s1-first"], "first mode retains block on turn 1");
    assert.deepEqual(calls, ["ses-1"], "only one V1 call for ses-1 across two prompts");
    assert.deepEqual(seenArgs, [
      { turnIndex: 0, hasCache: false },
      { turnIndex: 1, hasCache: true },
    ]);

    await h.prompt("ses-2", "m3");
    assert.deepEqual(await h.contextOf("ses-2"), ["s2-first"], "ses-2 has its own turnIndex 0 and cache");
    assert.deepEqual(await h.contextOf("ses-1"), ["s1-first"], "ses-1 isolated from ses-2");
    await cleanup();
    assert.deepEqual(await h.contextOf("ses-1"), [], "cleanup clears session maps");
  }

  // 2) always mode: two prompts -> two calls, changed blocks.
  {
    const h = createAdapterHarness();
    const blocks = ["b1", "b2"];
    const legacy = {
      "chat.message": async (_inp, out) => {
        const b = blocks.shift();
        if (b) out.message.parts.push({ type: "text", text: b, synthetic: true });
      },
      event: async () => {},
    };
    Object.defineProperty(legacy, "shouldRefresh", {
      enumerable: false,
      value: () => true,
    });
    const cleanup = await registerV2Adapter(h.ctx, legacy);
    await h.prompt("ses-1", "m1");
    assert.deepEqual(await h.contextOf("ses-1"), ["b1"]);
    await h.prompt("ses-1", "m2");
    assert.deepEqual(await h.contextOf("ses-1"), ["b2"]);
    await cleanup();
  }

  // 3) first mode with empty first response -> retries on second prompt (two calls).
  {
    const h = createAdapterHarness();
    let calls = 0;
    const blocks = [null, "recovered-on-second"];
    const legacy = {
      "chat.message": async (_inp, out) => {
        calls += 1;
        const b = blocks.shift();
        if (b) out.message.parts.push({ type: "text", text: b, synthetic: true });
      },
      event: async () => {},
    };
    Object.defineProperty(legacy, "shouldRefresh", {
      enumerable: false,
      value: ({ turnIndex, hasCache }) => !hasCache || turnIndex === 0,
    });
    const cleanup = await registerV2Adapter(h.ctx, legacy);
    await h.prompt("ses-1", "m1");
    assert.deepEqual(await h.contextOf("ses-1"), []);
    await h.prompt("ses-1", "m2");
    assert.deepEqual(await h.contextOf("ses-1"), ["recovered-on-second"], "empty first turn retries on next prompt");
    assert.equal(calls, 2);
    await cleanup();
  }

  // 4) overlapping same-session prompts: serialized so older completion cannot overwrite newer, while another session runs independently.
  {
    const h = createAdapterHarness();
    let releaseFirst;
    const firstGate = new Promise(r => { releaseFirst = r; });
    let firstStarted;
    const startedP = new Promise(r => { firstStarted = r; });
    const order = [];
    const legacy = {
      "chat.message": async (inp, out) => {
        const tag = `${inp.sessionID}:${out.message.id}`;
        order.push(`start:${tag}`);
        if (tag === "ses-1:m1") {
          firstStarted();
          await firstGate;
          out.message.parts.push({ type: "text", text: "stale-m1", synthetic: true });
        } else if (tag === "ses-1:m2") {
          out.message.parts.push({ type: "text", text: "fresh-m2", synthetic: true });
        } else {
          out.message.parts.push({ type: "text", text: "ses2-block", synthetic: true });
        }
        order.push(`end:${tag}`);
      },
      event: async () => {},
    };
    Object.defineProperty(legacy, "shouldRefresh", { enumerable: false, value: () => true });
    const cleanup = await registerV2Adapter(h.ctx, legacy);
    const p1 = h.prompt("ses-1", "m1");
    await startedP;
    const p2 = h.prompt("ses-1", "m2");
    // ses-2 is not blocked by ses-1's in-flight prompt:
    await h.prompt("ses-2", "m1");
    assert.deepEqual(await h.contextOf("ses-2"), ["ses2-block"]);
    releaseFirst();
    await Promise.all([p1, p2]);
    assert.deepEqual(await h.contextOf("ses-1"), ["fresh-m2"], "older completion must not overwrite newer");
    await cleanup();
  }

  // 5) Per-session turns isolation: concurrent sessions must not pollute each other's turn index.
  {
    const h = createAdapterHarness();
    let ses1Calls = 0, ses2Calls = 0;
    const legacy = {
      "chat.message": async (inp, out) => {
        if (inp.sessionID === "ses-1") ses1Calls++;
        else if (inp.sessionID === "ses-2") ses2Calls++;
        out.message.parts.push({ type: "text", text: `block-${inp.sessionID}`, synthetic: true });
      },
      event: async () => {},
    };
    Object.defineProperty(legacy, "shouldRefresh", {
      enumerable: false,
      value: ({ turnIndex, hasCache }) => !hasCache || turnIndex === 0,
    });
    const cleanup = await registerV2Adapter(h.ctx, legacy);

    // ses-1 turns 0, 1:
    await h.prompt("ses-1", "m1");
    assert.equal(ses1Calls, 1, "ses-1 turn 0 injects");
    await h.prompt("ses-1", "m2");
    assert.equal(ses1Calls, 1, "ses-1 turn 1 retains cache (first mode)");

    // ses-2 turn 0 must inject independently (not use ses-1's turn count):
    await h.prompt("ses-2", "m1");
    assert.equal(ses2Calls, 1, "ses-2 turn 0 must inject despite ses-1 at turn 2");
    assert.deepEqual(await h.contextOf("ses-2"), ["block-ses-2"], "ses-2 has its own block");

    await cleanup();
  }
});

async function telemetryCase(t, mode, query, { block = "<memory_context>BLOCK_SENTINEL_🚀</memory_context>", extraEnv = {} } = {}) {
  const tmp = tempRoot("om-telemetry-");
  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "manifest.json");
  stageStubEngine(pkgDir, { requestLogPath: path.join(tmp, "requests.ndjson"), manifestPath, injectMode: mode });
  // Local fixture override: exact block and engine timing, no harness-wide knob.
  const bin = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`, "opencode-agents-memory");
  if (mode === "success") {
    fs.writeFileSync(bin, fs.readFileSync(bin, "utf8").replace(
      'block: null, memory_count: 0, tokens: 0, truncated: false, handling_ms: 0',
      `block: ${JSON.stringify(block)}, memory_count: 1, tokens: 0, truncated: false, handling_ms: 37`));
  }
  t.after(() => assertReaped(manifestPath));
  const driver = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driver, `import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(${JSON.stringify(path.join(pkgDir, "dist/plugin.js"))}));
const { hooks } = await server({ client: {}, directory: "/work/project" });
await hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [{ type: "text", text: ${JSON.stringify(query)} }] } });
process.exit(0);`);
  const run = await runDriver(driver, [], envForDriver(tmp, extraEnv));
  assert.equal(run.code, 0, run.stderr);
  return { raw: run.stderr, events: run.stderr.split("\n").flatMap(line => { try { return [JSON.parse(line)]; } catch { return []; } }) };
}

// The inherited shell may hold the debug flag; absence is ENFORCED here, never
// assumed, so the flag-off case cannot be flipped red by the caller's env.
function envForDriver(tmp, extraEnv) {
  const env = { ...process.env };
  delete env.OPENCODE_MEM_DEBUG_INJECT;
  return { ...env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage"), ...extraEnv };
}

test("test_wrapper_telemetry_requested_and_received_events", async (t) => {
  const { events } = await telemetryCase(t, "success", "héllo 🚀");
  const requested = events.find(e => e.event === "inject.requested");
  const received = events.find(e => e.event === "inject.received");
  assert.equal(requested?.query_chars, 7);
  assert.equal(requested.session_id, "ses-1");
  assert.equal(received?.block_bytes, Buffer.byteLength("<memory_context>BLOCK_SENTINEL_🚀</memory_context>"));
  assert.equal(received.handling_ms, 37);
  assert.equal(received.status, "ok");
  assert.equal(received.session_id, "ses-1");
});

test("test_wrapper_telemetry_captures_zero_bytes_and_excludes_content", async (t) => {
  const empty = await telemetryCase(t, "empty", "QUERY_SENTINEL_SECRET");
  assert.deepEqual(empty.events.find(e => e.event === "inject.received"),
    { level: "info", component: "opencode-mem-plugin", event: "inject.received", session_id: "ses-1", block_bytes: 0, handling_ms: 0, status: "empty", host_contract: "server" });
  const err = await telemetryCase(t, "error", "QUERY_SENTINEL_SECRET");
  assert.equal(err.events.find(e => e.event === "inject.received")?.status, "error");
  assert.equal(err.events.find(e => e.event === "inject.received")?.block_bytes, 0);
  const nonempty = await telemetryCase(t, "success", "QUERY_SENTINEL_SECRET");
  assert.equal(nonempty.raw.includes("QUERY_SENTINEL_SECRET"), false);
  assert.equal(nonempty.raw.includes("BLOCK_SENTINEL_"), false);
});

// Issue 153: the debug-gated delivery echo. Both cases run the real hook, stub
// engine and log writer; nothing imports the helper, so neither criterion is
// reachable without the mechanism running.
const BLOCK = "<memory_context>\n## Project facts\n- f\n## Recent actions\n- ran bash (background): dev\n"
  + "🚀".repeat(600) + "\n## Captured session summaries\n</memory_context>";
const SMALL = "<memory_context>ok</memory_context>";

test("test_debug_inject_echo_when_flag_on", async (t) => {
  const { events } = await telemetryCase(t, "success", "q", { block: BLOCK, extraEnv: { OPENCODE_MEM_DEBUG_INJECT: "1" } });
  const received = events.find(e => e.event === "inject.received");
  assert.deepEqual(received.sections, ["Project facts", "Recent actions", "Captured session summaries"]);
  assert.equal(received.prefix_truncated, true);
  assert.ok(received.block_prefix.startsWith("<memory_context>\n## Project facts\n"));
  assert.ok(received.block_prefix.endsWith("…"));
  assert.equal(received.block_prefix.includes("\uFFFD"), false, "cut on codepoints, never bytes");
  assert.equal(Array.from(received.block_prefix).length, 513, "512 codepoints plus the ellipsis");
  assert.equal(received.block_prefix.includes("Captured session summaries"), false, "headings scanned over the whole block");
  const small = await telemetryCase(t, "success", "q", { block: SMALL, extraEnv: { OPENCODE_MEM_DEBUG_INJECT: "1" } });
  const smallReceived = small.events.find(e => e.event === "inject.received");
  assert.equal(smallReceived.prefix_truncated, false);
  assert.equal(smallReceived.block_prefix, SMALL);
});

test("test_debug_inject_absent_when_flag_off", async (t) => {
  const { events } = await telemetryCase(t, "success", "q", { block: BLOCK });
  const received = events.find(e => e.event === "inject.received");
  assert.equal("sections" in received, false);
  assert.equal("block_prefix" in received, false);
  assert.deepEqual(received, {
    level: "info", component: "opencode-mem-plugin", event: "inject.received",
    session_id: "ses-1", block_bytes: Buffer.byteLength(BLOCK), handling_ms: 37,
    status: "ok", host_contract: "server",
  });
});

// AC-11: INTEGRATION CONTROL, not a RED claim. Real wrapper (staged dist) + real
// adapter + stub engine advertising inject_on "first": it records wiring, not a mutant kill.
test("test_hello_first_policy_survives_midturn_compaction", { timeout: 30000 }, async (t) => {
  const tmp = tempRoot("om-ac11-");
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "requests.ndjson");
  const manifestPath = path.join(tmp, "manifest.json");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode: "success", helloConfig: { inject_on: "first" } });
  const bin = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`, "opencode-agents-memory");
  fs.writeFileSync(bin, fs.readFileSync(bin, "utf8").replace(
    "block: null, memory_count: 0, tokens: 0, truncated: false, handling_ms: 0",
    "block: 'BLOCK-' + params.session_id, memory_count: 1, tokens: 0, truncated: false, handling_ms: 1"));
  t.after(() => assertReaped(manifestPath));
  const driver = path.join(tmp, "driver.mjs");
  const resultPath = path.join(tmp, "result.json");
  fs.writeFileSync(driver, `import fs from "node:fs";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(${JSON.stringify(path.join(pkgDir, "dist/plugin.js"))}));
const { registerV2Adapter } = await import(pathToFileURL(${JSON.stringify(path.join(pkgDir, "dist/v2/adapter.js"))}));
const { hooks: legacy } = await server({ client: {}, directory: "/work/project" });
const v2 = new Map(); let push; const queue = []; let wake;
const ctx = {
  location: { directory: "/work/project" },
  session: { hook: async (n, cb) => v2.set(n, cb), get: async () => ({ location: { directory: "/work/project" } }) },
  event: { async *subscribe({ signal }) {
    signal.addEventListener("abort", () => wake?.(), { once: true });
    while (!signal.aborted) { if (queue.length) yield queue.shift(); else await new Promise(r => { wake = r; }); }
  } },
};
const cleanup = await registerV2Adapter(ctx, legacy);
const context = async (sessionID) => { const e = { sessionID, system: [] }; await v2.get("context")(e); return e.system.map(p => p.text); };
const prompt = (sessionID, messageID) => v2.get("prompt")({ sessionID, messageID, prompt: { text: "q" } });
const out = {};
await prompt("ses-1", "m1"); out.before = await context("ses-1");
await prompt("ses-1", "m2"); out.second = await context("ses-1");
queue.push({ type: "session.compaction.ended", location: { directory: "/work/project" }, properties: { sessionID: "ses-1" } }); wake?.();
await new Promise(r => setTimeout(r, 300));
out.after = await context("ses-1");
await prompt("ses-2", "m1"); out.other = await context("ses-2"); out.firstStill = await context("ses-1");
await cleanup();
fs.writeFileSync(${JSON.stringify(resultPath)}, JSON.stringify(out));
process.exit(0);`);
  const run = await runDriver(driver, [], { ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage") });
  assert.equal(run.code, 0, run.stderr);
  const out = JSON.parse(fs.readFileSync(resultPath, "utf8"));
  assert.deepEqual(out, { before: ["BLOCK-ses-1"], second: ["BLOCK-ses-1"], after: ["BLOCK-ses-1"], other: ["BLOCK-ses-2"], firstStill: ["BLOCK-ses-1"] });
  const injects = fs.readFileSync(requestLogPath, "utf8").split("\n").filter(Boolean)
    .map(l => JSON.parse(l).got).filter(m => m && m.method === "inject" && m.params.chat_injection === true);
  assert.deepEqual(injects.map(m => m.params.session_id), ["ses-1", "ses-2"], "one inject per session; first policy and compaction cause no refresh");
});

test("test_state_reset_on_child_replacement_enforces_new_policy", { timeout: 30000 }, async (t) => {
  const tmp = tempRoot("om-reset-");
  const pkgDir = stagePackageTree(tmp);
  const helloConfigPath = path.join(tmp, "hello.json");
  const manifestPath = path.join(tmp, "manifest.json");
  const resultPath = path.join(tmp, "result.json");
  
  const driver = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driver, `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.STORAGE_ROOT;
const { server } = await import(pathToFileURL(path.join(process.argv[2], "dist/plugin.js")));
const { hooks } = await server({ client: {}, directory: "/work/project" });
const decisions = [];
const turn = () => hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [] } });
await turn();
for (const turnIndex of [0, 1]) {
  decisions.push(await hooks.shouldRefresh({ turnIndex, hasCache: true }));
}
fs.writeFileSync(process.env.HELLO_CONFIG, "{}");
const { pid } = JSON.parse(fs.readFileSync(process.env.MANIFEST, "utf8"));
process.kill(pid, "SIGTERM");
await new Promise(resolve => setTimeout(resolve, 100));
await turn();
decisions.push(await hooks.shouldRefresh({ turnIndex: 2, hasCache: true }));
fs.writeFileSync(process.env.RESULT, JSON.stringify(decisions));
process.exit(0);
`);
  
  fs.writeFileSync(helloConfigPath, JSON.stringify({ inject_on: "first" }));
  stageStubEngine(pkgDir, { helloConfigPath, manifestPath });
  
  t.after(() => {
    try { assertReaped(manifestPath); } catch {}
    fs.rmSync(tmp, { recursive: true, force: true });
  });
  
  const run = await runDriver(driver, [pkgDir], { 
    ...process.env, 
    STORAGE_ROOT: path.join(tmp, "storage"),
    HELLO_CONFIG: helloConfigPath, 
    MANIFEST: manifestPath, 
    RESULT: resultPath,
  });
  assert.equal(run.code, 0, `First child: ${run.stderr}`);
  
  fs.writeFileSync(helloConfigPath, JSON.stringify({ inject_on: "always" }));
  const run2 = await runDriver(driver, [pkgDir], { 
    ...process.env, 
    STORAGE_ROOT: path.join(tmp, "storage"),
    HELLO_CONFIG: helloConfigPath, 
    MANIFEST: manifestPath, 
    RESULT: resultPath,
  });
  assert.equal(run2.code, 0, `Second child: ${run2.stderr}`);
});

// Issue 150: first-mode refresh policy (not turn-based). Probes call shouldRefresh directly.
const PROBE_DRIVER = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(path.join(process.argv[2], "dist/plugin.js")).href);
const { hooks } = await server({ client: {}, directory: "/work/project" });
await hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [] } });
const decisions = [];
for (const turnIndex of JSON.parse(process.argv[3])) decisions.push([turnIndex, await hooks.shouldRefresh({ turnIndex, hasCache: true })]);
fs.writeFileSync(process.env.RESULT, JSON.stringify(decisions));
process.exit(0);
`;

async function probeDecisions(t, inject_on, indices) {
  const tmp = tempRoot("om-refresh-probe-");
  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "manifest.json");
  const resultPath = path.join(tmp, "result.json");
  stageStubEngine(pkgDir, {
    requestLogPath: path.join(tmp, "requests.ndjson"), manifestPath,
    injectMode: "success", helloConfig: { inject_on },
  });
  t.after(() => assertReaped(manifestPath));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, PROBE_DRIVER);
  const run = await runDriver(driverPath, [pkgDir, JSON.stringify(indices)], {
    ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage"), RESULT: resultPath,
  });
  assert.equal(run.code, 0, run.stderr);
  return new Map(JSON.parse(fs.readFileSync(resultPath, "utf8")));
}

test("test_refresh_rule_first_mode_never_fires_on_turn_count", { timeout: 30000 }, async (t) => {
  // The refresh policy is not turn-based, at any turn index. At runtime
  // `turnIndex === 0` with a live cache is unreachable (the single `contexts`
  // writer runs after the turn counter increments), so this guards the
  // contract, not a live path: re-adding `turnIndex === 0` or `% 20` turns it red.
  const d = await probeDecisions(t, "first", [0, 20, 40]);
  for (const i of [0, 20, 40]) assert.equal(d.get(i), false, `first mode must not refresh at turnIndex ${i}`);
});

test("test_refresh_rule_always_mode_unchanged", { timeout: 30000 }, async (t) => {
  const all = Array.from({ length: 61 }, (_, i) => i);
  const always = await probeDecisions(t, "always", all);
  for (const i of all) assert.equal(always.get(i), true, `always mode refreshes at turnIndex ${i}`);
  // Positive control: the same probe in first mode is live at turnIndex 1.
  const first = await probeDecisions(t, "first", [1]);
  assert.equal(first.get(1), false, "first mode retains block at turnIndex 1");
});

// Wire tests: a V2 adapter over the real wrapper and stub engine. Each step is
// a prompt (records the filtered inject delta and the adapter context), a
// compaction event, or a failure-flag change.
const WIRE_DRIVER = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const pkgDir = process.argv[2];
const steps = JSON.parse(process.argv[3]);
const { server } = await import(pathToFileURL(path.join(pkgDir, "dist/plugin.js")).href);
const { registerV2Adapter } = await import(pathToFileURL(path.join(pkgDir, "dist/v2/adapter.js")).href);
const { hooks: legacy } = await server({ client: {}, directory: "/work/project" });
const v2 = new Map(); const queue = []; let wake;
const ctx = {
  location: { directory: "/work/project" },
  session: { hook: async (n, cb) => v2.set(n, cb), get: async () => ({ location: { directory: "/work/project" } }) },
  event: { async *subscribe({ signal }) {
    signal.addEventListener("abort", () => wake?.(), { once: true });
    while (!signal.aborted) { if (queue.length) yield queue.shift(); else await new Promise(r => { wake = r; }); }
  } },
};
const cleanup = await registerV2Adapter(ctx, legacy);
const context = async (sessionID) => { const e = { sessionID, system: [] }; await v2.get("context")(e); return e.system.map(p => p.text); };
const prompt = (sessionID, messageID) => v2.get("prompt")({ sessionID, messageID, prompt: { text: "q" } });
const requestLog = process.env.REQUEST_LOG;
const injectCount = () => {
  if (!fs.existsSync(requestLog)) return 0;
  return fs.readFileSync(requestLog, "utf8").split("\\n").filter(Boolean)
    .map(l => JSON.parse(l).got)
    .filter(m => m && m.method === "inject" && m.params.chat_injection === true).length;
};
const out = [];
for (const step of steps) {
  if (step.flag === "create") fs.writeFileSync(process.env.FLAG_PATH, "");
  else if (step.flag === "remove") fs.rmSync(process.env.FLAG_PATH, { force: true });
  else if (step.compact) {
    // The host's V2 compaction hook: marks the session stale (adapter.js).
    await v2.get("compaction")({ sessionID: "ses-1", system: [] });
  } else {
    const before = injectCount();
    await prompt("ses-1", "m" + step.prompt);
    out.push({ prompt: step.prompt, injects: injectCount() - before, context: await context("ses-1") });
  }
}
fs.writeFileSync(process.env.RESULT, JSON.stringify(out));
await cleanup();
process.exit(0);
`;

const WIRE_BLOCK = "<memory_context>STUB_FIRST_BLOCK</memory_context>";

async function wireRun(t, steps, { failFlag = false } = {}) {
  const tmp = tempRoot("om-refresh-wire-");
  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "manifest.json");
  const requestLogPath = path.join(tmp, "requests.ndjson");
  const resultPath = path.join(tmp, "result.json");
  const flagPath = path.join(tmp, "fail.flag");
  stageStubEngine(pkgDir, {
    requestLogPath, manifestPath, injectMode: "block", helloConfig: { inject_on: "first" },
    ...(failFlag ? { failFlagPath: flagPath } : {}),
  });
  t.after(() => assertReaped(manifestPath));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, WIRE_DRIVER);
  const run = await runDriver(driverPath, [pkgDir, JSON.stringify(steps)], {
    ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    REQUEST_LOG: requestLogPath, FLAG_PATH: flagPath, RESULT: resultPath,
  }, 45000);
  assert.equal(run.code, 0, run.stderr);
  const out = JSON.parse(fs.readFileSync(resultPath, "utf8"));
  out.stderr = run.stderr;
  return out;
}

const promptSteps = (from, to) => Array.from({ length: to - from + 1 }, (_, i) => ({ prompt: from + i }));

test("test_inject_requested_logs_turn_index_through_the_adapter", { timeout: 45000 }, async (t) => {
  // Issue 150 observability: the adapter's zero-based turn index rides the
  // inject.requested line. In first mode only turn 0 reaches chat.message.
  const out = await wireRun(t, promptSteps(0, 20));
  const turns = out.stderr.split("\n").flatMap((line) => {
    try { return [JSON.parse(line)]; } catch { return []; }
  }).filter((e) => e.event === "inject.requested").map((e) => e.turn_index);
  assert.deepEqual(turns, [0]);
});

test("test_refresh_rule_wire_injects_only_at_turn_0", { timeout: 45000 }, async (t) => {
  const out = await wireRun(t, promptSteps(0, 20));
  assert.equal(out[0].context.length > 0, true, "precondition: cache empty");
  assert.deepEqual(out[1].context, [WIRE_BLOCK], "turn-0 block retained at prompt 1");
  const indices = out.flatMap((o, i) => (o.injects > 0 ? [i] : []));
  assert.deepEqual(indices, [0]);
});

test("test_refresh_rule_compaction_at_5_refreshes_next_prompt_only", { timeout: 45000 }, async (t) => {
  const out = await wireRun(t, [...promptSteps(0, 5), { compact: true }, ...promptSteps(6, 7)]);
  const vector = out.map(o => o.injects);
  assert.deepEqual(vector, [1, 0, 0, 0, 0, 0, 1, 0], "compaction mark refreshes the next prompt once");
});

test("test_v2_compaction_then_failed_refresh_keeps_the_block", { timeout: 45000 }, async (t) => {
  // Issue 155: a refresh the engine cannot answer must not cost the turn the
  // block it was already using. Prompt 1's inject is silenced by the stub's
  // failure flag, so the wrapper's `chat.message` catches and the adapter keeps
  // the block the compaction otherwise made stale; prompt 2 re-asks (the retry
  // mark), prompt 3 replays the cache in `first` mode.
  const out = await wireRun(t, [
    { prompt: 0 },
    { compact: true },
    { flag: "create" },
    { prompt: 1 },
    { flag: "remove" },
    { prompt: 2 },
    { prompt: 3 },
  ], { failFlag: true });
  const at = (p) => out.find((o) => o.prompt === p);
  assert.deepEqual(at(0).context, [WIRE_BLOCK], "precondition: turn 0 injected");
  assert.deepEqual(at(1).context, [WIRE_BLOCK], "a failed refresh keeps the previous block");
  assert.equal(at(1).injects, 1, "prompt 1 really asked the engine");
  assert.deepEqual(at(2).context, [WIRE_BLOCK], "the next prompt re-asks and replaces it");
  assert.equal(at(2).injects, 1, "the retry mark forces the refresh");
  assert.equal(at(3).injects, 0, "and the kept block stops later refreshes in `first` mode");
});

test("test_refresh_rule_lost_block_reinjects_next_prompt", { timeout: 45000 }, async (t) => {
  // Silent inject at turn 0 waits REQUEST_TIMEOUT_MS (15 s); the explicit 45 s driver bound covers it.
  const out = await wireRun(t, [
    { flag: "create" }, ...promptSteps(0, 0), { flag: "remove" }, ...promptSteps(1, 1),
  ], { failFlag: true });
  const at = (p) => out.find(o => o.prompt === p);
  assert.deepEqual(at(0).context, [], "failed turn-0 inject leaves no block");
  assert.deepEqual(at(1).context, [WIRE_BLOCK], "next prompt re-injects after the loss");
  assert.equal(at(1).injects, 1, "inject request count rises at prompt 1");
});
