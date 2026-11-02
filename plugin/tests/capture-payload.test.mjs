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
// Issue 98: the driver owns the hello-config file's contents (R7). Seed it
// with the fixture's startup value; an event carrying \`beforeHelloConfig\`
// rewrites it so the NEXT hello handshake — and only the next one — reads the
// new value.
if (fixture.helloConfig !== undefined) {
  fs.writeFileSync(fixture.helloConfigPath, JSON.stringify(fixture.helloConfig));
}
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
// D-98-14 / M1 — the replacement-child handoff waits for the dead child to be
// REAPED. bridge.mjs's own procedure: the stub appends {exited:<pid>} BEFORE
// process.exit, so that line proves nothing about the kernel having reaped the
// child. Poll the LAST spawned pid until its exited record exists AND
// process.kill(pid, 0) throws ESRCH — the point at which the wrapper's close
// handler (child = null, then teardownChild) is imminent. The single 20 ms turn
// after that is defence in depth for the stdio close, never the retirement
// signal. Without this wait, ensureStartedIn's fast path (bridgeState
// === "installed") writes the next capture into a dead pipe and the run
// reports 0 captures. A log-line-plus-one-turn form was measured to miss close
// in 93/200 loaded runs; this pid-gone form missed 0/200.
const lifecycleLog = process.env.STUB_LOG + ".lifecycle";
function lifecycleEntries() {
  let text = "";
  try { text = fs.readFileSync(lifecycleLog, "utf8"); } catch { return []; }
  const entries = [];
  for (const line of text.split("\\n")) {
    if (line === "") continue;
    try { entries.push(JSON.parse(line)); } catch { /* a torn concurrent append */ }
  }
  return entries;
}
async function waitForTheLastChildToBeReaped() {
  const deadline = Date.now() + 2000;
  for (;;) {
    const spawned = lifecycleEntries().filter((entry) => entry.spawned !== undefined);
    const last = spawned[spawned.length - 1];
    if (last !== undefined) {
      const logged = lifecycleEntries().some((entry) => entry.exited === last.spawned);
      let reaped = false;
      try {
        process.kill(last.spawned, 0);
      } catch (error) {
        reaped = error.code === "ESRCH";
      }
      if (logged && reaped) break;
    }
    // No spawn record yet, or not reaped within the bound: return and let the
    // case fail on its own assertions rather than hang here.
    if (Date.now() >= deadline) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  await new Promise((resolve) => setTimeout(resolve, 20));
}
for (const event of fixture.events) {
  if (event.beforeHelloConfig !== undefined) {
    await waitForTheLastChildToBeReaped();
    fs.writeFileSync(fixture.helloConfigPath, JSON.stringify(event.beforeHelloConfig));
  }
  if (event.hook === "chat.message") {
    await hooks["chat.message"]({ sessionID: "ses-A" }, { message: { id: "msg-1", parts: [{ type: "text", text: "ping" }] } });
  } else {
    await hooks.event({ event: { properties: event } });
  }
}

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
  // Issue 98: the stub re-reads this file per hello handshake (the skillsPath
  // idiom), so a replacement child can advertise a different value.
  const helloConfigPath = path.join(tmp, "hello-config.json");
  if (fixture.helloConfig !== undefined) {
    fs.writeFileSync(helloConfigPath, JSON.stringify(fixture.helloConfig));
  }
  stageStubEngine(pkgDir, {
    requestLogPath,
    manifestPath,
    injectMode: "success",
    helloConfig: fixture.helloConfig,
    helloConfigPath,
    diesAfterRequests: fixture.diesAfterRequests,
  });
  t.after(() => assertReaped(manifestPath));

  const donePath = path.join(tmp, "driver-done.txt");
  const messagesCountPath = path.join(tmp, "messages-calls.txt");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(
    fixturePath,
    JSON.stringify({ ...fixture, countMessagesPath: messagesCountPath, helloConfigPath })
  );
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

// ---------------------------------------------------------------------------
// Issue 98 — C9: the wrapper gates BEFORE any host call.
//
// The stub advertises `hello.result.config`; only the exact boolean `false`
// refuses. Every case here spreads `caseA()`, because without a resolvable
// session + agent a characterization row would drop its capture at
// `capture.agent_unresolved` and read as a false failure.
// ---------------------------------------------------------------------------

test(
  "test_idle_capture_is_skipped_when_the_engine_advertises_chat_messages_off",
  { timeout: 30000 },
  async (t) => {
    const run = await runCapture(t, {
      ...caseA(),
      helloConfig: { chat_message_enabled: false },
      expectCaptures: 0,
      expectEvent: "capture.chat_message_disabled",
    });
    // Negative assertions FIRST: the line proves the gate ran, the capture
    // count is what the criterion is about.
    const named = namedEvents(run.stderr, "capture.chat_message_disabled");
    assert.equal(named.length, 1, "exactly one named skip line");
    assert.equal(named[0].session_id, "ses-A");
    assert.equal(captures(run).length, 0, "a flag-off engine sends no capture");
    assert.equal(
      run.messagesCalls.length,
      0,
      "the gate returns BEFORE the transcript fetch"
    );
    assert.equal(
      run.marker,
      "settled",
      "the named event settled the driver; a deadline is not a pass"
    );
  }
);

test(
  "test_idle_capture_still_runs_when_the_engine_advertises_chat_messages_on",
  { timeout: 30000 },
  async (t) => {
    const run = await runCapture(t, {
      ...caseA(),
      helloConfig: { chat_message_enabled: true },
      expectCaptures: 1,
      expectEvent: null,
    });
    assert.equal(namedEvents(run.stderr, "capture.chat_message_disabled").length, 0);
    assert.equal(captures(run).length, 1, "the gate is a gate: `true` still captures");
    assert.equal(run.messagesCalls.length, 1, "the transcript is still fetched");
  }
);

test(
  "test_engine_predating_the_config_key_keeps_capture",
  { timeout: 30000 },
  async (t) => {
    // No `helloConfig` at all: the stub omits `config`, which is exactly the
    // pre-issue-98 handshake shape (fail-open).
    const run = await runCapture(t, {
      ...caseA(),
      expectCaptures: 1,
      expectEvent: null,
    });
    assert.equal(namedEvents(run.stderr, "capture.chat_message_disabled").length, 0);
    assert.equal(captures(run).length, 1, "an engine predating the key keeps capturing");
    assert.equal(run.messagesCalls.length, 1);
  }
);

test(
  "test_absent_chat_message_enabled_key_keeps_capture",
  { timeout: 30000 },
  async (t) => {
    const run = await runCapture(t, {
      ...caseA(),
      helloConfig: {},
      expectCaptures: 1,
      expectEvent: null,
    });
    assert.equal(namedEvents(run.stderr, "capture.chat_message_disabled").length, 0);
    assert.equal(captures(run).length, 1, "a present `config` without the key keeps capturing");
    assert.equal(run.messagesCalls.length, 1);
  }
);

test(
  "test_blank_chat_message_enabled_value_keeps_capture",
  { timeout: 30000 },
  async (t) => {
    const run = await runCapture(t, {
      ...caseA(),
      helloConfig: { chat_message_enabled: "" },
      expectCaptures: 1,
      expectEvent: null,
    });
    assert.equal(namedEvents(run.stderr, "capture.chat_message_disabled").length, 0);
    assert.equal(captures(run).length, 1, "only the exact boolean false refuses");
    assert.equal(run.messagesCalls.length, 1);
  }
);

// ---------------------------------------------------------------------------
// Issue 98 — C6 / AC8: the cached value dies with the child.
//
// Child #1 advertises `false` and refuses its idle capture; it then answers one
// `inject` and dies. The driver waits for it to be REAPED (D-98-14) before
// rewriting the hello-config file, so child #2 handshakes with `config: {}` — a
// build that has the new hello shape but not this key. D-98-15 keeps an
// omitting child from overwriting the cache, so only `teardownChild` clearing
// it lets child #2 capture.
//
// `expectCaptures: 1` with `expectEvent: null`: the latched diagnostic cannot
// settle a second time, and `fireIdleCapture` is `void`-ed, so an event-based
// settle would exit on child #1's line before child #2 runs (D-98-14).
// ---------------------------------------------------------------------------

test(
  "test_a_replacement_child_that_advertises_nothing_captures_again",
  { timeout: 30000 },
  async (t) => {
    const run = await runCapture(t, {
      ...caseA(),
      // Child #1 answers `hello` plus ONE non-hello request (the `inject` from
      // the chat.message hook), then exits.
      diesAfterRequests: 1,
      helloConfig: { chat_message_enabled: false },
      expectCaptures: 1,
      expectEvent: null,
      events: [
        { type: "session.idle", sessionID: "ses-A" },
        { hook: "chat.message" },
        { beforeHelloConfig: {}, type: "session.idle", sessionID: "ses-A" },
      ],
    });
    assert.equal(
      run.wire.filter((message) => message && message.method === "hello").length,
      2,
      "anti-vacuity: a replacement child really handshook"
    );
    assert.equal(
      namedEvents(run.stderr, "capture.chat_message_disabled").length,
      1,
      "child #1's skip was announced exactly once"
    );
    assert.equal(
      captures(run).length,
      1,
      "the replacement child captures despite its predecessor advertising false"
    );
    assert.equal(
      run.marker,
      "settled",
      "a deadline here is the inherited `false` skipping child #2's capture"
    );
  }
);
