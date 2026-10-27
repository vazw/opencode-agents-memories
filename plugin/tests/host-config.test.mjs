// W5.1 `js` shard — host configuration forwarding (docs/bridge-schema.md §9 + §10).
// The wrapper reads OPENCODE_MEM_STORAGE_ROOT / OPENCODE_MEM_MODE /
// OPENCODE_MEM_WEB_HOST / OPENCODE_MEM_WEB_PORT and forwards them as ARGv flags
// to the child — and (R4, security posture (a)-(j)) it NEVER forwards host
// credentials: not in argv, not in the child's environment, not on stderr.
// Both tests stage a stub engine so a real child is actually spawned.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const realPkgRoot = path.resolve(here, "..");

/* ---------------------------------------------------------------- helpers */

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
  } else if (method === "shutdown" && !linger) {
    const reply = { id: parsed.id, jsonrpc: "2.0", result: { ok: true } };
    process.stdout.write(JSON.stringify(reply) + "\\n", () => process.exit(0));
    return null;
  } else if (method === "shutdown") {
    return { id: parsed.id, jsonrpc: "2.0", result: { ok: true } };
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
  return { id: parsed.id, jsonrpc: "2.0", result };
}
`;
}

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
process.exit(0);
`;

function writeCtxFile(tmpRoot, ctx) {
  const ctxPath = path.join(tmpRoot, "driver-ctx.json");
  fs.writeFileSync(ctxPath, JSON.stringify(ctx));
  return ctxPath;
}

function readManifest(manifestPath) {
  return JSON.parse(fs.readFileSync(manifestPath, "utf8"));
}

/* ------------------------------------------------------------------ tests */

test("test_host_config_forwards_env", { timeout: 60000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-cfg-"));
  const pkgDir = stagePackage(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const root = path.join(tmp, "storage");
  const driverResult = await (new Promise((resolve) => {
    const driver = spawn(process.execPath, [driverPath, pkgDir], {
      env: {
        STUB_LOG: requestLogPath,
        DRIVER_CTX_PATH: writeCtxFile(tmp, { client: null, project: null }),
        DRIVER_STORAGE_ROOT: root,
        DRIVER_WEB_HOST: "127.0.0.99",
        DRIVER_WEB_PORT: "3999",
        DRIVER_MODE_OVERRIDE: "stdio",
        DRIVER_DONE_PATH: path.join(tmp, "driver-done.txt"),
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderrText = "";
    driver.stderr.setEncoding("utf8");
    driver.stderr.on("data", (d) => (stderrText += d));
    const killer = setTimeout(() => driver.kill("SIGKILL"), 30000);
    driver.once("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stderr: stderrText });
    });
  }));
  assert.equal(driverResult.code, 0, `engine driver failed: stderr=${driverResult.stderr}`);
  const manifest = readManifest(manifestPath);
  const argv = manifest.argv;
  for (const [flag, value] of [
    ["--storage-root", root],
    ["--mode", "stdio"],
    ["--web-host", "127.0.0.99"],
    ["--web-port", "3999"],
  ]) {
    const idx = argv.indexOf(flag);
    assert.ok(idx >= 0, `child argv must carry ${flag}`);
    assert.equal(argv[idx + 1], value, `${flag} value must equal the resolved ${value}`);
  }
  // Forwarded as ARGV, never smuggled through the child environment:
  for (const name of [
    "OPENCODE_MEM_STORAGE_ROOT",
    "OPENCODE_MEM_WEB_HOST",
    "OPENCODE_MEM_WEB_PORT",
    "OPENCODE_MEM_MODE",
    "DRIVER_STORAGE_ROOT",
    "DRIVER_WEB_HOST",
    "DRIVER_WEB_PORT",
    "DRIVER_MODE_OVERRIDE",
  ]) {
    assert.equal(manifest.env[name], undefined, `child env must not carry ${name}`);
  }
});

test("test_host_config_does_not_forward_host_credentials", { timeout: 60000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-sec-"));
  const pkgDir = stagePackage(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageEngine(pkgDir, { linger: true, requestLogPath, manifestPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const authorization = "Basic dXNlcjpwYXNz";
  const token = "sk-super-secret-canary";
  const password = "hunter2-canary";
  const canary = "canary-env-secret";
  const env = {
    STUB_LOG: requestLogPath,
    DRIVER_CTX_PATH: writeCtxFile(tmp, {
      client: { _client: { headers: { Authorization: authorization } } },
      project: { config: { token, webServerAuthPassword: password } },
    }),
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: path.join(tmp, "driver-done.txt"),
    DRIVER_SECRET_CANARY: canary,
  };
  const driverResult = await (new Promise((resolve) => {
    const driver = spawn(process.execPath, [driverPath, pkgDir], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdoutText = "";
    let stderrText = "";
    driver.stdout.setEncoding("utf8");
    driver.stdout.on("data", (d) => (stdoutText += d));
    driver.stderr.setEncoding("utf8");
    driver.stderr.on("data", (d) => (stderrText += d));
    const killer = setTimeout(() => driver.kill("SIGKILL"), 30000);
    driver.once("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stdout: stdoutText, stderr: stderrText });
    });
  }));
  assert.equal(driverResult.code, 0, `engine driver failed: stderr=${driverResult.stderr}`);
  const manifest = readManifest(manifestPath);
  const surfaces = [
    ["child argv", JSON.stringify(manifest.argv)],
    ["child env", JSON.stringify(manifest.env)],
    ["driver stdout", driverResult.stdout],
    ["driver stderr", driverResult.stderr],
    ["stub stdin log", fs.readFileSync(requestLogPath, "utf8")],
  ];
  for (const secret of [authorization, token, password, canary]) {
    for (const [surface, text] of surfaces) {
      assert.ok(!text.includes(secret), `${surface} must not contain ${secret}`);
    }
  }
});
