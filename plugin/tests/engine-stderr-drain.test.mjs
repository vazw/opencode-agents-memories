// Engine stderr drain (issue 117).
//
// The wrapper spawns the engine with three pipes and, before this wave, read
// only the first. Every `structured_stderr` line the engine emits on fd 2 went
// into a pipe with no reader: invisible, and a full socket pair blocks the
// engine in `write(2)`. These cases pin the renderer's byte shape (rows 1-7,
// in-process against the PRODUCTION serializer — the `fixtures.test.mjs`
// precedent: no test twin to drift) and the drain end to end (rows 8-13,
// through a driver this file writes into its own temp package).
//
// The driver and the engine stub are LOCAL to this file: the shared harness
// driver fires a fixed part list, its stub never touches fd 2, and
// `bridge.mjs` is shared by several suites — `capture-payload.test.mjs:6-9`
// states the convention, so no shared file is edited for a fixture need. Only
// `stagePackageTree`, `runDriver` and `assertReaped` — generic utilities — are
// imported from it.
//
// The zone is pinned in-file and the pin is asserted: `node --test` gives each
// file its own process, so this assignment cannot leak into another suite, and
// row 2 fails loudly on a runtime that ignored it instead of comparing against
// the wrong zone.

process.env.TZ = "Asia/Bangkok";

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { engineLogPath, engineStderrLogLine } from "../src/plugin.js";
import { assertReaped, runDriver, stagePackageTree } from "./harness/bridge.mjs";

// The one instant every stamp assertion uses: 16:38 in Asia/Bangkok (+07:00).
const FIXED_INSTANT = new Date("2026-11-03T09:38:00.000Z");
const FIXED_STAMP = "[2026-11-03T16:38:00.000+07:00]";

// The live engine's own line, deliberately SHUFFLED: the engine emits sorted
// keys (`serde_json` map order), so a fixture in that order would make a
// missing canonicalize invisible. This one exercises rows 1 and 3 at once.
const JUMBLED_CONSOLIDATION_LINE =
  '{"level":"info","projects_scanned":3,"event":"consolidation.startup_tick","component":"consolidation","runs":0,"busy":0,"failed":0}';

/** Splits a rendered line into its three contract fields. */
function renderedParts(line) {
  assert.equal(
    typeof line,
    "string",
    "the renderer must return a string"
  );
  assert.ok(line.endsWith("\n"), `a rendered line must end in a newline: ${JSON.stringify(line)}`);
  assert.ok(!line.endsWith("\n\n"), "a rendered line ends in exactly one newline");
  const match = /^\[([^\]]{29})\] ([^:]*): (\{.*\})\n$/s.exec(line);
  assert.ok(
    match !== null,
    `a rendered line must be [<29-byte stamp>] <message>: {<json>}\\n, got ${JSON.stringify(line)}`
  );
  return { stamp: match[1], message: match[2], payload: match[3] };
}

// Row 1.
test("test_engine_stderr_line_is_lossless_in_the_logs_own_shape", () => {
  const rendered = engineStderrLogLine(JUMBLED_CONSOLIDATION_LINE, FIXED_INSTANT);
  const { stamp, message, payload } = renderedParts(rendered);

  assert.equal(stamp, FIXED_STAMP.slice(1, 30), "the receive-time stamp is the engine log's own");
  assert.equal(message, "consolidation.startup_tick", "an identifier-shaped event moves to the message slot");

  // Losslessness, stated as the reconstruction identity: nothing but the
  // `event` key may move. A payload "tidied" by dropping component/level
  // fails here.
  const input = JSON.parse(JUMBLED_CONSOLIDATION_LINE);
  assert.deepEqual({ event: message, ...JSON.parse(payload) }, input);
});

// Row 2.
test("test_engine_stderr_stamp_is_29_bytes_in_a_pinned_zone", () => {
  // Guard first: a runtime that ignored the TZ assignment must fail here, not
  // pass while comparing against a different zone's literal.
  assert.equal(
    new Date("2026-11-03T09:38:00.000Z").getTimezoneOffset(),
    -420,
    "process.env.TZ = Asia/Bangkok must take effect (expected -420 minutes)"
  );

  const rendered = engineStderrLogLine('{"event":"probe.tick","level":"info"}', FIXED_INSTANT);
  assert.ok(
    rendered.startsWith(FIXED_STAMP + " "),
    `the stamp must be the pinned local time with a zero-padded offset, got ${JSON.stringify(rendered.slice(0, 34))}`
  );
  const stamp = rendered.slice(1, 30);
  assert.equal(Buffer.byteLength(stamp, "utf8"), 29, "the stamp is exactly 29 bytes");
  assert.match(stamp, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}[+-]\d{2}:\d{2}$/);
});

// Row 3.
test("test_engine_stderr_payload_uses_canonical_sorted_keys", () => {
  const rendered = engineStderrLogLine('{"level":"info","event":"x.y","component":"z"}', FIXED_INSTANT);
  const { message, payload } = renderedParts(rendered);
  assert.equal(message, "x.y");
  assert.equal(payload, '{"component":"z","level":"info"}');
});

// Row 4.
test("test_engine_stderr_unstructured_line_is_json_wrapped_not_appended_raw", () => {
  // A panic hook's or a library's line. Appended raw it would land in the
  // message slot, where the frozen classifier's own `: error` rule would
  // invent an error bucket named "Fatal" out of a line the wrapper wrote.
  const rendered = engineStderrLogLine("Fatal: error", FIXED_INSTANT);
  const { message, payload } = renderedParts(rendered);
  assert.equal(message, "engine.stderr_unstructured");
  assert.deepEqual(JSON.parse(payload), { text: "Fatal: error" });
});

// Row 5.
test("test_engine_stderr_non_object_json_folds_into_the_unstructured_branch", () => {
  for (const literal of ["null", "[1,2]", "42", '"x"']) {
    const rendered = engineStderrLogLine(literal, FIXED_INSTANT);
    const { message, payload } = renderedParts(rendered);
    assert.equal(message, "engine.stderr_unstructured", `${literal} must fold into the unstructured branch`);
    assert.deepEqual(JSON.parse(payload), { text: literal });
    assert.equal(typeof JSON.parse(payload), "object", `${literal} must not be emitted as a bare payload`);
  }
});

// Row 6.
test("test_engine_stderr_event_name_that_is_not_an_identifier_falls_back_to_the_generic_message", () => {
  // The engine's own prose event names contain spaces and would forge a stats
  // counter if they reached the message slot.
  const input = '{"event":"Auto-capture memory persisted","level":"info","component":"x"}';
  const rendered = engineStderrLogLine(input, FIXED_INSTANT);
  const { message, payload } = renderedParts(rendered);
  assert.equal(message, "engine.stderr");
  assert.equal(payload, '{"component":"x","event":"Auto-capture memory persisted","level":"info"}');
  // The refused name stays IN the payload: the charset rule protects the
  // message slot and says nothing about the data.
  assert.deepEqual(JSON.parse(payload), JSON.parse(input));
});

// Row 7.
test("test_engine_stderr_line_over_the_cap_is_truncated_and_marked", () => {
  const filler = "A".repeat(4989);
  const payloadJson = '{"text":"' + filler + '"}';
  // Exactly 5000 bytes by construction: the `{"text":""}` envelope is 11 bytes.
  assert.equal(Buffer.byteLength(payloadJson, "utf8"), 5000, "the fixture payload must be exactly 5000 bytes");

  const rendered = engineStderrLogLine('{"event":"probe.big","text":"' + filler + '"}', FIXED_INSTANT);
  const { message, payload } = renderedParts(rendered);
  assert.equal(message, "probe.big");
  const parsed = JSON.parse(payload);
  // Separate assertions on purpose: canonical order puts `bytes` first, so no
  // single substring can carry all three fields.
  assert.equal(parsed.bytes, 5000, "bytes is the ORIGINAL payload JSON's byte length");
  assert.equal(parsed.truncated, true);
  assert.ok(
    payload.startsWith('{"bytes":5000,"text":"'),
    `the truncated payload must start in canonical order, got ${JSON.stringify(payload.slice(0, 40))}`
  );
  // `text` is a forensic byte dump of the payload JSON, so it carries the
  // payload's own 9-byte prefix — it is NOT the payload's content.
  assert.equal(parsed.text, '{"text":"' + "A".repeat(4087));
  assert.equal(Buffer.byteLength(parsed.text, "utf8"), 4096);

  // Multibyte, structured, no throw: the 4096-byte cut lands inside a codepoint.
  const euroJson = '{"text":"' + "€".repeat(2000) + '"}';
  assert.equal(Buffer.byteLength(euroJson, "utf8"), 6011, "the multibyte payload must be 6011 bytes");
  const euroPayload = renderedParts(
    engineStderrLogLine('{"event":"probe.big","text":"' + "€".repeat(2000) + '"}', FIXED_INSTANT)
  ).payload;
  const euro = JSON.parse(euroPayload);
  assert.equal(euro.bytes, 6011);
  assert.equal(Buffer.byteLength(euro.text, "utf8"), 4098, "9 prefix bytes + 1362 × 3 + one 3-byte U+FFFD");
  assert.ok(euro.text.includes("\uFFFD"), "the split codepoint renders U+FFFD rather than throwing");
});

// ── the drain, end to end (rows 8-13) ───────────────────────────────────
//
// The engine stub is local to this file because this suite needs one that
// writes to fd 2 BEFORE it installs its stdin listener and can be told, later
// and by the driver, to emit one more line. `stageStubEngine` is not called at
// all: this writes the same path `engineExecutablePath()` resolves, on top.
//
// HARD RULE: the stub never touches the `process.stderr` / `process.stdout`
// stream objects — not even `process.stderr.write("")`. MEASURED on this host:
// the same 1 139 000-byte fd-2 write blocks forever with no reader, but returns
// 146 176 bytes and throws EAGAIN after a single `process.stderr.write("")`,
// i.e. the child DIES instead of hanging. One stray touch silently converts row
// 9's RED into a fixture failure, and in GREEN can kill the stub before it
// installs its stdin listener. `fs.writeSync(1|2, …)` only.
function localStubSource({ preambleLine, repeat, manifestPath, triggerPath, extraLine }) {
  const config = JSON.stringify({ preambleLine, repeat, manifestPath, triggerPath, extraLine });
  return `#!/usr/bin/env node
// File-local engine stub for plugin/tests/engine-stderr-drain.test.mjs.
// Writes to fd 1 and fd 2 with fs.writeSync ONLY: touching the process.stderr
// or process.stdout stream objects would flip fd 2 non-blocking and turn the
// no-reader blocking fixture into an EAGAIN crash (measured).
import fs from "node:fs";

const CONFIG = ${config};

fs.writeFileSync(CONFIG.manifestPath, JSON.stringify({ pid: process.pid }));

function writeToFd(fd, text) {
  const buf = Buffer.from(text, "utf8");
  let offset = 0;
  while (offset < buf.length) {
    offset += fs.writeSync(fd, buf, offset, buf.length - offset);
  }
}

// Before the stdin listener, on purpose: with no reader this fills the socket
// and blocks here, which is exactly what row 9 measures. Do not reorder.
writeToFd(2, CONFIG.preambleLine.repeat(CONFIG.repeat));

let emitted = false;

function reply(id, result) {
  writeToFd(1, JSON.stringify({ id, result }) + "\\n");
}

function handle(line) {
  if (line.trim().length === 0) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return;
  }
  const method = message && typeof message.method === "string" ? message.method : null;
  if (method === "hello") {
    reply(message.id, {
      protocol: "1.0.0",
      capabilities: { compaction: true, fire_and_forget_capture: true, skills: true, standalone: true, web: true },
      config: {},
    });
    return;
  }
  if (method === "shutdown") {
    reply(message.id, { ack: true });
    process.exit(0);
  }
  // The trigger is a FILE the driver creates, checked on a request the driver
  // sends: no timer in the stub, so the emission instant is driver-controlled
  // and the recovery fixture cannot race the wrapper's retry window.
  if (!emitted && CONFIG.triggerPath !== null && fs.existsSync(CONFIG.triggerPath)) {
    emitted = true;
    writeToFd(2, CONFIG.extraLine);
  }
  reply(message.id, { ok: true });
}

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let index = buffer.indexOf("\\n");
  while (index >= 0) {
    const line = buffer.slice(0, index);
    buffer = buffer.slice(index + 1);
    handle(line);
    index = buffer.indexOf("\\n");
  }
});
process.stdin.on("end", () => process.exit(0));
process.on("SIGTERM", () => process.exit(0));
`;
}

function stageLocalStub(pkgDir, config) {
  const dir = path.join(pkgDir, "engine", `${process.platform}-${process.arch}`);
  fs.mkdirSync(dir, { recursive: true });
  const binPath = path.join(dir, "opencode-agents-memory");
  fs.writeFileSync(binPath, localStubSource(config), { mode: 0o755 });
  fs.chmodSync(binPath, 0o755);
  return binPath;
}

// The driver is local for the same reason: it polls the engine's log file and,
// in the recovery case, repairs a broken destination mid-run — neither is
// something the shared harness driver does.
const HOOK_GUARD_MS = 2000;

const DRIVER_SOURCE = `import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const pkgDir = process.argv[2];
const fixture = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
process.env.OPENCODE_MEM_STORAGE_ROOT = process.env.DRIVER_STORAGE_ROOT;

const HOOK_GUARD_MS = ${HOOK_GUARD_MS};
const LOG_DIR = path.join(process.env.HOME, ".opencode-mem");
const LOG_PATH = path.join(LOG_DIR, "opencode-mem.log");
const LINE_RE = /^\\[[^\\]]{29}\\] ([^:]*): (\\{.*\\})$/;

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const client = {
  session: { messages: async () => ({ data: fixture.sessionMessages ?? [] }) },
  tui: { showToast: async () => ({ data: true }) },
};

const mod = await import(pathToFileURL(path.join(pkgDir, "dist", "plugin.js")));
const { hooks } = await mod.default.server({ client, directory: fixture.directory ?? pkgDir });

const startedAt = process.hrtime.bigint();

let fullText = null;
let filePresent = false;
let logBytes = null;
let messageCounts = new Map();
let dropCounts = [];

function readLog() {
  try {
    return fs.readFileSync(LOG_PATH, "utf8");
  } catch {
    return null;
  }
}

// The counts are computed from the FULL text, never from the truncated copy
// handed to the record: a row that counted the copy would fail conservation
// for the wrong reason.
function scan(text) {
  messageCounts = new Map();
  dropCounts = [];
  if (text === null) return;
  for (const line of text.split("\\n")) {
    if (line === "") continue;
    const match = LINE_RE.exec(line);
    if (match === null) continue;
    messageCounts.set(match[1], (messageCounts.get(match[1]) ?? 0) + 1);
    if (match[1] !== "engine.stderr_dropped") continue;
    try {
      const payload = JSON.parse(match[2]);
      if (payload !== null && typeof payload.count === "number") dropCounts.push(payload.count);
    } catch {
      // torn read at a write boundary; the next poll re-reads
    }
  }
}

function refresh() {
  fullText = readLog();
  filePresent = fullText !== null;
  logBytes = fullText === null ? null : Buffer.byteLength(fullText, "utf8");
  scan(fullText);
}

async function pollFor(expectText, quietMs) {
  const deadline = Date.now() + (fixture.logWaitMs ?? 6000);
  let lastBytes = null;
  let quietSince = null;
  for (;;) {
    refresh();
    if (logBytes !== lastBytes) {
      lastBytes = logBytes;
      quietSince = Date.now();
    }
    if (
      fullText !== null &&
      fullText.includes(expectText) &&
      quietSince !== null &&
      Date.now() - quietSince >= quietMs
    ) {
      return;
    }
    if (Date.now() >= deadline) return;
    await delay(50);
  }
}

// elapsedMs is the HOOK-RACE duration, taken at the guard, and nothing else: it
// does not include the poll below. totalMs (spawn -> record) is recorded for
// the ledger and never asserted.
const t0 = process.hrtime.bigint();
const raced = await Promise.race([
  hooks["chat.message"](
    { sessionID: "ses-A" },
    { message: { id: "msg-1", parts: [{ type: "text", text: "ping" }] } }
  ).then(() => "hook"),
  delay(HOOK_GUARD_MS).then(() => "guard"),
]);
const elapsedMs = Number(process.hrtime.bigint() - t0) / 1e6;
const settled = raced === "hook";

let beforeRepairFilePresent = false;
let secondHook = null;
let recoveryLinePresent = false;
const pollMode = fixture.pollMode ?? "expectText";

if (pollMode === "none") {
  // The append / mkdir callback needs at least one libuv round-trip.
  await delay(fixture.settleMs ?? 1000);
  refresh();
} else {
  // No exit condition may depend on undefined.
  if (typeof fixture.expectText !== "string") throw new Error("fixture must pin expectText");
  if (pollMode === "recovery") {
    await delay(fixture.settleMs ?? 1000);
    refresh();
    beforeRepairFilePresent = filePresent;
    // repair the destination, wait past the wrapper's retry window, then let
    // the stub emit one more line on a request the driver sends.
    fs.rmSync(LOG_DIR, { force: true });
    fs.mkdirSync(LOG_DIR, { recursive: true });
    await delay((fixture.retryMs ?? 1000) + 400);
    fs.writeFileSync(fixture.triggerPath, "");
    try {
      await hooks["chat.message"](
        { sessionID: "ses-A" },
        { message: { id: "msg-2", parts: [{ type: "text", text: "ping" }] } }
      );
    } catch (err) {
      // recorded, never asserted: the stub emits on receipt, so even a
      // rejected call delivers the line this row needs.
      secondHook = String(err && err.message);
    }
    await pollFor(fixture.expectText, 500);
    recoveryLinePresent = fullText !== null && fullText.includes(fixture.expectText);
  } else {
    await pollFor(fixture.expectText, fixture.quietMs ?? 250);
  }
}

const totalMs = Number(process.hrtime.bigint() - startedAt) / 1e6;
const logText = fullText === null ? null : fullText.slice(0, fixture.logTextLimit ?? 65536);
fs.writeFileSync(
  process.env.DRIVER_DONE_PATH,
  JSON.stringify({
    settled,
    elapsedMs,
    totalMs,
    filePresent,
    logText,
    logBytes,
    messageCounts: Object.fromEntries(messageCounts),
    dropCounts,
    beforeRepairFilePresent,
    secondHook,
    recoveryLinePresent,
  })
);
// Flush the piped stderr, THEN exit: a bare exit truncates a pending pipe
// write, and row 12 reads the wrapper's own warning from this stream.
await new Promise((resolve) => process.stderr.write("", resolve));
process.exit(0);
`;

const CONSOLIDATION_LINE =
  '{"component":"consolidation","event":"consolidation.startup_tick","level":"info","busy":0,"failed":0,"projects_scanned":3,"runs":0}\n';
const CONSOLIDATION_EXPECT =
  '] consolidation.startup_tick: {"busy":0,"component":"consolidation","failed":0,"level":"info","projects_scanned":3,"runs":0}';
const PROBE_LINE = '{"component":"probe","event":"engine.stderr_probe","level":"info"}\n';
const PROBE_EXPECT = 'engine.stderr_probe: {"component":"probe","level":"info"}';

async function runDrain(t, fixture, { prepareHome, extraEnv } = {}) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-engine-stderr-"));
  const home = path.join(tmp, "home");
  fs.mkdirSync(home, { recursive: true });
  if (prepareHome !== undefined) prepareHome(home);

  const pkgDir = stagePackageTree(tmp);
  const manifestPath = path.join(tmp, "stub-manifest.json");
  const triggerPath = fixture.useTrigger === true ? path.join(tmp, "stderr-trigger") : null;
  stageLocalStub(pkgDir, {
    preambleLine: fixture.preambleLine,
    repeat: fixture.repeat ?? 1,
    manifestPath,
    triggerPath,
    extraLine: fixture.extraLine ?? "",
  });
  t.after(() => assertReaped(manifestPath));

  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const donePath = path.join(tmp, "driver-done.json");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify({ ...fixture, triggerPath }));

  // HOME is always passed, so a case can never write the developer's real
  // ~/.opencode-mem log.
  const extra = typeof extraEnv === "function" ? extraEnv(tmp) : (extraEnv ?? {});
  const run = await runDriver(driverPath, [pkgDir, fixturePath], {
    ...process.env,
    ...extra,
    HOME: home,
    DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
    DRIVER_DONE_PATH: donePath,
  });
  const record = fs.existsSync(donePath) ? JSON.parse(fs.readFileSync(donePath, "utf8")) : null;
  return { ...run, record, home, tmp, engineLog: engineLogPath(home) };
}

// Row 8.
test("test_engine_stderr_line_reaches_the_engine_log_file", { timeout: 30000 }, async (t) => {
  const run = await runDrain(t, {
    preambleLine: CONSOLIDATION_LINE,
    repeat: 1,
    expectText: CONSOLIDATION_EXPECT,
  });
  assert.equal(run.code, 0, `the drain driver failed (signal=${run.signal}): ${run.stderr}`);
  assert.ok(run.record !== null, "the driver must write its record");
  assert.equal(run.record.settled, true, "the handshake settles: the drain never blocks the engine");
  assert.ok(run.record.logText !== null, "the engine's log file must exist");
  assert.ok(
    run.record.logText.includes(CONSOLIDATION_EXPECT),
    `the drained line is missing from the engine log: ${JSON.stringify(run.record.logText)}`
  );
});

// The flood: 17 000 × 67-byte lines = 1 139 000 bytes to fd 2 before the stub
// installs its stdin listener — far past the socket buffer measured on this
// host (212 992 bytes). With no reader the child blocks in `write(2)` and never
// answers `hello`; with the drain it settles.
const FLOOD_REPEAT = 17000;

// Row 9.
test("test_engine_writing_past_the_socket_buffer_is_drained_not_blocked", { timeout: 60000 }, async (t) => {
  const run = await runDrain(t, {
    preambleLine: PROBE_LINE,
    repeat: FLOOD_REPEAT,
    expectText: PROBE_EXPECT,
    logTextLimit: 4096,
  });
  assert.equal(run.code, 0, `the flood driver failed (signal=${run.signal}): ${run.stderr}`);
  assert.ok(run.record !== null, "the driver must write its record");
  assert.equal(
    run.record.settled,
    true,
    `the handshake must settle — the engine blocked for ${Math.round(run.record.elapsedMs)} ms`
  );
  assert.ok(
    (run.record.messageCounts["engine.stderr_probe"] ?? 0) >= 1,
    "at least one flooded line reached the log"
  );
});

// Row 10.
test("test_engine_stderr_drops_are_counted_not_spammed", { timeout: 60000 }, async (t) => {
  const run = await runDrain(t, {
    preambleLine: PROBE_LINE,
    repeat: FLOOD_REPEAT,
    expectText: PROBE_EXPECT,
  });
  assert.equal(run.code, 0, `the flood driver failed (signal=${run.signal}): ${run.stderr}`);
  assert.ok(run.record !== null, "the driver must write its record");

  // Conservation, counted from the FULL file text (the record's logText is a
  // 64 KiB copy): every flooded line is either in the file or counted in a
  // drop summary.
  const written = run.record.messageCounts["engine.stderr_probe"] ?? 0;
  const dropped = run.record.dropCounts.reduce((sum, count) => sum + count, 0);
  assert.equal(written + dropped, FLOOD_REPEAT, `conservation: ${written} written + ${dropped} counted`);
  assert.ok(run.record.dropCounts.length >= 1, "a flood past the queue bound must be counted");
  for (const count of run.record.dropCounts) {
    assert.ok(count > 0, `every summary carries a positive count, got ${count}`);
  }
  // Not "exactly one": a momentary empty queue between two data events can
  // produce a second, larger summary. What must hold is that the summary is not
  // emitted per dropped line.
  assert.ok(
    run.record.dropCounts.length <= 1000,
    `the summary must not be per-line spam, got ${run.record.dropCounts.length} summaries`
  );
});

// Row 11.
test("test_drain_creates_the_log_directory_it_does_not_inherit", { timeout: 30000 }, async (t) => {
  // A fresh HOME with no `.opencode-mem` at all. The engine's own `LogSink::init`
  // normally primes that directory, which is exactly why the absent directory
  // isolates the wrapper's own mkdir branch.
  const run = await runDrain(t, {
    preambleLine: PROBE_LINE,
    repeat: 1,
    expectText: PROBE_EXPECT,
  });
  assert.equal(run.code, 0, `the drain driver failed (signal=${run.signal}): ${run.stderr}`);
  assert.ok(run.record !== null, "the driver must write its record");
  assert.ok(fs.existsSync(run.engineLog), `the drain must create ${run.engineLog} itself`);
  assert.ok(
    run.record.logText !== null && run.record.logText.includes(PROBE_EXPECT),
    `the line must land in a directory the wrapper created: ${JSON.stringify(run.record.logText)}`
  );
  assert.doesNotMatch(run.stderr, /engine\.stderr_log_unwritable/, "a creatable directory is not an error");
});

// Row 12 — three fixtures, one function: an unwritable destination must never
// take down the host, and the drain must resume once it is writable again.
test("test_unwritable_log_destination_does_not_take_down_the_host", { timeout: 60000 }, async (t) => {
  // (a) a REGULAR FILE where the directory must be: `mkdir` fails with EEXIST,
  //     so this exercises the mkdir failure site (the append's ENOTDIR is never
  //     reached).
  const regularFile = await runDrain(
    t,
    { preambleLine: PROBE_LINE, repeat: 1, pollMode: "none", settleMs: 1000 },
    {
      prepareHome: (home) => {
        fs.writeFileSync(path.join(home, ".opencode-mem"), "not a directory\n");
      },
    }
  );
  assert.equal(regularFile.code, 0, `(a) the host must survive a mkdir failure: ${regularFile.stderr}`);
  assert.ok(regularFile.record !== null, "(a) the driver must still write its record");
  assert.equal(regularFile.record.settled, true, "(a) the handshake still settles");
  assert.equal(regularFile.record.filePresent, false, "(a) the fixture must really be unwritable");
  assert.match(regularFile.stderr, /engine\.stderr_log_unwritable/, "(a) the failure is named once");

  // (b) the directory exists and the LOG PATH is a directory: `mkdir(parent)`
  //     succeeds, so this exercises the append failure site (EISDIR) and the
  //     `stderrDirReady` true branch.
  const logIsDirectory = await runDrain(
    t,
    { preambleLine: PROBE_LINE, repeat: 1, pollMode: "none", settleMs: 1000 },
    {
      prepareHome: (home) => {
        fs.mkdirSync(path.join(home, ".opencode-mem"), { recursive: true });
        fs.mkdirSync(path.join(home, ".opencode-mem", "opencode-mem.log"));
      },
    }
  );
  assert.equal(logIsDirectory.code, 0, `(b) the host must survive an append failure: ${logIsDirectory.stderr}`);
  assert.ok(logIsDirectory.record !== null, "(b) the driver must still write its record");
  assert.equal(logIsDirectory.record.settled, true, "(b) the handshake still settles");
  assert.equal(logIsDirectory.record.filePresent, false, "(b) the fixture must really be unwritable");
  assert.match(logIsDirectory.stderr, /engine\.stderr_log_unwritable/, "(b) the failure is named once");

  // (c) the LATCH / RECOVERY fixture: (a)'s shape, then the driver repairs the
  //     destination, waits past the retry window, and makes the stub emit one
  //     more line. Clearing `stderrWriting` only in the append callback (r3's
  //     arrangement) latches the flag on the mkdir path and this line never
  //     lands.
  const RECOVERY_LINE = '{"component":"probe","event":"engine.stderr_recovery","level":"info"}\n';
  const RECOVERY_EXPECT = 'engine.stderr_recovery: {"component":"probe","level":"info"}';
  const recovery = await runDrain(
    t,
    {
      preambleLine: PROBE_LINE,
      repeat: 1,
      extraLine: RECOVERY_LINE,
      useTrigger: true,
      pollMode: "recovery",
      settleMs: 1000,
      retryMs: 1000,
      expectText: RECOVERY_EXPECT,
    },
    {
      prepareHome: (home) => {
        fs.writeFileSync(path.join(home, ".opencode-mem"), "not a directory\n");
      },
    }
  );
  assert.equal(recovery.code, 0, `(c) the host must survive and recover: ${recovery.stderr}`);
  assert.ok(recovery.record !== null, "(c) the driver must still write its record");
  assert.equal(recovery.record.beforeRepairFilePresent, false, "(c) nothing was written before the repair");
  assert.equal(recovery.record.recoveryLinePresent, true, "(c) the drain must resume after the repair");
  // The lost preamble line is surfaced at least once. `>= 1`, not the exact
  // identity: a second flush inside the retry window legitimately counts the
  // pending summary as one more dropped line. Exact conservation is row 10's
  // job, on a writable destination.
  const recoveredDrops = recovery.record.dropCounts.reduce((sum, count) => sum + count, 0);
  assert.ok(recoveredDrops >= 1, `(c) the lost line must be counted, got ${JSON.stringify(recovery.record.dropCounts)}`);
  assert.ok(recovery.record.dropCounts.length <= 10, "(c) the loss is summarised, not spammed");
  // Exactly one warning: no second failure is induced, and `stderrFailureLogged`
  // is per state — a repetition after a successful recovery is silent by design.
  const warnings = (recovery.stderr.match(/engine\.stderr_log_unwritable/g) ?? []).length;
  assert.equal(warnings, 1, `(c) exactly one warning for the one failure, got ${warnings}: ${recovery.stderr}`);
});

// Row 13.
test("test_wrapper_does_not_honour_the_log_file_override_the_engine_cannot_see", { timeout: 30000 }, async (t) => {
  // The engine cannot see OPENCODE_MEM_LOG_FILE: `childEnvironment()` does not
  // forward it, so honouring it wrapper-side would split one logical log across
  // two files. The wrapper must keep writing the HOME path.
  const run = await runDrain(
    t,
    { preambleLine: PROBE_LINE, repeat: 1, expectText: PROBE_EXPECT },
    { extraEnv: (tmp) => ({ OPENCODE_MEM_LOG_FILE: path.join(tmp, "elsewhere.log") }) }
  );
  assert.equal(run.code, 0, `driver failed (signal=${run.signal}): ${run.stderr}`);
  assert.ok(run.record !== null, "the driver must write its record");
  assert.equal(
    fs.existsSync(path.join(run.tmp, "elsewhere.log")),
    false,
    "the wrapper must not honour an override the engine cannot see"
  );
  assert.ok(
    run.record.logText !== null && run.record.logText.includes(PROBE_EXPECT),
    "the line lands in the engine's own log file"
  );
  assert.ok(fs.existsSync(run.engineLog), `the default log path exists: ${run.engineLog}`);
});
