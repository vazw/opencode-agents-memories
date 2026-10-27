// W5.2 shared test harness for the staged-package cases. NOT a test file:
// `node --test` only matches `*.test.mjs`, so this module has no cases of its
// own — its correctness is proved by its two users, `v2-plugin-adapter.test.mjs`
// (the end-to-end capture, `mode: "v2"`) and `wrapper-toast.test.mjs` (the four
// toast cases, `mode: "v1"`).
//
// Why a subprocess and not an import: `plugin/src/plugin.js` spawns the engine
// with piped stdio and no `detached`/`unref` (`:365-368`), and the stub exits
// only on SIGTERM or stdin EOF. A test that imported the staged `dist/plugin.js`
// in-process would hold that child on `node:test`'s own event loop: a
// regression would HANG the file instead of failing it, and a success could
// leave an orphan. Every case therefore runs through `DRIVER_SOURCE` in a child
// process and asserts the stub pid is gone via `assertReaped` in `t.after`.
//
// The stub is staged where `startBridge` looks — `engine/<platform>-<arch>/
// opencode-agents-memory` — and must be executable: an absent file degrades
// with "no engine for …" and a non-`X_OK` one with "engine not executable
// for …" (`plugin/src/plugin.js:353-362`). A degraded bridge writes no wire log
// and no manifest, which would make the three toast failure cases pass for the
// wrong reason and the success pin fail for the wrong reason.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const REAL_PKG_ROOT = path.resolve(here, "..", "..");

/**
 * Complete temp copy of the package: `package.json` (the bundle's JSON import
 * attribute needs it), ALL of `dist/**` (H5: a `setup()` call resolves
 * `./v2/plugin.js` inside the staged package), all of `src/**` and `index.js`.
 */
export function stagePackageTree(tmpRoot) {
  const pkgDir = path.join(tmpRoot, "opencode-mem");
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.copyFileSync(path.join(REAL_PKG_ROOT, "package.json"), path.join(pkgDir, "package.json"));
  fs.cpSync(path.join(REAL_PKG_ROOT, "dist"), path.join(pkgDir, "dist"), { recursive: true });
  fs.cpSync(path.join(REAL_PKG_ROOT, "src"), path.join(pkgDir, "src"), { recursive: true });
  fs.copyFileSync(path.join(REAL_PKG_ROOT, "index.js"), path.join(pkgDir, "index.js"));
  return pkgDir;
}

/**
 * Stages the stub engine at the path the wrapper resolves and makes it
 * executable. `injectMode` is the knob the four toast cases need:
 *   "error"   → `inject` is answered with a JSON-RPC error, so `sendRequest`
 *               rejects (`plugin/src/plugin.js:315`), the `chat.message` catch
 *               runs and `notifyHost` is reached;
 *   "success" → `inject` answers the canonical empty block, so no catch runs
 *               and no toast may be emitted.
 */
export function stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode = "error" }) {
  const dir = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`);
  fs.mkdirSync(dir, { recursive: true });
  const binPath = path.join(dir, "opencode-agents-memory");
  fs.writeFileSync(binPath, stubEngineSource({ injectMode, requestLogPath, manifestPath }), {
    mode: 0o755,
  });
  fs.chmodSync(binPath, 0o755);
  return binPath;
}

// Static body (no template interpolation inside the script beyond CONFIG):
// answers the canned protocol-1.0.0 results, records its own pid, exits on
// SIGTERM or stdin EOF.
function stubEngineSource({ injectMode, requestLogPath, manifestPath }) {
  const config = JSON.stringify({ injectMode, requestLogPath, manifestPath });
  return `#!/usr/bin/env node
const CONFIG = ${config};
import fs from "node:fs";
const { injectMode, requestLogPath, manifestPath } = CONFIG;
function logLine(object) {
  try { fs.appendFileSync(requestLogPath, JSON.stringify(object) + "\\n"); } catch {}
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
  const id = parsed && parsed.id !== undefined ? parsed.id : null;
  const method = parsed && parsed.method;
  if (method === "inject" && injectMode === "error") {
    return { id, jsonrpc: "2.0", error: { code: -32001, message: "degraded" } };
  }
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
  const reply = { id, jsonrpc: "2.0", result };
  if (method === "shutdown") {
    process.stdout.write(JSON.stringify(reply) + "\\n", () => process.exit(0));
    return null;
  }
  return reply;
}
`;
}

/**
 * The driver script, written into the case's temp dir by its consumer. One
 * source, two modes (`DRIVER_MODE`), both reading their fixture from the JSON
 * file whose path the parent passes as argv[3]:
 *
 *   "v1" — imports the staged `dist/plugin.js`, builds `server()` from the
 *          fixture's client spec, fires its `chat.message` events, races each
 *          hook call against a 2 s guard started immediately before the call,
 *          and writes `{ settled, elapsedMs, toastCalls }`.
 *   "v2" — imports the staged `dist/v2/plugin.js`, materializes the fixture's
 *          serializable ctx into real functions, calls `setup(ctx)`, lets the
 *          supplied host events flow through the recorded subscription, polls
 *          the stub's wire log for the `capture` line and writes
 *          `{ helloCount, captureLine }`.
 */
export const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const pkgDir = process.argv[2];
const fixturePath = process.argv[3];
const mode = process.env.DRIVER_MODE ?? "v1";
const fixture = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;

function writeRecord(record) {
  fs.writeFileSync(process.env.DRIVER_DONE_PATH, JSON.stringify(record));
}

function readWire() {
  let text = "";
  try { text = fs.readFileSync(process.env.STUB_LOG, "utf8"); } catch { return []; }
  const messages = [];
  for (const line of text.split("\\n")) {
    if (!line.trim()) continue;
    try { messages.push(JSON.parse(line).got); } catch {}
  }
  return messages;
}

if (mode === "v1") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
  const spec = fixture.client ?? {};
  const client = {};
  if (Array.isArray(spec.sessionMessages)) {
    client.session = { messages: async () => ({ data: spec.sessionMessages }) };
  }
  let toastCalls = 0;
  if (spec.showToast === "counting") {
    client.tui = {
      showToast: async () => {
        toastCalls += 1;
        return { data: true };
      },
    };
  } else if (spec.showToast === "never") {
    client.tui = {
      showToast: () => {
        toastCalls += 1;
        return new Promise(() => {});
      },
    };
  }
  const { hooks } = await mod.default.server({ client, directory: fixture.directory ?? pkgDir });
  let settled = false;
  let elapsedMs = 0;
  for (const event of fixture.events ?? []) {
    const startedAt = Date.now();
    const guard = new Promise((resolve) => setTimeout(() => resolve("guard"), 2000));
    const outcome = await Promise.race([
      hooks["chat.message"](event.input ?? {}, event.output ?? {}).then(() => "hook"),
      guard,
    ]);
    settled = outcome === "hook";
    elapsedMs = Date.now() - startedAt;
  }
  writeRecord({ settled, elapsedMs, toastCalls });
  process.exit(0);
}

if (mode === "v2") {
  const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "v2", "plugin.js")));
  let registeredRpc = null;
  const ctx = {
    location: fixture.location,
    session: {
      hook: async () => {},
      context: async () => fixture.session?.context ?? [],
      get: async ({ sessionID }) => fixture.session?.get?.[sessionID] ?? null,
    },
    event: {
      async *subscribe({ signal }) {
        for (const event of fixture.events ?? []) yield event;
        await new Promise((resolve) => {
          if (signal.aborted) { resolve(); return; }
          signal.addEventListener("abort", () => resolve(), { once: true });
        });
      },
    },
  };
  if (fixture.rpc?.registered === true) {
    ctx.rpc = {
      register: async (definition) => {
        registeredRpc = definition?.id ?? null;
        return { events: { emit: async () => {} } };
      },
    };
  }
  await mod.setup(ctx);
  if (fixture.rpc?.registered === true && registeredRpc !== "opencode-mem") {
    throw new Error("the v2 entry did not register the toast rpc descriptor");
  }
  const deadline = Date.now() + 10000;
  let captureLine = null;
  for (;;) {
    captureLine = readWire().find((message) => message && message.method === "capture") ?? null;
    if (captureLine !== null) break;
    if (Date.now() > deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const helloCount = readWire().filter((message) => message && message.method === "hello").length;
  writeRecord({ helloCount, captureLine });
  process.exit(0);
}

throw new Error("unknown driver mode: " + mode);
`;

/**
 * Spawns the driver under a bounded SIGKILL and resolves with its exit and
 * captured output. `env` is used verbatim — pass `{ ...process.env, ...vars }`.
 */
export function runDriver(driverPath, args, env, timeoutMs = 30000) {
  return new Promise((resolve) => {
    const driver = spawn(process.execPath, [driverPath, ...args], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    driver.stdout.setEncoding("utf8");
    driver.stdout.on("data", (chunk) => (stdout += chunk));
    driver.stderr.setEncoding("utf8");
    driver.stderr.on("data", (chunk) => (stderr += chunk));
    const killer = setTimeout(() => driver.kill("SIGKILL"), timeoutMs);
    driver.once("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stdout, stderr, timedOut: signal === "SIGKILL" });
    });
  });
}

/**
 * Asserts the stub engine the driver's wrapper spawned is gone (pid `ESRCH`)
 * within 2 s — `plugin/tests/shutdown.test.mjs:234-239` precedent. Called from
 * `t.after(...)` by every case, so a leaked child fails the case instead of
 * outliving the run.
 */
export async function assertReaped(manifestPath, timeoutMs = 2000) {
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.ok(Number.isInteger(manifest.pid), "stub manifest must carry the child pid");
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      process.kill(manifest.pid, 0);
    } catch (error) {
      if (error.code === "ESRCH") break;
      throw error;
    }
    if (Date.now() > deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.throws(
    () => process.kill(manifest.pid, 0),
    /ESRCH/,
    "stub engine must be reaped (pid gone) within the 2 s bound"
  );
}
