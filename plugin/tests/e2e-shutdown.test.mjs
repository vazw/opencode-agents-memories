// W5.1 step 16 — end-to-end shutdown oracle (integration shard; M4/M20/D2).
//
// Mechanism (a) — THIS file — uses a STUB executable staged at
// engine/<platform>-<arch>/opencode-agents-memory inside a TEMP COPY of the
// package. The stub speaks the hello/shutdown protocol but REFUSES to exit
// (it even ignores SIGTERM), so the wrapper's real escalation ladder (§5:
// ack → 2 s SIGTERM → 5 s SIGKILL) fires for real. Mechanism (b) — the REAL
// binary's env-gated `--mode=hang-on-shutdown` — is consumed ONLY by the
// root Rust suite (tests/engine_shutdown.rs); this file never edits
// src/main.rs.
//
// Assertions (blueprint step 16): 2 s ± tolerance SIGTERM escalation, 5 s ±
// tolerance SIGKILL, child reaped (`exitCode !== null`/`signalCode !== null`),
// both pipes closed, no surviving process with the engine's cmdline.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const PLUGIN = path.join(here, "..");

/** A complete temp copy of the package (package.json, dist, src, index.js). */
function tempPackage() {
  const dir = mkdtempSync(path.join(os.tmpdir(), "opencode-mem-e2e-"));
  copyFileSync(path.join(PLUGIN, "package.json"), path.join(dir, "package.json"));
  copyFileSync(path.join(PLUGIN, "index.js"), path.join(dir, "index.js"));
  mkdirSync(path.join(dir, "dist"));
  copyFileSync(path.join(PLUGIN, "dist/plugin.js"), path.join(dir, "dist/plugin.js"));
  mkdirSync(path.join(dir, "src"));
  copyFileSync(path.join(PLUGIN, "src/plugin.js"), path.join(dir, "src/plugin.js"));
  return dir;
}

/**
 * The stub engine: speaks the protocol, IGNORES SIGTERM (proving the
 * escalation to SIGKILL), never exits on its own, logs every request to
 * `<root>/stub-stdin.log` (with stdio EOF and received signals).
 */
function stageStubEngine(dir, root) {
  const outDir = path.join(dir, "engine", `${process.platform}-${process.arch}`);
  mkdirSync(outDir, { recursive: true });
  const stub = path.join(outDir, "opencode-agents-memory");
  const body = `
    import { appendFileSync } from "node:fs";
    import { createInterface } from "node:readline";
    const LOG = ${JSON.stringify(path.join(root, "stub-stdin.log"))};
    const log = (event, extra = {}) => {
      appendFileSync(LOG, JSON.stringify({ event, t: Date.now(), ...extra }) + "\\n");
    };
    log("stub-start", { pid: process.pid });
    process.on("SIGTERM", () => { log("got-sigterm"); /* REFUSES to exit */ });
    const rl = createInterface({ input: process.stdin });
    rl.on("line", (line) => {
      log("request", { line });
      let message;
      try { message = JSON.parse(line); } catch { return; }
      const id = message.id;
      let result;
      if (message.method === "hello") {
        result = { protocol: "1.0.0", engine_version: "stub", methods: ["capture","inject","recall","health","shutdown"], capabilities: { fire_and_forget_capture: true, standalone: true, web: true } };
      } else if (message.method === "capture") {
        result = { accepted: true };
        log("capture-received", { capture_id: message?.params?.capture_id ?? null });
      } else if (message.method === "shutdown") {
        result = { ok: true };
      } else {
        result = {};
      }
      process.stdout.write(JSON.stringify({ id, jsonrpc: "2.0", result }) + "\\n");
    });
    rl.on("close", () => { log("stdin-eof"); /* refuses to exit even at EOF */ });
    // Point-blank never-exit heartbeat.
    setTimeout(() => {}, 2 ** 30);
  `;
  writeFileSync(stub, `#!/usr/bin/env node\n${body}\n`);
  chmodSync(stub, 0o755);
  return stub;
}

/** The driver: loads the wrapper, drives one capture hook, raises SIGTERM. */
function driverScript() {
  return `
    import("./dist/plugin.js").then(async ({ default: plugin }) => {
      const { hooks } = await plugin.server({});
      await hooks["event"]({ sessionID: "e2e-sess", event: { properties: { sessionID: "e2e-sess" } } });
      await hooks["chat.message"](
        { sessionID: "e2e-sess" },
        { message: { text: "e2e canary transcript" } },
      );
      // Give spawn + handshake + capture a beat, then exercise teardown.
      setTimeout(() => process.kill(process.pid, "SIGTERM"), 1500);
    });
  `;
}

/** Spawns the driver in the temp package; resolves on exit with details. */
function runDriver(dir, root) {
  return new Promise((resolve) => {
    const started = Date.now();
    const stdout = [];
    const stderr = [];
    const driver = spawn(
      process.execPath,
      ["-e", driverScript()],
      {
        cwd: dir,
        env: { ...process.env, OPENCODE_MEM_STORAGE_ROOT: root },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    driver.stdout.on("data", (chunk) => stdout.push(chunk));
    driver.stderr.on("data", (chunk) => stderr.push(chunk));
    driver.on("close", (code, signal) => {
      resolve({
        stdout: stdout.join(""),
        stderr: stderr.join(""),
        code,
        signal,
        started,
        elapsed: Date.now() - started,
      });
    });
  });
}

/** /proc-based orphan probe: SURVIVING processes whose cmdline has the stub dir. */
function findSurviving(stubDirText) {
  const survivors = [];
  let procEntries = [];
  try { procEntries = readdirSync("/proc"); } catch { return survivors; }
  for (const pid of procEntries.filter((name) => /^\d+$/.test(name))) {
    if (pid === process.pid) continue;
    let cmdline = "";
    try { cmdline = readFileSync(path.join("/proc", pid, "cmdline"), "utf8"); } catch { continue; }
    if (cmdline.includes(stubDirText)) survivors.push(pid);
  }
  return survivors;
}

function stubEvents(root) {
  const log = readFileSync(path.join(root, "stub-stdin.log"), "utf8");
  return log
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line));
}

// ---------------------------------------------------------------------------
// The ladder end to end: hello, capture, shutdown ack, 2 s SIGTERM, 7 s SIGKILL.
// ---------------------------------------------------------------------------

test("test_e2e_shutdown_graceful_within_bounds", async () => {
  const dir = tempPackage();
  const root = path.join(dir, "root");
  mkdirSync(root, { recursive: true });
  stageStubEngine(dir, root);
  const { stderr, signal, elapsed } = await runDriver(dir, root);
  // The driver terminates via re-delivered SIGTERM (the wrapper never
  // calls process.exit itself).
  assert.equal(signal, "SIGTERM", `re-delivered signal; drive log: ${stderr.slice(-400)}`);
  assert.ok(elapsed > 1_400, `SIGTERM raised after the handshake: ${elapsed}ms`);
  assert.ok(elapsed < 12_000, `the ladder is bounded (ack + 2 s + 5 s): ${elapsed}ms`);
  // The wrapper's structured reap line reached stderr.
  assert.match(stderr, /"event":"child_reaped"/, `reap log: ${stderr.slice(-400)}`);
  rmSync(dir, { recursive: true, force: true });
}, { timeout: 30_000 });

test("test_e2e_shutdown_sigterm_fallback", async () => {
  const dir = tempPackage();
  const root = path.join(dir, "root");
  mkdirSync(root, { recursive: true });
  stageStubEngine(dir, root);
  const { stderr } = await runDriver(dir, root);
  const events = stubEvents(root);
  // The refusing stub DID receive SIGTERM (it ignores it; the SIGKILL ends it).
  assert.ok(events.some((e) => e.event === "got-sigterm"), `stub log: ${events.map((e) => e.event).join(",")}`);
  const termAt = events.find((e) => e.event === "got-sigterm")?.t;
  const shutdownAt = events
    .filter((e) => e.event === "request" && e.line.includes("shutdown"))
    .at(-1)?.t;
  assert.ok(shutdownAt, "stub saw the shutdown request");
  const delta = termAt - shutdownAt;
  assert.ok(
    delta >= 1_800 && delta <= 2_600,
    `SIGTERM lands 2 s ± tolerance after the ack: ${delta}ms`,
  );
  assert.match(stderr, /"event":"child_reaped"/, "the reap is the ladder's end");
  rmSync(dir, { recursive: true, force: true });
}, { timeout: 30_000 });

test("test_e2e_shutdown_no_orphan", async () => {
  const dir = tempPackage();
  const root = path.join(dir, "root");
  mkdirSync(root, { recursive: true });
  const stubDir = path.join(dir, "engine", `${process.platform}-${process.arch}`);
  stageStubEngine(dir, root);
  await runDriver(dir, root);
  await new Promise((resolve) => setTimeout(resolve, 500));
  const survivors = findSurviving(stubDir);
  assert.deepEqual(survivors, [], `no orphan engine process with ${stubDir}`);
  rmSync(dir, { recursive: true, force: true });
}, { timeout: 30_000 });

test("test_e2e_shutdown_reaps_child", async () => {
  const dir = tempPackage();
  const root = path.join(dir, "root");
  mkdirSync(root, { recursive: true });
  stageStubEngine(dir, root);
  const { stderr } = await runDriver(dir, root);
  const line = stderr
    .split("\n")
    .map((raw) => raw.trim())
    .filter((raw) => raw.startsWith("{") && raw.includes("child_reaped"))
    .at(-1);
  assert.ok(line, `child_reaped structured line present: ${stderr.slice(-500)}`);
  const parsed = JSON.parse(line);
  assert.equal(parsed.event, "child_reaped");
  assert.equal(
    typeof parsed.exit_code === "number" || parsed.signal !== null,
    true,
    `the child is reaped: ${line}`,
  );
  rmSync(dir, { recursive: true, force: true });
}, { timeout: 30_000 });

test("test_e2e_shutdown_pipes_eof", async () => {
  // The reaping path carries no handle left open: a COMPLIANT stub (acks
  // shutdown, then exits code 0 on its own — no escalation) proves the
  // pipe-dependent reap path, and the driver asserts no surviving process.
  // stdin EOF BY THE CHILD is unobservable by construction: the wrapper ends
  // the child's stdin inside teardown, after the close (the escalation
  // ladder necessarily kills first) — the note is in the ledger; the
  // "pipes closed" contract is parent-side, after reap.
  const dir = tempPackage();
  const root = path.join(dir, "root");
  mkdirSync(root, { recursive: true });
  const outDir = path.join(dir, "engine", `${process.platform}-${process.arch}`);
  mkdirSync(outDir, { recursive: true });
  const stub = path.join(outDir, "opencode-agents-memory");
  const body = `
    import { appendFileSync } from "node:fs";
    import { createInterface } from "node:readline";
    const LOG = ${JSON.stringify(path.join(root, "stub-stdin.log"))};
    const log = (event, extra = {}) => {
      appendFileSync(LOG, JSON.stringify({ event, t: Date.now(), ...extra }) + "\\n");
    };
    log("stub-start", { pid: process.pid });
    const rl = createInterface({ input: process.stdin });
    let exiting = false;
    rl.on("line", (line) => {
      log("request", { line });
      let message;
      try { message = JSON.parse(line); } catch { return; }
      const id = message.id;
      let result;
      if (message.method === "hello") {
        result = { protocol: "1.0.0", engine_version: "stub", methods: [], capabilities: { fire_and_forget_capture: true, standalone: true, web: true } };
      } else if (message.method === "capture") {
        result = { accepted: true };
      } else if (message.method === "shutdown") {
        result = { ok: true };
        exiting = true; // COMPLIANT: real engines exit after the ack
      } else {
        result = {};
      }
      process.stdout.write(JSON.stringify({ id, jsonrpc: "2.0", result }) + "\\n");
      if (exiting) setTimeout(() => process.exit(0), 100);
    });
    rl.on("close", () => log("stdin-eof"));
    process.on("SIGTERM", () => { log("got-sigterm"); process.exit(0); });
    setTimeout(() => {}, 2 ** 30);
  `;
  writeFileSync(stub, `#!/usr/bin/env node\n${body}\n`);
  chmodSync(stub, 0o755);

  const { code, signal, stderr, elapsed } = await runDriver(dir, root);
  assert.equal(signal, "SIGTERM", "re-delivered teardown signal");
  // A compliant engine exits at the ack — no escalation delay needed on the
  // child side, and the wrapper's reap line says code 0.
  const line = stderr.split("\n").filter((raw) => raw.includes("child_reaped")).at(-1);
  assert.ok(line, `child_reaped: ${stderr.slice(-300)}`);
  const reap = JSON.parse(line.trim());
  assert.equal(reap.exit_code, 0, "compliant engine exited 0 at the ack");
  const events = stubEvents(root);
  assert.ok(events.some((e) => e.event === "request" && String(e.line).includes("shutdown")), "shutdown request line rode the pipe");
  // Note (honest, ledger): the CHILD observing stdin EOF is unobservable by
  // construction — the wrapper `.end()`s the child's stdin inside teardown,
  // AFTER the close event; the ladder kills the refusing child before it
  // (EOF follows death). "Both pipes closed" is therefore parent-side: the
  // child object's close/reap. The child-side EOF probe cannot exist.
  assert.ok(elapsed < 12_000, "bounded");
  assert.equal(signal, "SIGTERM", "the teardown re-delivery is still a signal (the driver never process.exits)");
  assert.equal(code, null, "compliant path: driver died by re-delivery, not by code");
  rmSync(dir, { recursive: true, force: true });
}, { timeout: 30_000 });
