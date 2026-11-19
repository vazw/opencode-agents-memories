import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import crypto from "node:crypto";
import { assertReaped, runDriver, stagePackageTree, stageStubEngine } from "./harness/bridge.mjs";

const PKG = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OWNER = fs.realpathSync(os.userInfo().homedir);
const LOG = path.join(os.userInfo().homedir, ".opencode-mem", "opencode-mem.log");

// Each case is its own process (runDriver). `injectMode` selects the stub
// engine answer: "success" → empty block (inject.received status "empty"),
// "error" → -32001 (inject.received status "error").
function driver(kind) {
  return `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
if (!process.env.DRIVER_STORAGE_ROOT) { process.stderr.write("driver refused: DRIVER_STORAGE_ROOT unset\\n"); process.exit(2); }
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const { server } = await import(pathToFileURL(path.join(process.argv[2], "dist/plugin.js")));
const input = { client: {}, directory: "/work/project" };
if (process.env.HOST_CONTRACT) input.hostContract = process.env.HOST_CONTRACT;
const { hooks } = await server(input);
const events = Number(process.env.EVENTS ?? 1);
for (let i = 0; i < events; i++) {
  if (i === 1 && process.env.UNBLOCK_HOME) {
    fs.rmSync(process.env.UNBLOCK_HOME);
    fs.mkdirSync(process.env.UNBLOCK_HOME);
  }
  await hooks["chat.message"]({ sessionID: process.env.SESSION_ID ?? "ses-1" }, { message: { parts: [{ type: "text", text: "hello" }] } });
}
fs.writeFileSync(process.env.RESULT, JSON.stringify({ ok: true }));
process.exit(0);
`;
}

function runCase({ name, injectMode, env = {}, cwdHome }) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-log-sink-"));
  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "manifest.json");
  const requestLogPath = path.join(tmp, "requests.ndjson");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, driver(name));
  const home = cwdHome ?? path.join(tmp, "home");
  fs.mkdirSync(home, { recursive: true });
  return { tmp, pkgDir, driverPath, home, env, manifestPath };
}

test("test_sink_appends_inject_received_to_engine_log", { timeout: 30000 }, async () => {
  const c = runCase({ name: "append", injectMode: "success" });
  const result = path.join(c.tmp, "result.json");
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, HOME: c.home, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"),
    RESULT: result, OPENCODE_MEM_WEB_PORT: process.env.OPENCODE_MEM_WEB_PORT,
  });
  assert.equal(r.code, 0, r.stderr);
  const logPath = path.join(c.home, ".opencode-mem", "opencode-mem.log");
  const text = fs.readFileSync(logPath, "utf8");
  assert.match(text, /\] inject\.received: \{/);
  await assertReaped(c.manifestPath);
});

test("test_sink_line_uses_the_engine_log_envelope", { timeout: 30000 }, async () => {
  const c = runCase({ name: "envelope", injectMode: "success" });
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, HOME: c.home, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"),
    RESULT: path.join(c.tmp, "result.json"), OPENCODE_MEM_WEB_PORT: process.env.OPENCODE_MEM_WEB_PORT,
  });
  assert.equal(r.code, 0, r.stderr);
  const line = fs.readFileSync(path.join(c.home, ".opencode-mem", "opencode-mem.log"), "utf8")
    .split("\n").find((l) => l.includes("inject.received: {"));
  assert.ok(line, "inject.received line present");
  assert.doesNotMatch(line, /\[object Object\]/);
  // engineStderrLogLine moves `event` into the message slot, so the payload
  // keeps component and fields but not `event`.
  const json = JSON.parse(line.slice(line.indexOf(": {") + 2));
  assert.equal(json.component, "opencode-mem-plugin");
  assert.equal(json.level, "info");
  assert.equal(json.status, "empty");
  await assertReaped(c.manifestPath);
});

test("test_sink_failure_never_throws_and_warns_once", { timeout: 30000 }, async () => {
  const c = runCase({ name: "failure", injectMode: "success" });
  // A regular file where the home directory should be: mkdir fails with
  // ENOTDIR, so the sink's failure path runs. (/proc/self/nonexistent stalls
  // the stub engine launch and times out.)
  const unwritableHome = path.join(c.tmp, "home-file");
  fs.writeFileSync(unwritableHome, "");
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, HOME: unwritableHome, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"),
    RESULT: path.join(c.tmp, "result.json"), OPENCODE_MEM_WEB_PORT: process.env.OPENCODE_MEM_WEB_PORT,
    EVENTS: "3",
  });
  assert.equal(r.code, 0, r.stderr);
  const warns = r.stderr.split("\n").filter((l) => l.includes('"event":"log_sink_unwritable"'));
  assert.equal(warns.length, 1, r.stderr);
  await assertReaped(c.manifestPath);
});

test("test_sink_retries_after_a_failed_write", { timeout: 30000 }, async () => {
  const c = runCase({ name: "retry", injectMode: "success" });
  const blockedHome = path.join(c.tmp, "home-file");
  fs.writeFileSync(blockedHome, "");
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, HOME: blockedHome, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"),
    RESULT: path.join(c.tmp, "result.json"), OPENCODE_MEM_WEB_PORT: process.env.OPENCODE_MEM_WEB_PORT,
    EVENTS: "2", UNBLOCK_HOME: blockedHome,
  });
  assert.equal(r.code, 0, r.stderr);
  const logPath = path.join(blockedHome, ".opencode-mem", "opencode-mem.log");
  assert.ok(fs.existsSync(logPath), "the sink never recovered");
  const text = fs.readFileSync(logPath, "utf8");
  assert.match(text, /\] inject\.received: \{/);
  await assertReaped(c.manifestPath);
});

test("test_inject_events_carry_host_contract", { timeout: 60000 }, async () => {
  for (const [hostContract, expected] of [[undefined, "server"], ["setup", "setup"]]) {
    for (const injectMode of ["error"]) {
      const c = runCase({ name: "contract", injectMode });
      const r = await runDriver(c.driverPath, [c.pkgDir], {
        ...process.env, HOME: c.home, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"),
        RESULT: path.join(c.tmp, "result.json"), OPENCODE_MEM_WEB_PORT: process.env.OPENCODE_MEM_WEB_PORT,
        ...(hostContract ? { HOST_CONTRACT: hostContract } : {}),
      });
      assert.equal(r.code, 0, r.stderr);
      const text = fs.readFileSync(path.join(c.home, ".opencode-mem", "opencode-mem.log"), "utf8");
      const received = text.split("\n").filter((l) => l.includes("inject.received:"));
      assert.ok(received.length >= 1, text);
      for (const l of received) {
        const json = JSON.parse(l.slice(l.indexOf(": {") + 2));
        assert.equal(json.host_contract, expected);
      }
      await assertReaped(c.manifestPath);
    }
  }
});

test("test_sink_path_is_engineLogPath_of_home", async () => {
  const target = process.env.PLUGIN_PATH ?? "src/plugin.js";
  const { engineLogPath } = await import(pathToFileURL(path.resolve(target)).href);
  assert.equal(engineLogPath("/x"), path.join("/x", ".opencode-mem", "opencode-mem.log"));
});

// Issue 157: runDriver must never hand a driver the owner HOME.
function noopDriver(tmp) {
  const p = path.join(tmp, "noop.mjs");
  fs.writeFileSync(p, "process.exit(0);\n");
  return p;
}

test("test_run_driver_default_home_is_never_owner", { timeout: 30000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-157-"));
  const run = await runDriver(noopDriver(tmp), [], { ...process.env, DRIVER_STORAGE_ROOT: path.join(tmp, "root") });
  assert.equal(typeof run.home, "string");
  assert.notEqual(fs.realpathSync(run.home), OWNER, `owner home spawned: ${OWNER}`);
  assert.ok(fs.statSync(run.home).isDirectory());
});

test("test_run_driver_rejects_symlinked_owner_home", { timeout: 30000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-157-"));
  const alias = path.join(tmp, "alias");
  fs.symlinkSync(OWNER, alias);
  const run = await runDriver(noopDriver(tmp), [], { ...process.env, HOME: alias, DRIVER_STORAGE_ROOT: path.join(tmp, "root") });
  assert.notEqual(fs.realpathSync(run.home), OWNER, `symlink alias reached owner home: ${run.home}`);
});

test("test_run_driver_treats_empty_home_as_absent", { timeout: 30000 }, async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-157-"));
  const run = await runDriver(noopDriver(tmp), [], { ...process.env, HOME: "", DRIVER_STORAGE_ROOT: path.join(tmp, "root") });
  assert.equal(typeof run.home, "string");
  assert.notEqual(run.home, "");
  assert.notEqual(fs.realpathSync(run.home), OWNER);
});

test("test_run_driver_surfaces_bad_home", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-157-"));
  const plain = path.join(tmp, "plainfile");
  fs.writeFileSync(plain, "");
  assert.throws(
    () => runDriver(noopDriver(tmp), [], { ...process.env, HOME: path.join(plain, "sub"), DRIVER_STORAGE_ROOT: path.join(tmp, "root") }),
    /ENOTDIR|ENOENT|ELOOP|EACCES/,
  );
});

// T3: a driver with no HOME key logs its inject under the run's HOME, never the owner log.
test("test_driver_logs_under_temp_home", { timeout: 30000 }, async () => {
  const c = runCase({ name: "t3", injectMode: "success" });
  const id = "t157probe-" + crypto.randomUUID();
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"), SESSION_ID: id,
    RESULT: path.join(c.tmp, "result.json"),
  });
  assert.equal(r.code, 0, r.stderr);
  assert.equal(typeof r.home, "string");
  const runLog = path.join(r.home, ".opencode-mem", "opencode-mem.log");
  assert.ok(fs.existsSync(runLog), `no run log at ${runLog}`);
  const runText = fs.readFileSync(runLog, "utf8");
  assert.ok(runText.split("\n").some((l) => l.includes("inject.requested") && l.includes(`"session_id":"${id}"`)), "inject.requested for probe id missing from run log");
  const owner = fs.existsSync(LOG) ? fs.readFileSync(LOG, "utf8") : "";
  assert.ok(!owner.includes(id), "probe session id reached the owner live log");
  await assertReaped(c.manifestPath);
});

// T6: the real staged engine, started by the production wrapper, logs under the run's HOME.
test("test_wrapper_engine_child_logs_under_run_home", { timeout: 60000 }, async () => {
  const REAL = path.join(PKG, "engine", `${process.platform}-${process.arch}`, "opencode-agents-memory");
  if (!fs.existsSync(REAL)) throw new Error("staged engine missing; run plugin/scripts/stage-engine.sh");
  const c = runCase({ name: "t6", injectMode: "success" });
  const engineDir = path.join(c.pkgDir, "engine", `${process.platform}-${process.arch}`);
  fs.mkdirSync(engineDir, { recursive: true });
  fs.copyFileSync(REAL, path.join(engineDir, "opencode-agents-memory"));
  fs.chmodSync(path.join(engineDir, "opencode-agents-memory"), 0o755);
  // The lock is a lifetime lock released when the engine exits, so the driver
  // observes it while the engine is alive and records the answer in RESULT.
  fs.writeFileSync(c.driverPath, driver("t6").replace(
    'fs.writeFileSync(process.env.RESULT, JSON.stringify({ ok: true }));',
    `let lockSeen = false;
const lockFile = path.join(process.env.DRIVER_STORAGE_ROOT, ".engine.lock");
for (let i = 0; i < 100 && !lockSeen; i++) {
  lockSeen = fs.existsSync(lockFile);
  if (!lockSeen) await new Promise((resolve) => setTimeout(resolve, 100));
}
fs.writeFileSync(process.env.RESULT, JSON.stringify({ ok: true, lockSeen }));`,
  ));
  const root = path.join(c.tmp, "root");
  const result = path.join(c.tmp, "result.json");
  const t0 = Date.now();
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, DRIVER_STORAGE_ROOT: root, SESSION_ID: "t157probe-" + crypto.randomUUID(),
    RESULT: result,
  }, 20000);
  assert.equal(r.code, 0, r.stderr);
  assert.equal(typeof r.home, "string");
  const banner = /^--- Session started: (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}[+-]\d{2}:\d{2}) ---$/m;
  const runText = fs.existsSync(path.join(r.home, ".opencode-mem", "opencode-mem.log"))
    ? fs.readFileSync(path.join(r.home, ".opencode-mem", "opencode-mem.log"), "utf8") : "";
  const m = banner.exec(runText);
  assert.ok(m, `no engine banner in run log; driver stderr: ${r.stderr}`);
  assert.ok(Date.parse(m[1]) >= t0, "banner predates this run");
  assert.equal(JSON.parse(fs.readFileSync(result, "utf8")).lockSeen, true, "engine did not lock the run's storage root");
  const owner = fs.existsSync(LOG) ? fs.readFileSync(LOG, "utf8") : "";
  for (const line of owner.split("\n")) {
    const om = banner.exec(line);
    if (om) assert.ok(Date.parse(om[1]) < t0, "engine banner from this run reached the owner live log");
  }
});

// T9s: the service.json tripwire must see a same-size, same-mtime content change.
function fingerprint(file) {
  const st = fs.statSync(file);
  return { sha: crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex"), size: st.size, mtimeMs: st.mtimeMs };
}
function changed(a, b) {
  return a.sha !== b.sha || a.size !== b.size || a.mtimeMs !== b.mtimeMs;
}
test("test_service_tripwire_detects_same_mtime_content_change", () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-157-"));
  const file = path.join(tmp, "service.json");
  fs.writeFileSync(file, "AAAA");
  const before = fingerprint(file);
  fs.writeFileSync(file, "BBBB");
  fs.utimesSync(file, before.mtimeMs / 1000, before.mtimeMs / 1000);
  const after = fingerprint(file);
  assert.equal(after.size, before.size);
  assert.equal(after.mtimeMs, before.mtimeMs);
  assert.notEqual(after.sha, before.sha);
  assert.equal(changed(before, after), true);
});
