// W5.2 `js` shard — the toast producer (`notifyHost`). Four cases, every one
// through a driver SUBPROCESS (blueprint Step 5): the wrapper spawns its engine
// with piped stdio and no `detached`/`unref` (`plugin/src/plugin.js:365-368`),
// and the stub exits only on SIGTERM or stdin EOF, so an in-process import
// would hold a live child on `node:test`'s own event loop and a regression
// would HANG the file instead of failing it. Each case asserts the stub pid is
// gone from `t.after`.
//
// Drivers are built from `tests/harness/bridge.mjs`; `injectMode` decides
// whether `chat.message` reaches the inject catch, and the fixture's
// `client.showToast` decides whether the host capability exists at all:
//   "counting" → a resolving counter ("never" → a counter that never settles).
// The success pin stages the SAME counting capability as cases 1 and 3, so its
// `toastCalls === 0` means "no attempt was made", not "the capability was
// absent" — with the capability deleted, `notifyHost` would take its
// `toast.not_delivered` branch and the count would read 0 for the wrong reason.

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

function chatMessage(sessionID = "sess-1", text = "hello world") {
  return {
    input: { sessionID },
    output: { message: { id: "msg-1", parts: [{ type: "text", text }] } },
  };
}

// Stages the package + stub engine + driver, runs one case, and returns the
// driver's record, the stub's wire log and the driver's stderr.
async function runToastCase(t, { injectMode, showToast, events, killBoundMs, helloConfig, diesAfterRequests }) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-toast-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode, helloConfig, diesAfterRequests });
  t.after(() => assertReaped(manifestPath));

  const donePath = path.join(tmp, "driver-done.json");
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(fixturePath, JSON.stringify({ client: { showToast }, events }));
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);

  const startedAt = Date.now();
  const result = await runDriver(
    driverPath,
    [pkgDir, fixturePath],
    {
      ...process.env,
      DRIVER_MODE: "v1",
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      DRIVER_DONE_PATH: donePath,
      STUB_LOG: requestLogPath,
    },
    killBoundMs
  );
  const spawnMs = Date.now() - startedAt;
  assert.equal(
    result.code,
    0,
    `the driver failed (signal=${String(result.signal)}, ${spawnMs} ms, timedOut=${result.timedOut}): stderr=${result.stderr}`
  );

  const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
  const wire = fs
    .readFileSync(requestLogPath, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line).got);
  return { record, wire, stderr: result.stderr, spawnMs };
}

// Path A of the falsifier: an awaited toast makes the driver's own 2 s guard
// win, the driver writes `{ settled: false, elapsedMs: 2000 }` and exits 0, and
// THIS assertion fails at ~2 s. Path B (the 5 s SIGKILL) is the backstop for a
// driver that cannot write at all. 15000 test / 5 s kill / 2 s driver guard.
test(
  "test_wrapper_toasts_once_when_inject_fails_and_does_not_wait_for_the_host",
  { timeout: 15000 },
  async (t) => {
    const { record } = await runToastCase(t, {
      injectMode: "error",
      showToast: "never",
      events: [chatMessage()],
      killBoundMs: 5000,
    });

    assert.equal(
      record.settled,
      true,
      `chat.message did not settle (${record.elapsedMs} ms) — the toast was awaited`
    );
    assert.ok(
      record.elapsedMs < 2000,
      `the hook must not wait on the host toast (took ${record.elapsedMs} ms)`
    );
    assert.equal(record.toastCalls, 1, "the failing inject made exactly one toast attempt");
    assert.deepEqual(
      record.toastBodies,
      [
        {
          title: "opencode-mem",
          message: "Memory injection failed; memory may be disabled.",
          variant: "warning",
        },
      ],
      "the payload is the fixed warning — never the engine error text, a transcript fragment or a path"
    );
  }
);

test(
  "test_wrapper_skips_the_toast_when_the_host_has_no_toast_api",
  { timeout: 60000 },
  async (t) => {
    const { record, stderr } = await runToastCase(t, {
      injectMode: "error",
      showToast: "absent",
      events: [chatMessage()],
      killBoundMs: 60000,
    });

    assert.equal(record.settled, true);
    assert.equal(record.toastCalls, 0, "no capability means no call");
    assert.ok(
      stderr.includes('"event":"toast.not_delivered"'),
      `a missing toast API is logged once, never thrown (stderr=${stderr})`
    );
    assert.ok(!stderr.includes('"event":"toast.failed"'), "an absent capability is not a failed call");
  }
);

test("test_wrapper_dedups_repeated_failures_into_one_toast", { timeout: 60000 }, async (t) => {
  const { record, wire } = await runToastCase(t, {
    injectMode: "error",
    showToast: "counting",
    events: [chatMessage("sess-1", "first"), chatMessage("sess-1", "second")],
    killBoundMs: 60000,
  });

  assert.equal(record.settled, true);
  assert.equal(
    wire.filter((message) => message.method === "inject").length,
    2,
    "both chat.message hooks failed their inject"
  );
  assert.equal(record.toastCalls, 1, "repeated failures of one reason notify the host once per process");
});

// The regression pin: after GREEN the success path never enters the inject
// catch, so no toast is attempted. It passes before this wave by design — and
// it is the case a later "toast unconditionally" change must trip. Three
// positive facts: the counting capability IS present, the wire carries exactly
// one inject, and no toast line reaches stderr.
test("test_wrapper_does_not_toast_when_inject_succeeds", { timeout: 60000 }, async (t) => {
  const { record, wire, stderr } = await runToastCase(t, {
    injectMode: "success",
    showToast: "counting",
    events: [chatMessage()],
    killBoundMs: 60000,
  });

  assert.equal(record.settled, true);
  assert.equal(record.toastCalls, 0, "a successful inject attempts no toast");
  assert.equal(
    wire.filter((message) => message.method === "inject").length,
    1,
    "exactly one inject reached the engine"
  );
  assert.ok(!stderr.includes('"event":"toast'), `no toast is logged on the success path (stderr=${stderr})`);
});

test(
  "test_wrapper_toasts_when_engine_advertises_inert_keys_named_summary",
  { timeout: 60000 },
  async (t) => {
    const { record } = await runToastCase(t, {
      injectMode: "success",
      showToast: "counting",
      events: [chatMessage()],
      killBoundMs: 60000,
      helloConfig: {
        inert_keys: ["similarityThreshold", "maxProfileItems", "autoUpdate"],
      },
    });

    assert.equal(record.settled, true);
    assert.equal(record.toastCalls, 1, "exactly one toast for inert keys");
    assert.deepEqual(record.toastBodies, [
      {
        title: "opencode-mem",
        message:
          "Inert config keys set: similarityThreshold, maxProfileItems, autoUpdate. These have no effect.",
        variant: "warning",
      },
    ]);
  }
);

test(
  "test_wrapper_toasts_when_engine_advertises_many_inert_keys_count_summary",
  { timeout: 60000 },
  async (t) => {
    const { record } = await runToastCase(t, {
      injectMode: "success",
      showToast: "counting",
      events: [chatMessage()],
      killBoundMs: 60000,
      helloConfig: {
        inert_keys: [
          "similarityThreshold",
          "maxProfileItems",
          "autoCaptureMaxIterations",
          "autoUpdate",
        ],
      },
    });

    assert.equal(record.settled, true);
    assert.equal(record.toastCalls, 1, "exactly one toast for inert keys");
    assert.deepEqual(record.toastBodies, [
      {
        title: "opencode-mem",
        message: "4 inert config keys set. These have no effect.",
        variant: "warning",
      },
    ]);
  }
);

test(
  "test_wrapper_skips_toast_when_inert_keys_empty",
  { timeout: 60000 },
  async (t) => {
    const { record } = await runToastCase(t, {
      injectMode: "success",
      showToast: "counting",
      events: [chatMessage()],
      killBoundMs: 60000,
      helloConfig: { inert_keys: [] },
    });

    assert.equal(record.settled, true);
    assert.equal(record.toastCalls, 0, "no toast when inert_keys is empty");
  }
);

test(
  "test_wrapper_dedupes_inert_keys_toast_across_multiple_handshakes",
  { timeout: 60000 },
  async (t) => {
    const { record, wire } = await runToastCase(t, {
      injectMode: "success",
      showToast: "counting",
      events: [
        chatMessage("sess-1", "first"),
        { ...chatMessage("sess-1", "second"), waitDeadChild: true },
      ],
      killBoundMs: 60000,
      helloConfig: { inert_keys: ["similarityThreshold"] },
      diesAfterRequests: 1,
    });

    assert.equal(record.settled, true);
    const hellos = wire.filter((message) => message.method === "hello");
    assert.equal(hellos.length, 2, "two handshakes occurred across child restart");
    assert.equal(
      record.toastCalls,
      1,
      "inert keys toast is deduplicated across handshakes"
    );
  }
);
