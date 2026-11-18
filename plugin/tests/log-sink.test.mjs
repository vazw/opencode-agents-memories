import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { assertReaped, runDriver, stagePackageTree, stageStubEngine } from "./harness/bridge.mjs";

const PKG = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// Each case is its own process (runDriver). `injectMode` selects the stub
// engine answer: "success" → empty block (inject.received status "empty"),
// "error" → -32001 (inject.received status "error").
function driver(kind) {
  return `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
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
  await hooks["chat.message"]({ sessionID: "ses-1" }, { message: { parts: [{ type: "text", text: "hello" }] } });
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
