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
if (process.env.DRIVER_MODE_OVERRIDE !== undefined) process.env.OPENCODE_MEM_MODE = process.env.DRIVER_MODE_OVERRIDE;

const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
const plugin = mod.default;
const ctx = JSON.parse(fs.readFileSync(process.env.DRIVER_CTX_PATH, "utf8"));
const hooks = await plugin.server({
  client: ctx.client ?? null,
  project: ctx.project ?? null,
  directory: pkgDir,
  worktree: null,
  $: null,
});
await hooks.hooks["chat.message"](
  { sessionID: process.env.DRIVER_SESSION || "sess-1" },
  { message: { id: "msg-1", parts: [{ type: "text", text: "hello world" }] } }
);
const logPath = process.env.STUB_LOG;
const deadline = Date.now() + 15000;
let seenCapture = false;
for (;;) {
  let text = "";
  try { text = fs.readFileSync(logPath, "utf8"); } catch {}
  if (text.includes('"method":"capture"')) { seenCapture = true; break; }
  if (Date.now() > deadline) break;
  await new Promise((resolve) => setTimeout(resolve, 20));
}
fs.writeFileSync(process.env.DRIVER_DONE_PATH, seenCapture ? "capture-seen" : "deadline");
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

test("test_wrapper_forwards_chat_message_as_capture", { timeout: 60000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-glue-"));
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
  });
  assert.equal(driverResult.code, 0, `engine driver failed: stderr=${driverResult.stderr}`);
  const parsed = fs
    .readFileSync(requestLogPath, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
  assert.ok(parsed.length >= 2, "stub stdin must see at least hello + capture");
  assert.equal(parsed[0].method, "hello", "first message after spawn must be hello");
  const capture = parsed.find((message) => message.method === "capture");
  assert.ok(capture, "stub stdin must receive a capture request line");
  assert.equal(capture.id, "capture-2");
  assert.equal(capture.params.session_id, "sess-1", "session_id from the hook must be forwarded");
  assert.ok(
    typeof capture.params.capture_id === "string" && capture.params.capture_id.length <= 64,
    "capture params must carry a capture_id of at most 64 chars"
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
