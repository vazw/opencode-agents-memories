import test, { after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { assertReaped, runDriver, stagePackageTree, stageStubEngine } from "./harness/bridge.mjs";

// Module-anchored scratch: here is plugin/tests; two `..` reach the repo root.
const here = path.dirname(fileURLToPath(import.meta.url));
const scratchRoot = path.resolve(here, "..", "..", ".tmp");
const made = [];
after(() => { for (const d of made) fs.rmSync(d, { recursive: true, force: true }); });

// Issue 145, Step 1b: one warning toast per failure reason per process.
// Every case runs its own driver (its own process). The driver below is local
// to this file on purpose: it needs the stderr tee, a rejecting showToast, the
// idle event envelope, a session fixture, and an answer-marker deadline poll,
// none of which the shared DRIVER_SOURCE provides.

const TURN = [
  { info: { id: "u1", role: "user" }, parts: [{ type: "text", text: "fix the parser" }] },
  { info: { id: "a1", role: "assistant" }, parts: [{ type: "text", text: "done" }] },
];

const CAPTURE_TEXT = "Memory capture failed.";
const INJECT_TEXT = "Memory injection failed; memory may be disabled.";

// The driver reads a JSON spec from SPEC_PATH:
//   events: [{ kind: "idle", sessionID } | { kind: "chat", sessionID }]
//   showToast: "counting" | "rejecting"
//   expectCaptures / expectInjects: wire counts the poll waits for
//   expectAnswers: { capture, inject } reply markers the poll waits for
const DRIVER = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;
const spec = JSON.parse(fs.readFileSync(process.env.SPEC_PATH, "utf8"));
const stderrLog = [];
const realStderrWrite = process.stderr.write.bind(process.stderr);
process.stderr.write = (chunk, ...rest) => {
  stderrLog.push(typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8"));
  return realStderrWrite(chunk, ...rest);
};
const pkgDir = process.argv[2];
const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
let toastCalls = 0;
const toastBodies = [];
const client = {
  session: {
    get: async () => ({ agent: "build" }),
    messages: async () => ({ data: spec.turn }),
  },
  tui: {
    showToast: (input) => {
      toastCalls += 1;
      toastBodies.push(input?.body?.message ?? null);
      if (spec.showToast === "rejecting") return Promise.reject(new Error("toast refused"));
      return Promise.resolve({ data: true });
    },
  },
};
const { hooks } = await mod.default.server({ client, directory: spec.directory });
const record = { parts: null };
const readWire = () => {
  let text = "";
  try { text = fs.readFileSync(process.env.STUB_LOG, "utf8"); } catch {}
  return text;
};
const count = (re) => (readWire().match(re) ?? []).length;
const answered = (method) => {
  let text = "";
  try { text = fs.readFileSync(process.env.ANSWER_LOG, "utf8"); } catch {}
  return text.split("\\n").filter((l) => l.includes('"answered":"' + method + '"')).length;
};
for (const ev of spec.events) {
  if (ev.kind === "idle") {
    hooks.event({ event: { properties: { type: "session.idle", sessionID: ev.sessionID } } });
  } else {
    const output = { message: { parts: [{ type: "text", text: "hello" }] } };
    await hooks["chat.message"]({ sessionID: ev.sessionID }, output);
    record.parts = output.message.parts;
  }
}
const HARD_MS = 5000;
const QUIET_MS = 150;
const t0 = Date.now();
let lastChange = Date.now();
let lastToastCount = 0;
let settled = false;
for (;;) {
  if (toastCalls !== lastToastCount) { lastToastCount = toastCalls; lastChange = Date.now(); }
  const wireDone = count(/"method":"capture"/g) >= spec.expectCaptures && count(/"method":"inject"/g) >= spec.expectInjects;
  const answersDone = answered("capture") >= spec.expectAnswers.capture && stderrLog.join("").split('"event":"inject.received"').length - 1 >= spec.expectAnswers.inject;
  if (wireDone && answersDone && Date.now() - lastChange >= QUIET_MS) { settled = true; break; }
  if (Date.now() - t0 >= HARD_MS) break;
  await new Promise((resolve) => setTimeout(resolve, 20));
}
record.settled = settled;
record.elapsedMs = Date.now() - t0;
record.toastCalls = toastCalls;
record.toastBodies = toastBodies;
record.stderr = stderrLog.join("");
record.answers = { capture: answered("capture") };
fs.writeFileSync(process.env.RESULT, JSON.stringify(record));
process.stderr.write = realStderrWrite;
process.exit(0);
`;

function stage({ captureMode = "accepted", injectMode = "error", wireErrors = {} }) {
  fs.mkdirSync(scratchRoot, { recursive: true });
  const tmp = fs.mkdtempSync(path.join(scratchRoot, "toast-145-")); made.push(tmp);
  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "manifest.json");
  const requestLogPath = path.join(tmp, "requests.ndjson");
  const answerLogPath = path.join(tmp, "answers.ndjson");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode, captureMode, wireErrors, answerLogPath });
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER);
  const home = path.join(tmp, "home");
  fs.mkdirSync(home, { recursive: true });
  return { tmp, pkgDir, driverPath, home, manifestPath, requestLogPath, answerLogPath };
}

async function runToastCase(name, spec, opts) {
  const c = stage(opts);
  const specPath = path.join(c.tmp, "spec.json");
  const resultPath = path.join(c.tmp, "result.json");
  fs.writeFileSync(specPath, JSON.stringify({ directory: "/work/proj", turn: TURN, ...spec }));
  const r = await runDriver(c.driverPath, [c.pkgDir], {
    ...process.env, HOME: c.home, DRIVER_STORAGE_ROOT: path.join(c.tmp, "root"),
    SPEC_PATH: specPath, RESULT: resultPath, STUB_LOG: c.requestLogPath, ANSWER_LOG: c.answerLogPath,
    OPENCODE_MEM_WEB_PORT: process.env.OPENCODE_MEM_WEB_PORT,
  }, 30000);
  assert.equal(r.code, 0, r.stderr);
  const record = JSON.parse(fs.readFileSync(resultPath, "utf8"));
  await assertReaped(c.manifestPath);
  return { ...c, record, stderr: r.stderr, name };
}

const wireCount = (c, method) => (fs.readFileSync(c.requestLogPath, "utf8").match(new RegExp('"method":"' + method + '"', "g")) ?? []).length;
const answerCount = (c, method) => {
  let text = "";
  try { text = fs.readFileSync(c.answerLogPath, "utf8"); } catch {}
  return text.split("\n").filter((l) => l.includes('"answered":"' + method + '"')).length;
};

test("test_capture_refusal_toasts_once", { timeout: 60000 }, async () => {
  const c = await runToastCase("refusal", { events: [{ kind: "idle", sessionID: "ses-A" }], expectCaptures: 1, expectInjects: 0, expectAnswers: { capture: 1, inject: 0 }, showToast: "counting" }, { captureMode: "refused" });
  assert.equal(c.record.settled, true, c.stderr);
  assert.equal(wireCount(c, "capture"), 1);
  assert.equal(answerCount(c, "capture"), 1);
  assert.equal(c.record.toastCalls, 1, "toast assertion");
  assert.deepEqual(c.record.toastBodies, [CAPTURE_TEXT]);
});

test("test_capture_throw_toasts_once", { timeout: 60000 }, async () => {
  const c = await runToastCase("throw", { events: [{ kind: "idle", sessionID: "ses-A" }], expectCaptures: 1, expectInjects: 0, expectAnswers: { capture: 1, inject: 0 }, showToast: "counting" }, { wireErrors: { capture: { code: -32603, message: "degraded", data: null } } });
  assert.equal(c.record.settled, true, c.stderr);
  assert.equal(wireCount(c, "capture"), 1);
  assert.equal(answerCount(c, "capture"), 1);
  assert.equal(c.record.toastCalls, 1, "toast assertion");
  assert.deepEqual(c.record.toastBodies, [CAPTURE_TEXT]);
});

test("test_same_reason_different_sessions_one_toast", { timeout: 60000 }, async () => {
  const c = await runToastCase("two-sessions", { events: [{ kind: "idle", sessionID: "ses-A" }, { kind: "idle", sessionID: "ses-B" }], expectCaptures: 2, expectInjects: 0, expectAnswers: { capture: 2, inject: 0 }, showToast: "counting" }, { captureMode: "refused" });
  assert.equal(c.record.settled, true, c.stderr);
  assert.equal(wireCount(c, "capture"), 2);
  assert.equal(answerCount(c, "capture"), 2);
  assert.equal(c.record.toastCalls, 1, "toast assertion");
});

test("test_two_reasons_two_toasts_with_their_own_texts", { timeout: 60000 }, async () => {
  const c = await runToastCase("two-reasons", { events: [{ kind: "chat", sessionID: "ses-A" }, { kind: "idle", sessionID: "ses-A" }], expectCaptures: 1, expectInjects: 1, expectAnswers: { capture: 1, inject: 1 }, showToast: "counting" }, { captureMode: "refused", injectMode: "error" });
  assert.equal(c.record.settled, true, c.stderr);
  assert.equal(wireCount(c, "capture"), 1);
  assert.equal(wireCount(c, "inject"), 1);
  assert.equal(answerCount(c, "capture"), 1);
  assert.equal(c.stderr.split('"event":"inject.received"').length - 1, 1);
  assert.equal(c.record.toastCalls, 2, "toast assertion");
  assert.equal(c.record.toastBodies.length, 2);
  assert.ok(c.record.toastBodies.includes(CAPTURE_TEXT), "capture body");
  assert.ok(c.record.toastBodies.includes(INJECT_TEXT), "inject body");
});

test("test_inject_empty_block_does_not_toast", { timeout: 60000 }, async () => {
  const c = await runToastCase("empty", { events: [{ kind: "chat", sessionID: "ses-A" }], expectCaptures: 0, expectInjects: 1, expectAnswers: { capture: 0, inject: 1 }, showToast: "counting" }, { injectMode: "success" });
  assert.equal(c.record.settled, true, c.stderr);
  assert.equal(wireCount(c, "inject"), 1);
  assert.match(c.stderr, /"event":"inject\.received".*"status":"empty"/);
  assert.equal(c.record.toastCalls, 0, "toast assertion");
});

test("test_inject_error_keeps_single_toast", { timeout: 60000 }, async () => {
  const c = await runToastCase("inject-error", { events: [{ kind: "chat", sessionID: "ses-A" }], expectCaptures: 0, expectInjects: 1, expectAnswers: { capture: 0, inject: 1 }, showToast: "counting" }, { injectMode: "error" });
  assert.equal(c.record.settled, true, c.stderr);
  assert.match(c.stderr, /"event":"inject\.received".*"status":"error"/);
  assert.equal(c.record.toastCalls, 1, "toast assertion");
  assert.deepEqual(c.record.toastBodies, [INJECT_TEXT]);
});

test("test_toast_failure_does_not_change_the_hook_output", { timeout: 120000 }, async () => {
  const counting = await runToastCase("counting", { events: [{ kind: "chat", sessionID: "ses-A" }], expectCaptures: 0, expectInjects: 1, expectAnswers: { capture: 0, inject: 1 }, showToast: "counting" }, { injectMode: "error" });
  const rejecting = await runToastCase("rejecting", { events: [{ kind: "chat", sessionID: "ses-A" }], expectCaptures: 0, expectInjects: 1, expectAnswers: { capture: 0, inject: 1 }, showToast: "rejecting" }, { injectMode: "error" });
  for (const c of [counting, rejecting]) {
    assert.equal(c.record.settled, true, c.stderr);
    assert.equal(c.record.toastCalls, 1, "toast assertion");
    assert.deepEqual(c.record.parts, [{ type: "text", text: "hello" }]);
  }
  assert.equal(JSON.stringify(counting.record.parts), JSON.stringify(rejecting.record.parts));
  assert.doesNotMatch(rejecting.stderr, /UnhandledPromiseRejection/);
});
