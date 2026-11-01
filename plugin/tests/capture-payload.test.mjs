// Capture attribution — the wrapper supplies the agent identity.
//
// `fireIdleCapture` must send a payload the engine accepts: the resolved
// `agent_id`, the hook's own `session_id`, and the host `project_id`. When an
// identity cannot be resolved it sends NOTHING and names the drop on stderr.
//
// The driver is local to this file: the shared harness driver fires a fixed
// part list and these cases need their own; `bridge.mjs` is shared by five
// suites, so no shared file is edited for a fixture need.
//
// Settle discipline: a zero-capture case has no positive wire signal (the
// capture is exactly what must not appear), so a "capture count" poll is
// satisfied at t = 0 and the driver would exit while `fireIdleCapture` is still
// inside `await ensureStarted()` (`void`-ed at plugin.js). The driver therefore
// polls a POSITIVE signal (the wrapper's own named stderr event, or the capture
// line itself) under a hard deadline and records `settled` / `deadline`.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

const FIXTURE_PATH = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "fixtures",
  "idle-capture-payload.json"
);

const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

// Tee stderr BEFORE the import. The guards' only observable is their own
// \`logWarn\` line (process.stderr.write), so the positive settle signal is that
// line itself. A capture COUNT cannot be the signal for a zero-capture case: 0
// is already true at t = 0 and \`void fireIdleCapture\` lets the process exit
// inside \`await ensureStarted()\`.
const stderrLog = [];
const realStderrWrite = process.stderr.write.bind(process.stderr);
process.stderr.write = (chunk, ...rest) => {
  stderrLog.push(typeof chunk === "string" ? chunk : Buffer.from(chunk).toString("utf8"));
  return realStderrWrite(chunk, ...rest);
};

const pkgDir = process.argv[2];
const fixture = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;

const hooksRef = { current: null };
let retargeted = false;
const client = { session: {
  get: async (args) => {
    const id = args?.path?.id;
    const known = fixture.sessions?.[id];
    return known === undefined ? { data: null } : { data: { id, ...known } };
  },
  messages: async (args) => {
    const id = args?.path?.id;
    if (fixture.countMessagesPath) {
      fs.appendFileSync(fixture.countMessagesPath, id + "\\n");
    }
    if (!retargeted && fixture.retarget && id === fixture.retarget.onMessagesFor) {
      retargeted = true; // reentrant retarget: fires while fireIdleCapture is suspended
      await hooksRef.current.event({ event: { properties: fixture.retarget.fire } });
    }
    return { data: fixture.messages?.[id] ?? [] };
  },
}, prompt: async () => ({}) };

const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
// server() returns { hooks }: the hook map is one level deeper.
const { hooks } = await mod.default.server({ client, directory: fixture.directory ?? undefined, project: null });
hooksRef.current = hooks;
for (const event of fixture.events) await hooks.event({ event: { properties: event } });

const HARD_MS = 5000;
const captureCount = () => {
  let text = "";
  try { text = fs.readFileSync(process.env.STUB_LOG, "utf8"); } catch {}
  return (text.match(/"method":"capture"/g) ?? []).length;
};
const t0 = Date.now();
let settled = false;
for (;;) {
  settled = fixture.expectEvent
    ? stderrLog.join("").includes('"event":"' + fixture.expectEvent + '"')
    : captureCount() >= fixture.expectCaptures;
  if (settled || Date.now() - t0 >= HARD_MS) break;
  await new Promise((resolve) => setTimeout(resolve, 20));
}
fs.writeFileSync(process.env.DRIVER_DONE_PATH, settled ? "settled" : "deadline");
// Flush the piped stderr, THEN exit. Both halves were measured:
//  - the spawned engine child holds the driver's event loop open (piped stdio,
//    nothing unrefs it; the stub exits only on SIGTERM or stdin EOF), so
//    without an explicit exit the run reaches the harness's 30 s bound and is
//    SIGKILLed (12017 ms, code=null, signal=SIGKILL, versus 118 ms, code=0);
//  - a bare exit truncates a piped stderr burst (1,200,000 bytes written ->
//    152,040 delivered; 1,200,000 after this empty-write flush). Below the pipe
//    buffer nothing is lost either way.
await new Promise((resolve) => process.stderr.write("", resolve));
process.exit(0);
`;

async function runCapture(t, fixture) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-capture-payload-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode: "success" });
  t.after(() => assertReaped(manifestPath));

  const donePath = path.join(tmp, "driver-done.txt");
  const messagesCountPath = path.join(tmp, "messages-calls.txt");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify({ ...fixture, countMessagesPath: messagesCountPath }));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);

  const { code, signal, stderr, timedOut } = await runDriver(driverPath, [pkgDir, fixturePath], {
    ...process.env,
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: donePath,
    STUB_LOG: requestLogPath,
  });
  assert.equal(code, 0, `the capture driver failed (signal=${signal}, timedOut=${timedOut}): ${stderr}`);
  const requestLog = fs.existsSync(requestLogPath) ? fs.readFileSync(requestLogPath, "utf8") : "";
  const wire = requestLog.split("\n").filter(Boolean).map((line) => JSON.parse(line).got);
  const marker = fs.existsSync(donePath) ? fs.readFileSync(donePath, "utf8") : "missing";
  const messagesCalls = fs.existsSync(messagesCountPath)
    ? fs.readFileSync(messagesCountPath, "utf8").split("\n").filter(Boolean)
    : [];
  return { code, stderr, wire, marker, requestLog, messagesCalls };
}

function captures(run) {
  return run.wire.filter((m) => m && m.method === "capture");
}

function namedEvents(stderr, event) {
  const found = [];
  for (const line of stderr.split("\n")) {
    try {
      const parsed = JSON.parse(line);
      if (parsed && parsed.event === event) found.push(parsed);
    } catch {
      // node may emit non-JSON noise on the same stream
    }
  }
  return found;
}

const TURN = [
  { info: { id: "u1", role: "user" }, parts: [{ type: "text", text: "fix the parser" }] },
  { info: { id: "a1", role: "assistant" }, parts: [{ type: "text", text: "done" }] },
];

function caseA() {
  return {
    directory: "/work/proj",
    sessions: { "ses-A": { agent: "build" } },
    messages: { "ses-A": TURN },
    events: [{ type: "session.idle", sessionID: "ses-A" }],
    retarget: null,
    expectCaptures: 1,
    expectEvent: null,
  };
}

test("test_idle_capture_payload_carries_the_resolved_agent_id", { timeout: 30000 }, async (t) => {
  const run = await runCapture(t, caseA());
  const sent = captures(run);
  assert.equal(sent.length, 1, "one capture reached the wire");
  assert.equal(sent[0].params.agent_id, "build");
  assert.equal(sent[0].params.project_id, "/work/proj");
});

test("test_session_agent_needs_no_second_messages_call", { timeout: 30000 }, async (t) => {
  const run = await runCapture(t, caseA());
  assert.equal(captures(run).length, 1, "the capture was sent");
  assert.equal(run.messagesCalls.length, 1, "only the transcript fetch calls session.messages");
});

test("test_idle_capture_without_an_agent_sends_nothing_and_names_it", { timeout: 30000 }, async (t) => {
  const run = await runCapture(t, {
    ...caseA(),
    sessions: { "ses-A": {} },
    expectCaptures: 0,
    expectEvent: "capture.agent_unresolved",
  });
  assert.equal(captures(run).length, 0, "an unresolvable agent sends no capture");
  const named = namedEvents(run.stderr, "capture.agent_unresolved");
  assert.equal(named.length, 1, "exactly one named drop line");
  assert.equal(named[0].session_id, "ses-A");
  assert.equal(run.marker, "settled", "the named event settled the driver; a deadline is not a pass");
});

test("test_idle_capture_payload_stamps_the_hook_session_id", { timeout: 30000 }, async (t) => {
  // The fake `messages` for ses-A fires ses-B's idle event BEFORE returning, so
  // the process-global session id is re-latched while fireIdleCapture is
  // suspended between its host awaits. Firing the retarget from `session.get`
  // would be a dead fixture (the pre-fix code never calls it).
  const run = await runCapture(t, {
    ...caseA(),
    sessions: { "ses-A": { agent: "build" }, "ses-B": { agent: "review" } },
    messages: {
      "ses-A": TURN,
      "ses-B": [
        { info: { id: "u2", role: "user" }, parts: [{ type: "text", text: "unrelated turn" }] },
        { info: { id: "a2", role: "assistant" }, parts: [{ type: "text", text: "ok" }] },
      ],
    },
    retarget: { onMessagesFor: "ses-A", fire: { type: "session.idle", sessionID: "ses-B" } },
    expectCaptures: 2,
    expectEvent: null,
  });
  const sent = captures(run);
  const fixParser = sent.find((m) => m.params.transcript === "fix the parser");
  assert.ok(fixParser, "the ses-A capture reached the wire");
  assert.equal(fixParser.params.session_id, "ses-A");
  assert.equal(fixParser.params.agent_id, "build");
  const unrelated = sent.find((m) => m.params.transcript === "unrelated turn");
  assert.ok(unrelated, "anti-vacuity: the reentrant ses-B capture exists, so the retarget fired");
  assert.equal(unrelated.params.session_id, "ses-B");
});

// Cases D and E share ONE run (node:test runs a file's tests sequentially, so
// no lock is needed): a second run would double the 5 s worst case for no extra
// evidence, and the absence assertion in E covers the whole staged log.
let noDirectoryRun = null;
function runWithoutDirectory(t) {
  noDirectoryRun ??= runCapture(t, {
    ...caseA(),
    directory: null,
    expectCaptures: 0,
    expectEvent: "capture.project_unresolved",
  });
  return noDirectoryRun;
}

test("test_idle_capture_without_a_directory_sends_nothing", { timeout: 30000 }, async (t) => {
  const run = await runWithoutDirectory(t);
  assert.equal(captures(run).length, 0, "no host directory sends no capture");
  const named = namedEvents(run.stderr, "capture.project_unresolved");
  assert.equal(named.length, 1, "exactly one named drop line");
  assert.equal(named[0].session_id, "ses-A");
  assert.equal(run.marker, "settled", "the named event settled the driver; a deadline is not a pass");
});

test("test_project_id_absent_never_reaches_a_dead_letter", { timeout: 30000 }, async (t) => {
  // "no entry => no attempt => no dead letter" is a structural argument, not a
  // measurement. Its engine half is pinned by `test_empty_project_id_still_dead_letters`
  // (capture_wrapper_payload.rs), which shows what an empty project_id WOULD do:
  // together the pair is non-vacuous, and a mutant that sends anyway with
  // project_id omitted fails both. Inherits D's settle signal; no count poll.
  const run = await runWithoutDirectory(t);
  assert.equal(run.marker, "settled");
  assert.equal(run.requestLog.includes('"method":"capture"'), false, "the staged log holds no capture entry");
});

test("test_idle_capture_fixture_matches_the_live_payload", { timeout: 30000 }, async (t) => {
  // The committed fixture is the Rust seam's input (`include_str!` in
  // crates/mem-bridge/tests/capture_wrapper_payload.rs). It is regenerated here
  // from the live producer; the ONLY normalisation is `capture_id` (a real
  // UUIDv7 must never enter a committed file; the engine only requires it
  // non-blank).
  const run = await runCapture(t, caseA());
  const sent = captures(run);
  assert.equal(sent.length, 1);
  const live = { ...sent[0].params, capture_id: "cap-fixture-0001" };
  const committed = JSON.parse(fs.readFileSync(FIXTURE_PATH, "utf8"));
  assert.deepEqual(live, committed);
  assert.equal(committed.session_id, "ses-A");
});
