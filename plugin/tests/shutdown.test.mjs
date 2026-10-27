// W5.1 `js` shard — shutdown contract (docs/bridge-schema.md §5, D-45).
//
// Part 1 (source rules, against src/plugin.js AND dist/plugin.js):
//   - never calls process.exit( (the host owns process lifetime);
//   - drops its SIGINT/SIGTERM handlers first, then re-delivers the signal
//     with process.kill(process.pid, sig) so the host can exit by default
//     disposition;
//   - clears escalation timers from the shutdownTimers array;
//   - drains in-flight captures with `await awaitCaptureDrain()` guarded by
//     the pendingCaptures counter.
//
// Part 2 (behavioral): the wrapper is spawned against a stub engine in a temp
// package copy; the stub answers shutdown with {"ok":true} but does NOT exit
// (linger), so the wrapper must escalate after 2 s with SIGTERM, reap the
// child (exitCode !== null, signalCode === null — the stub exits 0 on SIGTERM),
// re-deliver the signal and die. The test then asserts the stub pid is gone.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const realPkgRoot = path.resolve(here, "..");

/* ------------------------------------------------------------ source rules */

test("test_shutdown_source_rules", () => {
  for (const rel of ["src/plugin.js", "dist/plugin.js"]) {
    const fullPath = path.join(realPkgRoot, rel);
    const text = fs.readFileSync(fullPath, "utf8");
    const mustContain = [
      'process.off("SIGINT", shutdownHandler)',
      'process.off("SIGTERM", shutdownHandler)',
      "process.kill(process.pid, sig",
      "for (const t of shutdownTimers) clearTimeout(t)",
      "await awaitCaptureDrain()",
    ];
    for (const needle of mustContain) {
      assert.ok(text.includes(needle), `${rel}: missing required source form: ${needle}`);
    }
    assert.match(text, /const shutdownTimers = \[\];/, `${rel}: shutdownTimers must be an array`);
    assert.ok(text.includes("pendingCaptures"), `${rel}: pendingCaptures counter must exist`);
    assert.ok(
      !text.includes("process.exit("),
      `${rel}: wrapper must never call process.exit`
    );
  }
});

/* --------------------------------------------------------- behavioral reap */

function stagePackage(tmpRoot) {
  const pkgDir = path.join(tmpRoot, "opencode-mem");
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.copyFileSync(path.join(realPkgRoot, "package.json"), path.join(pkgDir, "package.json"));
  fs.mkdirSync(path.join(pkgDir, "dist"), { recursive: true });
  fs.copyFileSync(path.join(realPkgRoot, "dist", "plugin.js"), path.join(pkgDir, "dist", "plugin.js"));
  fs.copyFileSync(path.join(realPkgRoot, "dist", "compaction.js"), path.join(pkgDir, "dist", "compaction.js"));
  fs.copyFileSync(path.join(realPkgRoot, "dist", "tools.js"), path.join(pkgDir, "dist", "tools.js"));
  fs.mkdirSync(path.join(pkgDir, "src"), { recursive: true });
  fs.copyFileSync(path.join(realPkgRoot, "src", "plugin.js"), path.join(pkgDir, "src", "plugin.js"));
  fs.copyFileSync(path.join(realPkgRoot, "src", "compaction.js"), path.join(pkgDir, "src", "compaction.js"));
  fs.copyFileSync(path.join(realPkgRoot, "src", "tools.js"), path.join(pkgDir, "src", "tools.js"));
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

function stubEngineSource({ linger, requestLogPath, manifestPath }) {
  const config = JSON.stringify({ linger, requestLogPath, manifestPath });
  return `#!/usr/bin/env node
const CONFIG = ${config};
import fs from "node:fs";
const { linger, requestLogPath, manifestPath } = CONFIG;
function logLine(obj) {
  try { fs.appendFileSync(requestLogPath, JSON.stringify(obj) + "\\n"); } catch {}
}
try {
  fs.writeFileSync(manifestPath, JSON.stringify({ pid: process.pid, argv: process.argv, env: process.env }, null, 2) + "\\n");
} catch {}
// Graceful SIGTERM: exit code 0, no signal — so the wrapper's reap observes
// exitCode !== null and signalCode === null on the escalation path.
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
  } else if (method === "shutdown") {
    result = { ok: true }; // linger=true: reply, but keep running (no exit)
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
  } else {
    return null;
  }
  if (method === "shutdown" && !linger) {
    process.stdout.write(JSON.stringify({ id: parsed.id, jsonrpc: "2.0", result }) + "\\n", () => process.exit(0));
    return null;
  }
  return { id: parsed.id, jsonrpc: "2.0", result };
}
`;
}

// Same driver shape as wrapper-glue/host-config, plus DRIVER_MODE=linger:
// mark readiness once the capture has been sent, then hold the process alive
// until the test delivers the host signal.
const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const pkgDir = process.argv[2];
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
const plugin = mod.default;
const ctx = JSON.parse(fs.readFileSync(process.env.DRIVER_CTX_PATH, "utf8"));
const hooks = await plugin.server({
  client: null,
  project: null,
  directory: pkgDir,
  worktree: null,
  $: null,
});
await hooks.hooks["chat.message"](
  { sessionID: "reap-test" },
  { message: { id: "msg-1", parts: [{ type: "text", text: "hello world" }] } }
);
const logPath = process.env.STUB_LOG;
const deadline = Date.now() + 15000;
for (;;) {
  let text = "";
  try { text = fs.readFileSync(logPath, "utf8"); } catch {}
  if (text.includes('"id":"hello-1"') && text.includes('"method":"capture"')) break;
  if (Date.now() > deadline) break;
  await new Promise((resolve) => setTimeout(resolve, 20));
}
fs.writeFileSync(process.env.STUB_READY, "ready");
await new Promise(() => {});
`;

test("test_shutdown_reaps_child", { timeout: 60000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-shutdown-"));
  const pkgDir = stagePackage(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const readyPath = path.join(tmp, "wrapper-ready");

  const driver = spawn(process.execPath, [driverPath, pkgDir], {
    env: {
      STUB_LOG: requestLogPath,
      STUB_READY: readyPath,
      DRIVER_CTX_PATH: path.join(tmp, "driver-ctx.json"),
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  fs.writeFileSync(path.join(tmp, "driver-ctx.json"), JSON.stringify({}));
  let stderrText = "";
  driver.stderr.setEncoding("utf8");
  driver.stderr.on("data", (data) => (stderrText += data));

  // Wait until the wrapper has spawned the engine and completed at least a
  // hello + capture exchange; then deliver the host signal.
  const readyDeadline = Date.now() + 20000;
  let ready = false;
  while (Date.now() < readyDeadline) {
    if (fs.existsSync(readyPath)) {
      ready = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.ok(ready, "wrapper must have spawned the engine before SIGTERM");

  const startedAt = Date.now();
  driver.kill("SIGTERM");
  const closed = await new Promise((resolve) => {
    driver.once("close", (code, signal) => resolve({ code, signal }));
  });
  const elapsed = Date.now() - startedAt;
  assert.ok(elapsed < 8000, `host must not be held indefinitely (took ${elapsed}ms)`);
  assert.equal(closed.signal, "SIGTERM", "re-delivered SIGTERM must terminate the driver");

  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.ok(Number.isInteger(manifest.pid), "stub manifest must carry the child pid");
  assert.throws(
    () => process.kill(manifest.pid, 0),
    /ESRCH/,
    "stub engine must be reaped (pid gone) within the 5 s bound"
  );

  const reapedLines = [];
  for (const line of stderrText.split("\n")) {
    if (!line.includes("child_reaped")) continue;
    try {
      reapedLines.push(JSON.parse(line));
    } catch {}
  }
  assert.ok(reapedLines.length >= 1, `wrapper must log the reap, stderr=${stderrText}`);
  assert.equal(reapedLines[0].exit_code, 0, "stub exited cleanly on SIGTERM escalation");
  assert.equal(reapedLines[0].signal, null, "reap must observe a signalCode of null");
});
