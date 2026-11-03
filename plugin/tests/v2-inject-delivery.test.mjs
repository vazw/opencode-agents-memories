import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { assertReaped, runDriver, stagePackageTree, stageStubEngine } from "./harness/bridge.mjs";

const DRIVER = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(path.join(process.argv[2], "dist/plugin.js")));
const { hooks } = await server({ client: {}, directory: "/work/project" });
const decisions = [];
const turn = () => hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [] } });
await turn(); // starts the child and runs hello
for (const turnIndex of [0, 1]) {
  decisions.push(await hooks.shouldRefresh({ turnIndex, hasCache: true }));
}
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
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-inject-delivery-"));
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
      [true, inject_on !== "first", true], String(inject_on));
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
});

async function telemetryCase(t, mode, query) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-telemetry-"));
  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "manifest.json");
  stageStubEngine(pkgDir, { requestLogPath: path.join(tmp, "requests.ndjson"), manifestPath, injectMode: mode });
  // Local fixture override: exact block and engine timing, no harness-wide knob.
  const bin = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`, "opencode-agents-memory");
  if (mode === "success") {
    fs.writeFileSync(bin, fs.readFileSync(bin, "utf8").replace(
      'block: null, memory_count: 0, tokens: 0, truncated: false, handling_ms: 0',
      'block: "<memory_context>BLOCK_SENTINEL_🚀</memory_context>", memory_count: 1, tokens: 0, truncated: false, handling_ms: 37'));
  }
  t.after(() => assertReaped(manifestPath));
  const driver = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driver, `import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(${JSON.stringify(path.join(pkgDir, "dist/plugin.js"))}));
const { hooks } = await server({ client: {}, directory: "/work/project" });
await hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [{ type: "text", text: ${JSON.stringify(query)} }] } });
process.exit(0);`);
  const run = await runDriver(driver, [], { ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "storage") });
  assert.equal(run.code, 0, run.stderr);
  return { raw: run.stderr, events: run.stderr.split("\n").flatMap(line => { try { return [JSON.parse(line)]; } catch { return []; } }) };
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
    { level: "info", component: "opencode-mem-plugin", event: "inject.received", session_id: "ses-1", block_bytes: 0, handling_ms: 0, status: "empty" });
  const err = await telemetryCase(t, "error", "QUERY_SENTINEL_SECRET");
  assert.equal(err.events.find(e => e.event === "inject.received")?.status, "error");
  assert.equal(err.events.find(e => e.event === "inject.received")?.block_bytes, 0);
  const nonempty = await telemetryCase(t, "success", "QUERY_SENTINEL_SECRET");
  assert.equal(nonempty.raw.includes("QUERY_SENTINEL_SECRET"), false);
  assert.equal(nonempty.raw.includes("BLOCK_SENTINEL_"), false);
});
