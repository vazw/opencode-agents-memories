// W5.5 — the V2 inheritance pin. The V2 adapter registers `prompt`, `context`
// and `compaction`, forwards every location-matching host event to the V1
// `event` hook, and `LEGACY_EVENT_ALIASES` maps `session.compaction.ended` to
// `session.compacted`. So the restore half is inherited and pinned here; the
// pre-compaction ingest is `v2-compaction-hook.test.mjs`'s subject (issue 67,
// ADR-0017 DECISION 5).
//
// The fake ctx is COMPLETE on purpose: `eventBelongsToLocation` resolves a
// location-less event through `ctx.session.get`, so a partial ctx silently
// drops the event and the case would pass for the wrong reason.
//
// Assertions:
//   (a) the event crossed the locator and reached the handler —
//       sessionGetCalls deep-equals ["sess-A","sess-A"] (the locator's
//       `ctx.session.get`, then `readSession`'s) AND the stub's wire carries one
//       restore inject; only this pair is an oracle. Issue 114 restored
//       `session.get`, so the second read is expected: the restore resolves its
//       session row. A one-element array now means the handler never read the
//       row.
//   (b) one synthetic restore write (description "memory context") and one
//       toast — issue 112 made the V2 restore an actual send.
//   (c) helloCount >= 1 and consoleErrors === [] — liveness/no-throw only.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  DRIVER_SOURCE,
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

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

test(
  "test_v2_session_compaction_ended_reaches_the_v1_compacted_handler",
  { timeout: 30000 },
  async (t) => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-v2-compaction-"));
    const pkgDir = stagePackageTree(tmp);
    const requestLogPath = path.join(tmp, "stub-requests.ndjson");
    const manifestPath = path.join(tmp, "stub-manifest.json");
    stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode: "compaction" });
    t.after(() => assertReaped(manifestPath));

    const directory = path.join(tmp, "project");
    fs.mkdirSync(directory, { recursive: true });
    const fixturePath = path.join(tmp, "fixture.json");
    fs.writeFileSync(
      fixturePath,
      JSON.stringify({
        location: { directory },
        session: {
          // The session ROW carries the agent (issue 114): the send no longer
          // depends on the transcript's shape, which after a compaction is
          // `[compaction, idle]` and holds no agent at all.
          context: [{ id: "msg-1", type: "assistant", agent: "builder", content: [] }],
          get: { "sess-A": { agent: "builder", location: { directory } } },
        },
        // No location on the envelope: the locator takes its session branch
        // and asks the host (ctx.session.get).
        events: [{ type: "session.compaction.ended", data: { sessionID: "sess-A" } }],
        rpc: { registered: true },
        pollForCapture: false,
      })
    );
    const donePath = path.join(tmp, "driver-done.json");
    const driverPath = path.join(tmp, "driver.mjs");
    fs.writeFileSync(driverPath, DRIVER_SOURCE);

    const result = await runDriver(
      driverPath,
      [pkgDir, fixturePath],
      {
        ...process.env,
        DRIVER_MODE: "v2",
        DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
        DRIVER_DONE_PATH: donePath,
        STUB_LOG: requestLogPath,
      },
      30000
    );
    assert.equal(
      result.code,
      0,
      `the v2 driver failed (signal=${String(result.signal)}): stderr=${result.stderr}`
    );

    const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
    const wire = readWire(requestLogPath);

    // (a) the oracle: the locator asked the host AND the handler read the row.
    assert.deepEqual(
      record.sessionGetCalls,
      ["sess-A", "sess-A"],
      "the locator's read, then readSession's (issue 114 restored session.get)"
    );
    const restore = wire.filter(
      (message) =>
        message &&
        message.method === "inject" &&
        message.params?.include?.compaction === "restore"
    );
    assert.equal(
      restore.length,
      1,
      `the V1 compacted handler must emit one restore inject (wire=${JSON.stringify(wire)})`
    );
    assert.equal(restore[0].params.session_id, "sess-A");

    // (b) the restore sends: one synthetic write, one toast.
    assert.equal(record.toastEmits.length, 1, "one restore, one toast");
    assert.equal(record.syntheticCalls.length, 1, "one synthetic restore write");
    assert.equal(record.syntheticCalls[0].description, "memory context");

    // (c) liveness and no-throw.
    assert.ok(record.helloCount >= 1, "the engine was reached");
    assert.deepEqual(record.consoleErrors, [], "nothing escaped the adapter's watcher");
  }
);
