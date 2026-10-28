// Issue 68 — the compaction capability gate at the wrapper level. The engine's
// `hello` names the behaviours it serves; an engine built before issue 33 has
// no `capabilities.compaction`, ignores `include.compaction` and answers the
// compaction request with the ordinary project chat block. Both compaction
// hooks therefore no-op unless the handshake advertised `compaction === true`:
// appending the chat block a second time is a silent double count.
//
// Every case runs the staged wrapper in a driver SUBPROCESS
// (`tests/harness/bridge.mjs`, `mode: "v1-compaction"`), because the wrapper
// spawns the stub engine and an in-process import would hold that child on
// `node:test`'s own event loop.
//
// The fixture is `baseFixture()`: `sess-A` has a non-summary assistant message
// carrying `agent: "builder"`, so a NON-gated restore would reach
// `session.prompt` — the restore assertions are not vacuous.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  DEFAULT_STUB_CAPABILITIES,
  DRIVER_SOURCE,
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

// The exact pre-issue-33 engine shape: the object is present, the key is not.
const PRE_68_CAPABILITIES = { ...DEFAULT_STUB_CAPABILITIES };
delete PRE_68_CAPABILITIES.compaction;

function baseFixture() {
  return {
    directory: "/work/proj",
    sessions: {
      "sess-A": { model: { providerID: "acme", id: "model-a", variant: "high" } },
    },
    // Summary LAST: a handler that resolves the agent would still find
    // `"builder"` on the non-summary message, so the gate — not an unresolvable
    // agent — is what keeps `session.prompt` at zero.
    sessionMessages: {
      "sess-A": [
        { info: { role: "assistant", agent: "builder" } },
        { info: { role: "assistant", summary: true, mode: "compaction" } },
      ],
    },
    steps: ["compacting", "compacted"],
  };
}

function readWire(requestLogPath) {
  let text = "";
  try {
    text = fs.readFileSync(requestLogPath, "utf8");
  } catch {
    return [];
  }
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
}

function injects(wire, mode) {
  return wire.filter(
    (message) =>
      message &&
      message.method === "inject" &&
      (mode === undefined
        ? true
        : message.params?.include?.compaction === mode)
  );
}

// Every structured stderr line, in order. A non-JSON line is kept as an
// `unparsed` entry so a broken channel cannot vanish from the count.
function structuredLines(stderr) {
  const events = [];
  for (const line of stderr.split("\n")) {
    if (line.trim().length === 0) continue;
    try {
      events.push(JSON.parse(line));
    } catch {
      events.push({ unparsed: line });
    }
  }
  return events;
}

function capabilityLines(stderr) {
  return structuredLines(stderr).filter(
    (event) => event !== null && typeof event === "object" && event.event === "engine.capability_missing"
  );
}

function failureLines(stderr) {
  return structuredLines(stderr).filter(
    (event) =>
      event !== null &&
      typeof event === "object" &&
      (event.event === "compaction.ingest_failed" || event.event === "compaction.restore_failed")
  );
}

async function runCase(t, options) {
  const {
    injectMode = "compaction",
    fixture = baseFixture(),
    stageEngine = true,
    killBoundMs = 30000,
    capabilities,
  } = options;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-capa-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  if (stageEngine) {
    stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode, capabilities });
    t.after(() => assertReaped(manifestPath));
  }

  const donePath = path.join(tmp, "driver-done.json");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify(fixture));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);

  const result = await runDriver(
    driverPath,
    [pkgDir, fixturePath],
    {
      ...process.env,
      DRIVER_MODE: "v1-compaction",
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      DRIVER_DONE_PATH: donePath,
      STUB_LOG: requestLogPath,
    },
    killBoundMs
  );
  assert.equal(
    result.code,
    0,
    `the v1-compaction driver failed (signal=${String(result.signal)}): stderr=${result.stderr}`
  );
  const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
  return { record, wire: readWire(requestLogPath), stderr: result.stderr };
}

// T1's 1–8: both hooks ran and settled, nothing reached the wire, nothing
// reached the host, and exactly one privacy-fixed line names the gap.
function assertGatedNoOp(record, wire, stderr) {
  assert.equal(injects(wire, "ingest").length, 0, "no compaction ingest on the wire");
  assert.equal(injects(wire, "restore").length, 0, "no compaction restore on the wire");
  assert.equal(
    wire.filter((message) => message && message.method === "inject").length,
    0,
    "no inject of any kind"
  );
  assert.deepEqual(record.contextAfter["sess-A"], [], "output.context is untouched");
  assert.equal(record.promptCalls.length, 0, "no restore prompt");
  assert.equal(record.toastBodies.length, 0, "no toast on the gated path");
  assert.equal(record.calls.length, 2, "both hooks ran");
  assert.ok(
    record.calls.every((call) => call.settled),
    "neither hook hung (a hang is not a no-op)"
  );
  const gaps = capabilityLines(stderr);
  assert.equal(gaps.length, 1, "exactly one engine.capability_missing line");
  const [gap] = gaps;
  assert.deepEqual(
    Object.keys(gap).sort(),
    ["capability", "component", "engine_protocol", "event", "level", "min_protocol"],
    "the line carries only fixed fields (no session id, transcript, path or error text)"
  );
  assert.equal(gap.capability, "compaction");
  assert.equal(gap.engine_protocol, "1.0.0");
  assert.equal(gap.min_protocol, "1.0.0");
}

test(
  "test_compaction_hooks_no_op_when_the_engine_does_not_advertise_the_capability",
  { timeout: 30000 },
  async (t) => {
    const { record, wire, stderr } = await runCase(t, {
      capabilities: PRE_68_CAPABILITIES,
    });
    assertGatedNoOp(record, wire, stderr);
  }
);

// T2 — the older-engine shape: `capabilities: null` makes `JSON.stringify`
// drop the key, so the hello result has no `capabilities` object at all. A
// keyed read without a null guard throws inside each hook's own `catch`; the
// no-op still happens, but the gap line is lost and two generic failure lines
// take its place — the silent defect, re-created.
test(
  "test_compaction_hooks_no_op_and_log_without_throwing_when_the_hello_result_has_no_capabilities",
  { timeout: 30000 },
  async (t) => {
    const { record, wire, stderr } = await runCase(t, {
      capabilities: null,
    });
    assertGatedNoOp(record, wire, stderr);
    assert.deepEqual(
      failureLines(stderr),
      [],
      "no compaction.ingest_failed / compaction.restore_failed line (a throw is not a no-op)"
    );
  }
);

// T3 — the strict boolean read: a truthy `"true"` string must not open the
// gate. The object carries four keys (the pre-68 three plus `compaction`), so a
// truthiness gate AND a key-count gate both open it; only `=== true` stays shut.
test(
  "test_compaction_hooks_no_op_when_the_capability_is_not_the_boolean_true",
  { timeout: 30000 },
  async (t) => {
    const { record, wire, stderr } = await runCase(t, {
      capabilities: { ...DEFAULT_STUB_CAPABILITIES, compaction: "true" },
    });
    assertGatedNoOp(record, wire, stderr);
    assert.deepEqual(
      failureLines(stderr),
      [],
      "no compaction.ingest_failed / compaction.restore_failed line"
    );
  }
);
