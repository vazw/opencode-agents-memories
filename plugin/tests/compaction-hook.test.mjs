// W5.5 — the compaction hook at the wrapper level: ingest before, restore
// after. Every case is a driver SUBPROCESS (`tests/harness/bridge.mjs`), because
// the wrapper spawns a stub engine and an in-process import would hold the
// child on `node:test`'s own event loop.
//
// The stub is request-shape aware (`injectMode: "compaction"`): a restore
// request is answered `RESTORE_2` / memory_count 7, an ingest request
// `INGEST_2` / memory_count 2, and a BARE inject (no `include.compaction`) the
// project chat block. That third arm is the trap: a no-subtraction
// implementation receives the chat block and appends it to `output.context`.
//
// The driver fires `chat.message` for `sess-OTHER` first, so `capturedSessionId`
// cannot stand in for the compaction hooks' own `sess-A` on the wire.

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

const RESTORE_2 =
  "## Restored Session Memory\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n";
const INGEST_2 =
  "## Session Memory Before Compaction\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n";

function baseFixture() {
  return {
    directory: "/work/proj",
    // A MAP keyed by session id: one session object cannot make `sess-A` a
    // parent and another id its child in the same run (fixture C).
    sessions: {
      "sess-A": { model: { providerID: "acme", id: "model-a", variant: "high" } },
    },
    // Summary LAST: a "take the last assistant message" implementation resolves
    // `"compaction"` instead of `"builder"`.
    sessionMessages: {
      "sess-A": [
        { info: { role: "assistant", agent: "builder" } },
        { info: { role: "assistant", summary: true, mode: "compaction" } },
      ],
    },
  };
}

function withFixture(overrides) {
  return { ...baseFixture(), ...overrides };
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

async function runCase(t, options) {
  const {
    injectMode = "compaction",
    fixture = withFixture({}),
    stageEngine = true,
    killBoundMs = 30000,
  } = options;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-compaction-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  if (stageEngine) {
    stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode });
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

// ── AC 1: ingest ─────────────────────────────────────────────────────────

test(
  "test_compacting_hook_appends_one_session_context_string_and_never_the_injected_block",
  { timeout: 30000 },
  async (t) => {
    const { record, wire } = await runCase(t, {
      fixture: withFixture({ steps: ["chat.message", "compacting"] }),
    });

    assert.deepEqual(
      record.contextAfter["sess-A"],
      [INGEST_2],
      "exactly one byte-exact ingest string, never the project chat block"
    );
    assert.equal(record.promptPresent, false, "output.prompt was never written");
    assert.equal(record.promptValue, null, "output.prompt is undefined (never \"\")");

    const bare = wire.filter((message) => message.method === "inject");
    assert.equal(bare.length, 2, "the chat.message inject and the compacting inject");
    const [chatInject, ingestInject] = bare;
    assert.deepEqual(
      chatInject.params.include,
      { query: "hi" },
      "chat.message sends the authored harness text as the query"
    );
    assert.equal(ingestInject.params.include.compaction, "ingest");
    assert.equal(
      ingestInject.params.include.query,
      undefined,
      "the compaction leg carries no query (compactionParams overwrites include)"
    );
    assert.equal(ingestInject.params.session_id, "sess-A", "the hook's own id, never sess-OTHER");
    assert.equal(
      ingestInject.params.project_id,
      chatInject.params.project_id,
      "the compaction inject carries the same project_id the chat.message inject carried"
    );
    assert.ok(ingestInject.params.project_id, "a blank project_id would make assemble a no-op");
  }
);

// ── AC 2: restore ────────────────────────────────────────────────────────

test(
  "test_compacted_event_restores_one_synthetic_no_reply_message",
  { timeout: 30000 },
  async (t) => {
    const { record, wire } = await runCase(t, {
      fixture: withFixture({ steps: ["chat.message", "compacted"] }),
    });

    assert.equal(record.promptCalls.length, 1, "exactly ONE message per compaction");
    const call = record.promptCalls[0];
    assert.equal(call.path.id, "sess-A");
    assert.equal(call.body.noReply, true);
    assert.equal(call.body.parts.length, 1, "one part, never one message per memory");
    const part = call.body.parts[0];
    assert.ok(part.id.startsWith("prt-compaction-"), "a process-unique part id");
    assert.equal(part.type, "text");
    assert.equal(part.synthetic, true);
    assert.equal(part.text, RESTORE_2, "the restored bytes are upstream-exact");
    assert.equal(call.body.agent, "builder");

    const chatInject = wire.find(
      (message) =>
        message.method === "inject" && message.params?.include?.compaction === undefined
    );
    const restoreInject = injects(wire, "restore")[0];
    assert.ok(restoreInject, "the restore read reached the engine");
    assert.equal(restoreInject.params.session_id, "sess-A");
    assert.equal(
      restoreInject.params.project_id,
      chatInject.params.project_id,
      "the restore inject also carries the preceding chat.message project_id"
    );
  }
);

test(
  "test_compacted_event_without_a_session_id_restores_nothing",
  { timeout: 30000 },
  async (t) => {
    const { record, wire } = await runCase(t, {
      fixture: withFixture({
        // Its OWN steps: NO compacting step (that hook's readSession would push
        // sess-A into sessionGetCalls and make [] unreachable for a correct
        // handler). The chat.message step plants the sess-OTHER literal a
        // cache read would put on the wire.
        steps: ["chat.message", { hook: "compacted", sessionID: "sess-A", compactedNoId: true }],
      }),
    });

    assert.deepEqual(record.sessionGetCalls, [], "an id-less event never reads a session");
    assert.equal(
      injects(wire, "restore").length,
      0,
      "no restore inject without a session id"
    );
    assert.equal(record.promptCalls.length, 0);
  }
);

// ── AC 3: agent + model ──────────────────────────────────────────────────

test(
  "test_compacted_restore_passes_the_resolved_agent_and_model_explicitly",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      fixture: withFixture({ steps: ["compacted"] }),
    });
    const call = record.promptCalls[0];
    assert.equal(call.body.agent, "builder", "session.get().agent is absent; the history resolves");
    assert.deepEqual(
      call.body.model,
      { providerID: "acme", modelID: "model-a" },
      "the model is {providerID, modelID} — never the raw session.model"
    );
    assert.deepEqual(Object.keys(call.body.model).sort(), ["modelID", "providerID"]);
    assert.equal(call.body.variant, "high", "the variant is forwarded when not \"default\"");
  }
);

test(
  "test_compacted_restore_skips_the_compaction_summary_when_resolving_the_agent",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      fixture: withFixture({ steps: ["compacted"] }),
    });
    assert.equal(
      record.promptCalls[0].body.agent,
      "builder",
      "the trailing summary message (mode: compaction) is skipped, never resolved"
    );
  }
);

test(
  "test_compacted_restore_skips_when_no_agent_can_be_resolved",
  { timeout: 30000 },
  async (t) => {
    const { record, wire } = await runCase(t, {
      fixture: withFixture({ sessionMessages: {}, steps: ["compacted"] }),
    });
    assert.equal(record.promptCalls.length, 0, "upstream returns early when no agent resolves");
    assert.equal(record.toastBodies.length, 0, "no send means no toast");
    assert.equal(
      injects(wire, "restore").length,
      1,
      "the block WAS fetched: the early return happens after a successful read"
    );
  }
);

// ── AC 4: child sessions ─────────────────────────────────────────────────

function fixtureC() {
  const base = baseFixture();
  return {
    ...base,
    sessions: {
      ...base.sessions,
      // `agent: "sub"` makes a no-gate handler reach the prompt call, so the
      // child's 0 is not vacuous.
      "sess-K": { parentID: "sess-A", agent: "sub" },
    },
  };
}

test(
  "test_child_session_restores_nothing_while_the_parent_in_the_same_fixture_does",
  { timeout: 30000 },
  async (t) => {
    const { record, wire } = await runCase(t, {
      fixture: {
        ...fixtureC(),
        steps: [
          { hook: "compacted", sessionID: "sess-K" },
          { hook: "compacted", sessionID: "sess-A" },
        ],
      },
    });

    assert.equal(
      record.promptCalls.filter((call) => call.path.id === "sess-K").length,
      0,
      "a child session restores nothing"
    );
    assert.equal(
      record.promptCalls.filter((call) => call.path.id === "sess-A").length,
      1,
      "the parent in the same run DOES restore (positive control)"
    );
    assert.equal(record.toastBodies.length, 1, "only the parent's restore toasts");
    assert.ok(
      injects(wire, "restore").every((message) => message.params.session_id !== "sess-K"),
      "the child's gate returns before the request: no sess-K on the wire"
    );
  }
);

test(
  "test_child_session_ingest_is_a_no_op_while_the_parent_gets_context",
  { timeout: 30000 },
  async (t) => {
    const { record, wire } = await runCase(t, {
      fixture: {
        ...fixtureC(),
        steps: [
          { hook: "compacting", sessionID: "sess-K" },
          { hook: "compacting", sessionID: "sess-A" },
        ],
      },
    });

    assert.deepEqual(
      record.contextAfter["sess-K"],
      [],
      "the child's hook RAN and appended nothing (contextAfter is keyed by session)"
    );
    assert.deepEqual(
      record.contextAfter["sess-A"],
      [INGEST_2],
      "the parent in the same run gets the ingest string (positive control)"
    );
    assert.ok(
      injects(wire, "ingest").every((message) => message.params.session_id !== "sess-K"),
      "the child's gate returns before the request"
    );
  }
);

// ── AC 7: fail open ──────────────────────────────────────────────────────

test(
  "test_compaction_paths_fail_open_without_an_engine",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      stageEngine: false,
      fixture: withFixture({ steps: ["compacting", "compacted"] }),
    });
    assert.equal(record.calls.length, 2, "both hooks were reached");
    assert.ok(record.calls.every((call) => call.settled), "neither hook threw or hung");
    assert.deepEqual(record.contextAfter["sess-A"], [], "no context without an engine");
    assert.equal(record.promptCalls.length, 0);
    assert.equal(record.toastBodies.length, 0);
  }
);

test(
  "test_compaction_paths_fail_open_when_the_engine_errors",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      injectMode: "error",
      fixture: withFixture({ steps: ["compacting", "compacted"] }),
    });
    assert.ok(record.calls.every((call) => call.settled), "the JSON-RPC error is swallowed");
    assert.deepEqual(record.contextAfter["sess-A"], []);
    assert.equal(record.promptCalls.length, 0, "a failed inject never prompts")
    assert.equal(record.toastBodies.length, 0);
  }
);

test(
  "test_compaction_paths_do_nothing_when_the_engine_returns_no_block",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      injectMode: "success",
      fixture: withFixture({ steps: ["compacting", "compacted"] }),
    });
    assert.deepEqual(record.contextAfter["sess-A"], []);
    assert.equal(record.promptCalls.length, 0, "no block means no prompt, not an empty prompt");
  }
);

test(
  "test_compaction_hook_does_not_block_when_the_engine_never_answers",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      injectMode: "silent",
      fixture: withFixture({ steps: ["compacting"] }),
    });
    assert.equal(record.calls.length, 1, "one call measured");
    assert.equal(
      record.calls[0].settled,
      true,
      "the hook settled before the driver's 3000 ms guard"
    );
    assert.ok(record.calls[0].elapsedMs < 3000, `settled in ${record.calls[0].elapsedMs} ms`);
    assert.equal(record.promptCalls.length, 0);
  }
);

// ── The toast oracle ─────────────────────────────────────────────────────

test(
  "test_restore_toast_reports_the_engine_memory_count_not_the_heading_count",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      fixture: withFixture({ steps: ["compacted"] }),
    });
    assert.deepEqual(record.toastBodies, [
      {
        title: "Memory Restored",
        message: "7 memories injected after compaction",
        variant: "success",
        duration: 3000,
      },
    ]);
  }
);

test(
  "test_restore_toast_is_not_deduped_per_process",
  { timeout: 30000 },
  async (t) => {
    const { record } = await runCase(t, {
      fixture: withFixture({ steps: ["compacted"], compactedRepeat: 2 }),
    });
    assert.equal(record.toastBodies.length, 2, "upstream toasts every restore, never once per process");
    assert.deepEqual(record.toastBodies[1], record.toastBodies[0]);
    assert.equal(record.promptCalls.length, 2, "two events restore twice");
  }
);
