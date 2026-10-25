// W5.1 `js` shard — wrapper glue: the chat.message hook must mint a capture_id
// and push an NDJSON capture request (with session_id) into the engine's stdin,
// and capture ids must be UUIDv7-shaped (version nibble 7), ≤ 64 chars and
// unique. Uses the stub-engine technique: a tiny node script answering the
// canned protocol-1.0.0 responses, staged at engine/<platform>-<arch>/ inside a
// temp copy of the package, with the stub's stdin logged to a file.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const realPkgRoot = path.resolve(here, "..");
const distEntry = path.join(realPkgRoot, "dist", "plugin.js");

/* ---------------------------------------------------------------- helpers */

// Complete temp copy: package.json + dist + src + index.js must exist so the
// package.json JSON import attribute resolves. Missing pieces are a test bug:
// fail loudly instead of running against a phantom package.
function stagePackage(tmpRoot) {
  const pkgDir = path.join(tmpRoot, "opencode-mem");
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.copyFileSync(path.join(realPkgRoot, "package.json"), path.join(pkgDir, "package.json"));
  fs.mkdirSync(path.join(pkgDir, "dist"), { recursive: true });
  fs.copyFileSync(path.join(realPkgRoot, "dist", "plugin.js"), path.join(pkgDir, "dist", "plugin.js"));
  fs.mkdirSync(path.join(pkgDir, "src"), { recursive: true });
  fs.copyFileSync(path.join(realPkgRoot, "src", "plugin.js"), path.join(pkgDir, "src", "plugin.js"));
  fs.copyFileSync(path.join(realPkgRoot, "index.js"), path.join(pkgDir, "index.js"));
  return pkgDir;
}

function stageEngine(pkgDir, { linger, requestLogPath, manifestPath }) {
  const dir = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`);
  fs.mkdirSync(dir, { recursive: true });
  const script = stubEngineSource({ linger, requestLogPath, manifestPath });
  const binPath = path.join(dir, "opencode-agents-memory");
  fs.writeFileSync(binPath, script, { mode: 0o755 });
  fs.chmodSync(binPath, 0o755);
  return binPath;
}

// Static body (no interpolation): CONFIG carries the baked paths.
function stubEngineSource({ linger, requestLogPath, manifestPath }) {
  const config = JSON.stringify({ linger, requestLogPath, manifestPath });
  return `#!/usr/bin/env node
// Stub engine for the W5.1 bridge tests. Answers every JSON-RPC request with
// the canned protocol 1.0.0 results from tests/fixtures/bridge/*-response.json.
const CONFIG = ${config};
import fs from "node:fs";
const { linger, requestLogPath, manifestPath } = CONFIG;
function logLine(obj) {
  try { fs.appendFileSync(requestLogPath, JSON.stringify(obj) + "\\n"); } catch {}
}
try {
  fs.writeFileSync(manifestPath, JSON.stringify({ pid: process.pid, argv: process.argv, env: process.env }, null, 2) + "\\n");
} catch {}
process.on("SIGTERM", () => process.exit(0));
let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let index = buffer.indexOf("\\n");
  while (index >= 0) {
    const line = buffer.slice(0, index);
    buffer = buffer.slice(index + 1);
    index = buffer.indexOf("\\n");
    if (!line.trim()) continue;
    let parsed;
    try { parsed = JSON.parse(line); } catch { continue; }
    logLine({ got: parsed });
    const reply = respond(parsed);
    if (reply) process.stdout.write(JSON.stringify(reply) + "\\n");
  }
});
process.stdin.on("end", () => process.exit(0));
function respond(parsed) {
  const method = parsed && parsed.method;
  let result = null;
  if (method === "hello") {
    result = {
      capabilities: { fire_and_forget_capture: true, standalone: true, web: true },
      engine_version: "0.1.0",
      methods: ["capture", "inject", "recall", "health", "shutdown"],
      protocol: "1.0.0",
    };
  } else if (method === "capture") {
    result = { accepted: true };
  } else if (method === "recall") {
    result = { items: [], count: 0 };
  } else if (method === "inject") {
    result = { block: null, memory_count: 0, tokens: 0, truncated: false, handling_ms: 0 };
  } else if (method === "health") {
    result = {
      ok: true,
      mode: "full",
      degraded_reason: null,
      protocol: "1.0.0",
      engine_version: "0.1.0",
      db: { path: "stub.db", ready: true },
      uptime_ms: 1,
      sessions: 0,
    };
  } else if (method === "shutdown") {
    result = { ok: true };
  } else {
    return null;
  }
  const reply = { id: parsed && parsed.id !== undefined ? parsed.id : null, jsonrpc: "2.0", result };
  if (method === "shutdown" && !linger) {
    // Reply, flush, then exit — the real engine exits after shutdown (§5).
    process.stdout.write(JSON.stringify(reply) + "\\n", () => process.exit(0));
    return null;
  }
  return reply;
}
`;
}

// Driver: imports the wrapper from the temp copy, runs server(), fires
// chat.message, waits for the capture line to appear in the stub's stdin log.
// In "linger" mode it additionally markers readiness and holds the process
// alive until the test delivers a signal (used by tests/shutdown.test.mjs).
const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const pkgDir = process.argv[2];
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
if (process.env.DRIVER_WEB_HOST !== undefined) process.env.OPENCODE_MEM_WEB_HOST = process.env.DRIVER_WEB_HOST;
if (process.env.DRIVER_WEB_PORT !== undefined) process.env.OPENCODE_MEM_WEB_PORT = process.env.DRIVER_WEB_PORT;
if (process.env.DRIVER_WEB_DIST !== undefined) process.env.OPENCODE_MEM_WEB_DIST = process.env.DRIVER_WEB_DIST;
if (process.env.DRIVER_ALLOW_UNGUARDED !== undefined) process.env.OPENCODE_MEM_ALLOW_UNGUARDED_OPEN = process.env.DRIVER_ALLOW_UNGUARDED;
if (process.env.DRIVER_MODE_OVERRIDE !== undefined) process.env.OPENCODE_MEM_MODE = process.env.DRIVER_MODE_OVERRIDE;

const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
const plugin = mod.default;
const ctx = JSON.parse(fs.readFileSync(process.env.DRIVER_CTX_PATH, "utf8"));
let client = ctx.client ?? null;
if (client && typeof client === "object" && client.sessionStub) {
  const stubData = client.sessionStub.data;
  client = {
    session: {
      messages: async (args) =>
        args && args.path && args.path.id ? { data: stubData } : { data: [] },
    },
  };
}
const hooks = await plugin.server({
  client,
  project: ctx.project ?? null,
  directory: pkgDir,
  worktree: null,
  $: null,
});
const events = process.env.DRIVER_EVENTS_PATH
  ? JSON.parse(fs.readFileSync(process.env.DRIVER_EVENTS_PATH, "utf8"))
  : [
      {
        hook: "chat.message",
        input: { sessionID: process.env.DRIVER_SESSION || "sess-1" },
        output: { message: { id: "msg-1", parts: [{ type: "text", text: "hello world" }] } },
      },
    ];
for (const event of events) {
  await hooks.hooks[event.hook](event.input ?? {}, event.output ?? {});
}
const logPath = process.env.STUB_LOG;
const waitMethod = process.env.DRIVER_WAIT_METHOD ?? "capture";
const deadline = Date.now() + 15000;
let seenCapture = false;
for (;;) {
  let text = "";
  try { text = fs.readFileSync(logPath, "utf8"); } catch {}
  if (waitMethod === "none" || text.includes('"method":"' + waitMethod + '"')) { seenCapture = true; break; }
  if (Date.now() > deadline) break;
  await new Promise((resolve) => setTimeout(resolve, 20));
}
fs.writeFileSync(process.env.DRIVER_DONE_PATH, seenCapture ? "seen" : "deadline");
if (process.env.DRIVER_MODE === "linger") {
  fs.writeFileSync(process.env.STUB_READY, "ready");
  await new Promise(() => {});
}
process.exit(0);
`;

function writeCtxFile(tmpRoot, ctx) {
  const ctxPath = path.join(tmpRoot, "driver-ctx.json");
  fs.writeFileSync(ctxPath, JSON.stringify(ctx));
  return ctxPath;
}

function runDriver(driverPath, args, env, timeoutMs = 30000) {
  return new Promise((resolve) => {
    const driver = spawn(process.execPath, [driverPath, ...args], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdoutText = "";
    let stderrText = "";
    driver.stdout.setEncoding("utf8");
    driver.stdout.on("data", (d) => (stdoutText += d));
    driver.stderr.setEncoding("utf8");
    driver.stderr.on("data", (d) => (stderrText += d));
    const killer = setTimeout(() => driver.kill("SIGKILL"), timeoutMs);
    driver.once("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stdout: stdoutText, stderr: stderrText });
    });
  });
}

/* ------------------------------------------------------------ tests */


/* Replacement control flow (W5.3 Step 8 / issue 34): the idle event owns
 * the capture; chat.message owns the injection ONLY; `project_id` is the
 * host `input.directory` frozen ONCE. Justified (reviewer note): the W5.1
 * oracle `test_wrapper_forwards_chat_message_as_capture` pinned the exact
 * per-message capture call W5.3's blueprint REMOVES; its coverage lives on
 * in these five oracles (AC7).
 */

// Shared helper: run the staged driver and parse the stub's stdin log.
async function runGlueTest(tmpRootSuffix, { ctx, events, wait }) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), tmpRootSuffix));
  const pkgDir = stagePackage(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const driverResult = await runDriver(driverPath, [pkgDir], {
    STUB_LOG: requestLogPath,
    DRIVER_CTX_PATH: writeCtxFile(tmp, ctx ?? { client: null, project: null }),
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: path.join(tmp, "driver-done.txt"),
    DRIVER_EVENTS_PATH: writeEventsFile(tmp, events ?? []),
    DRIVER_WAIT_METHOD: wait ?? "none",
  });
  assert.equal(driverResult.code, 0, `engine driver failed: stderr=${driverResult.stderr}`);
  return fs
    .readFileSync(requestLogPath, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
}

function writeEventsFile(tmpRoot, events) {
  const eventsPath = path.join(tmpRoot, "driver-events.json");
  fs.writeFileSync(eventsPath, JSON.stringify(events));
  return eventsPath;
}

function chatMessage(sessionID, parts) {
  return {
    hook: "chat.message",
    input: { sessionID: sessionID ?? "sess-1" },
    output: { message: { id: "msg-1", parts: parts ?? [{ type: "text", text: "hello world" }] } },
  };
}

function idleEvent(sessionID, properties) {
  return {
    hook: "event",
    input: {
      sessionID: sessionID ?? "sess-1",
      event: {
        type: "event",
        properties: { type: "session.idle", sessionID: sessionID ?? "sess-1", ...(properties ?? {}) },
      },
    },
  };
}

// W6.9 / issue 36: stage the package + stub engine, run the driver with ONE
// chat.message and DRIVER_WEB_DIST as the host's dist setting, then return the
// stub's wire log plus the manifest the stub wrote for its own process
// (pid, argv, env) — the only place the child's argv/env are observable.
async function runWebDistCase(distEnvValue) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-glue-webdist-"));
  const pkgDir = stagePackage(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const driverResult = await runDriver(driverPath, [pkgDir], {
    STUB_LOG: requestLogPath,
    DRIVER_CTX_PATH: writeCtxFile(tmp, { client: null, project: null }),
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: path.join(tmp, "driver-done.txt"),
    DRIVER_EVENTS_PATH: writeEventsFile(tmp, [chatMessage()]),
    DRIVER_WAIT_METHOD: "inject",
    DRIVER_WEB_DIST: distEnvValue,
  });
  assert.equal(driverResult.code, 0, `engine driver failed: stderr=${driverResult.stderr}`);
  const wire = fs
    .readFileSync(requestLogPath, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  return { wire, manifest };
}

// Issue 45 / O5: `childEnvironment()` is a WHITELIST, so without this forwarding
// the override env var is silently dropped on the only production launch path.
// Same shape as `runWebDistCase`: stage the package + stub engine, run the driver
// once, and read the manifest the stub wrote for its own process.
async function runAllowUnguardedCase(allowEnvValue) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-glue-unguarded-"));
  const pkgDir = stagePackage(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const env = {
    STUB_LOG: requestLogPath,
    DRIVER_CTX_PATH: writeCtxFile(tmp, { client: null, project: null }),
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: path.join(tmp, "driver-done.txt"),
    DRIVER_EVENTS_PATH: writeEventsFile(tmp, [chatMessage()]),
    DRIVER_WAIT_METHOD: "inject",
  };
  if (allowEnvValue !== undefined) env.DRIVER_ALLOW_UNGUARDED = allowEnvValue;
  const driverResult = await runDriver(driverPath, [pkgDir], env);
  assert.equal(
    driverResult.code,
    0,
    `allow-unguarded driver failed: stderr=${driverResult.stderr}`
  );
  return JSON.parse(fs.readFileSync(manifestPath, "utf8"));
}

test(
  "test_wrapper_forwards_allow_unguarded_open_env_to_child_engine",
  { timeout: 60000 },
  async () => {
    const enabled = await runAllowUnguardedCase("1");
    assert.equal(
      enabled.env.OPENCODE_MEM_ALLOW_UNGUARDED_OPEN,
      "1",
      "the wrapper's child-environment whitelist must forward OPENCODE_MEM_ALLOW_UNGUARDED_OPEN"
    );

    const unset = await runAllowUnguardedCase(undefined);
    assert.equal(
      unset.env.OPENCODE_MEM_ALLOW_UNGUARDED_OPEN,
      undefined,
      "an unset value must forward nothing"
    );
  }
);

test("test_wrapper_does_not_capture_on_every_chat_message", { timeout: 60000 }, async () => {
  const wire = await runGlueTest("om-glue-dc-", {
    events: [chatMessage(), chatMessage(), chatMessage()],
    wait: "none",
  });
  assert.ok(wire.length >= 2, "stub stdin must see at least hello + one runtime call");
  assert.ok(
    !wire.some((message) => message.method === "capture"),
    "a chat.message hook NEVER mints a per-message capture"
  );
  const injects = wire.filter((message) => message.method === "inject");
  assert.equal(injects.length, 3, "each eligible chat.message awaits inject");
  assert.equal(injects[0].params.session_id, "sess-1");
});

test("test_wrapper_project_id_comes_from_host_directory", { timeout: 60000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-glue-"));
  const pkgDir = stagePackage(tmp);
  // A hostile payload tries to override the project scope from transcript
  // AND event data; the wrapper must ONLY trust input.directory.
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath: path.join(tmp, "stub-manifest.json") });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const driverResult = await runDriver(driverPath, [pkgDir], {
    STUB_LOG: requestLogPath,
    DRIVER_CTX_PATH: writeCtxFile(tmp, { client: { sessionStub: { data: [
      { info: { id: "u1", role: "user" }, parts: [{ type: "text", text: "hello" }] },
      { info: { id: "a1", role: "assistant" }, parts: [{ type: "text", text: "hi" }] },
    ] } }, project: { id: "hostile-project-override" } }),
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: path.join(tmp, "driver-done.txt"),
    DRIVER_EVENTS_PATH: writeEventsFile(tmp, [
      {
        hook: "chat.message",
        input: { sessionID: "sess-1", projectId: "hostile" },
        output: {
          message: {
            id: "msg-1",
            parts: [{ type: "text", text: "hello" }],
            projectId: "hostile",
          },
        },
      },
      idleEvent("sess-1", { projectId: "hostile" }),
    ]),
    DRIVER_WAIT_METHOD: "capture",
  });
  assert.equal(driverResult.code, 0, `engine driver failed: stderr=${driverResult.stderr}`);
  const wire = fs
    .readFileSync(requestLogPath, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
  const capture = wire.find((message) => message.method === "capture");
  assert.ok(capture, "the idle event captured");
  assert.equal(capture.params.project_id, pkgDir, "project_id == input.directory, byte-frozen");
});

test("test_wrapper_captures_session_idle_with_project_id", { timeout: 60000 }, async () => {
  const wire = await runGlueTest("om-glue-idle-", {
    ctx: {
      client: {
        sessionStub: {
          data: [
            { info: { id: "u1", role: "user" }, parts: [{ type: "text", text: "fix the login bug" }] },
            { info: { id: "a1", role: "assistant" }, parts: [{ type: "text", text: "done — updated the handler" }] },
            { info: { id: "a2", role: "assistant" }, parts: [{ type: "tool", tool: "edit", state: { input: { path: "src/h.js" } } }] },
          ],
        },
      },
    },
    events: [idleEvent("sess-1")],
    wait: "capture",
  });
  const capture = wire.find((message) => message.method === "capture");
  assert.ok(capture, "the idle event captured");
  const context = capture.params.metadata.autoCaptureContext;
  assert.equal(context.userPrompt, "fix the login bug");
  assert.equal(
    context.textResponses[0],
    "done — updated the handler",
    "the eligible prompt's assistant RESPONSE text is assembled"
  );
  assert.equal(context.toolCalls[0].name, "edit");
  assert.ok(capture.params.metadata.promptId === "u1", "the eligible user-message id as promptId");
  assert.ok(
    typeof capture.params.capture_id === "string" && capture.params.capture_id.length <= 64,
    "idle capture carries a valid capture_id"
  );
});

test("test_wrapper_reuses_capture_id_for_replayed_idle", { timeout: 60000 }, async () => {
  const wire = await runGlueTest("om-glue-replay-", {
    ctx: { client: { sessionStub: { data: [
      { info: { id: "u1", role: "user" }, parts: [{ type: "text", text: "fix the login bug" }] },
      { info: { id: "a1", role: "assistant" }, parts: [{ type: "text", text: "done" }] },
    ] } } },
    events: [idleEvent("sess-1"), idleEvent("sess-1")],
    wait: "capture",
  });
  const captures = wire.filter((message) => message.method === "capture");
  assert.equal(captures.length, 2, "both idle deliveries queued their capture");
  assert.equal(
    captures[0].params.capture_id,
    captures[1].params.capture_id,
    "the replayed idle delivery REUSES the same capture_id (idempotent upsert)"
  );
});

test("test_wrapper_injects_on_chat_message_not_session_start", { timeout: 60000 }, async () => {
  const wire = await runGlueTest("om-glue-inject-", {
    ctx: { client: { sessionStub: { data: [
      { info: { id: "u1", role: "user" }, parts: [{ type: "text", text: "prompt" }] },
      { info: { id: "a1", role: "assistant" }, parts: [{ type: "text", text: "response" }] },
    ] } } },
    events: [idleEvent("sess-1"), chatMessage()],
    wait: "capture",
  });
  const injects = wire.filter((message) => message.method === "inject");
  assert.equal(injects.length, 1, "exactly one chat.message fired inject");
  const captures = wire.filter((message) => message.method === "capture");
  assert.equal(captures.length, 1, "the idle event captured once");
  assert.ok(wire[0]?.method === "hello", "hello precedes everything");
});

// W6.9 / issue 36 — the wrapper forwards the dist setting two ways: the env
// name (whitelisted, verbatim) and the two-token `--web-dist <dir>` argv pair.
test("test_wrapper_forwards_web_dist_env_and_flag_to_child_engine", { timeout: 60000 }, async () => {
  // A space-bearing path pins argv safety: it must survive as ONE argv element
  // (never `=`-joined, never a shell fragment). It need not exist.
  const dist = "/opt/opencode mem/web ui/dist";
  const { manifest } = await runWebDistCase(dist);
  assert.equal(
    manifest.env.OPENCODE_MEM_WEB_DIST,
    dist,
    "the env whitelist forwards OPENCODE_MEM_WEB_DIST byte-for-byte"
  );
  const index = manifest.argv.indexOf("--web-dist");
  assert.ok(index >= 0, "--web-dist is present in the child argv");
  assert.equal(
    manifest.argv[index + 1],
    dist,
    "--web-dist and its value are two ADJACENT argv elements, unmodified"
  );
  assert.equal(
    manifest.argv.filter((argument) => argument === "--web-dist").length,
    1,
    "exactly one --web-dist occurrence"
  );
  const rootIndex = manifest.argv.indexOf("--storage-root");
  assert.ok(
    rootIndex >= 0 && typeof manifest.argv[rootIndex + 1] === "string" && manifest.argv[rootIndex + 1].length > 0,
    "the frozen --storage-root pair is still present"
  );
});

// Paired guard (passes before and after by design): the wrapper invents no
// default of its own — an empty host setting forwards nothing, so the engine's
// exe-relative default stays the single source of truth.
test("test_wrapper_omits_web_dist_when_host_sets_nothing", { timeout: 60000 }, async () => {
  const { manifest } = await runWebDistCase("");
  assert.equal(
    manifest.env.OPENCODE_MEM_WEB_DIST,
    undefined,
    "an empty host setting is not forwarded into the child environment"
  );
  assert.equal(
    manifest.argv.indexOf("--web-dist"),
    -1,
    "no --web-dist is appended when the host sets nothing"
  );
});

test("test_capture_id_is_uuidv7_shaped_and_bounded", { timeout: 10000 }, async () => {
  const mod = await import(distEntry);
  assert.equal(typeof mod.newCaptureId, "function");
  const segments = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
  for (let iteration = 0; iteration < 100; iteration += 1) {
    const id = mod.newCaptureId();
    assert.match(id, segments, "5 hex groups of 8-4-4-4-12");
    const parts = id.split("-");
    assert.equal(parts.length, 5);
    assert.ok(parts[2].startsWith("7"), "UUIDv7 version nibble must be 7");
    assert.match(parts[3], /^[89ab]/, "RFC 4122 variant nibble");
    assert.ok(id.length <= 64, `contract bounds capture ids at 64 chars (got ${id.length})`);
  }
  const set = new Set();
  for (let iteration = 0; iteration < 100; iteration += 1) set.add(mod.newCaptureId());
  assert.equal(set.size, 100, "capture ids must be unique across 100 calls");
});
