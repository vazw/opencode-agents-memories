// W5.4 — the `recall_project_sessions` tool: its W4.2 descriptor, the
// per-tool scope checks, and the wire/result mapping. No engine and no
// subprocess here; the end-to-end case lives in test 18 of this file.
//
// Every failure is a VALUE (`{ success: false, error }`), never a throw: the
// host would surface a throw as a `Tool.Error` with no recovery instruction.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  RECALL_TOOL_DESCRIPTION,
  RECALL_TOOL_INPUT,
  RECALL_TOOL_NAME,
  createRecallTool,
} from "../src/tools.js";
import { setup } from "../src/plugin.js";
import {
  DRIVER_SOURCE,
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const fixture = JSON.parse(
  fs.readFileSync(path.join(here, "fixtures", "recall-tool.json"), "utf8")
);

/** The tool result payload, parsed from its single `content` string. */
const payload = async (invocation) => JSON.parse((await invocation).content);

/** A recording `sendRecall` fake; an override page may be supplied. */
function recordingSendRecall(page = { count: 0, items: [] }) {
  const calls = [];
  const sendRecall = async (params) => {
    calls.push(params);
    return page;
  };
  return { calls, sendRecall };
}

test("test_recall_tool_declares_the_w42_descriptor", () => {
  assert.equal(RECALL_TOOL_NAME, "recall_project_sessions");
  assert.deepEqual(
    RECALL_TOOL_INPUT,
    fixture.input,
    "the model-facing input is the W4.2 schema, not a hand-rolled copy"
  );
  assert.equal(RECALL_TOOL_DESCRIPTION, fixture.description);
});

test("test_recall_tool_sends_the_host_project_and_the_call_time_session", async () => {
  const { calls, sendRecall } = recordingSendRecall();
  const tool = createRecallTool({ projectId: "/workspace/project", sendRecall });

  const first = await tool.execute(
    { projectId: "/workspace/project", lastNSessions: 5 },
    { sessionID: "ses-1" }
  );
  const second = await tool.execute(
    { projectId: "/workspace/project", lastNSessions: 5 },
    { sessionID: "ses-2" }
  );
  const third = await tool.execute(
    { projectId: "/workspace/project", lastNSessions: 5, sessionId: "filter-1" },
    { sessionID: "ses-3" }
  );

  assert.deepEqual(
    calls.map((call) => call.session_id),
    ["ses-1", "ses-2", "ses-3"],
    "the envelope session id is per call, read from the tool context"
  );
  assert.deepEqual(calls[0], {
    session_id: "ses-1",
    projectId: "/workspace/project",
    lastNSessions: 5,
  });
  // The envelope `session_id` (call context) and the W4.2 filter `sessionId`
  // (model argument) are different keys: both travel, neither is renamed.
  assert.deepEqual(calls[2], {
    session_id: "ses-3",
    projectId: "/workspace/project",
    lastNSessions: 5,
    sessionId: "filter-1",
  });
  assert.deepEqual(await payload(first), { summaries: [] });
});

test("test_recall_tool_refuses_a_foreign_project", async () => {
  const { calls, sendRecall } = recordingSendRecall();
  const tool = createRecallTool({ projectId: "/workspace/project", sendRecall });

  assert.deepEqual(
    await payload(tool.execute({ projectId: "/workspace/other" }, { sessionID: "ses-1" })),
    { success: false, error: "scope_rejected" }
  );
  // A malformed call is a caller fault, never a scope mismatch.
  assert.deepEqual(await payload(tool.execute({}, { sessionID: "ses-1" })), {
    success: false,
    error: "invalid_params",
  });
  assert.deepEqual(
    await payload(tool.execute({ projectId: 42 }, { sessionID: "ses-1" })),
    { success: false, error: "invalid_params" }
  );
  assert.equal(calls.length, 0, "no refusal ever reaches the engine");
});

test("test_recall_tool_fails_closed_without_a_host_project", async () => {
  const { calls, sendRecall } = recordingSendRecall();
  const tool = createRecallTool({ projectId: undefined, sendRecall });

  // The host check precedes the argument check, so a host with no directory
  // is not reported as a malformed call.
  assert.deepEqual(
    await payload(tool.execute({ projectId: "/workspace/project" }, { sessionID: "ses-1" })),
    { success: false, error: "scope_unavailable" }
  );
  assert.equal(calls.length, 0);
});

test("test_recall_tool_maps_the_engine_page_to_the_w42_result", async () => {
  const page = {
    count: 2,
    items: [
      { id: "a", summary: "s", similarity: 0.87 },
      { id: "b", summary: "t" },
    ],
  };

  // (a) semantic: the request arguments must reach the wire, or the tool's
  // semantic path is dead while the payload shape still looks right.
  const semanticCalls = [];
  const tool = createRecallTool({
    projectId: "/workspace/project",
    sendRecall: async (params) => {
      semanticCalls.push(params);
      return page;
    },
  });
  const semantic = await tool.execute(
    { projectId: "/workspace/project", mode: "semantic", query: "vector store" },
    { sessionID: "ses-1" }
  );
  assert.deepEqual(semanticCalls[0], {
    session_id: "ses-1",
    projectId: "/workspace/project",
    mode: "semantic",
    query: "vector store",
  });
  assert.equal(semanticCalls[0].query, "vector store");
  assert.equal(semanticCalls[0].mode, "semantic");
  assert.deepEqual(await payload(semantic), { summaries: page.items });
  assert.deepEqual(Object.keys(await payload(semantic)), ["summaries"]);

  // (b) full sessions: the key is chosen from the REQUEST argument, and that
  // argument is on the wire.
  const fullCalls = [];
  const fullTool = createRecallTool({
    projectId: "/workspace/project",
    sendRecall: async (params) => {
      fullCalls.push(params);
      return page;
    },
  });
  const full = await fullTool.execute(
    { projectId: "/workspace/project", includeFullSessions: true },
    { sessionID: "ses-1" }
  );
  assert.equal(fullCalls[0].includeFullSessions, true);
  assert.deepEqual(Object.keys(await payload(full)), ["fullSessions"]);

  // (c) empty page: never null, never a missing key.
  const emptyTool = createRecallTool({
    projectId: "/workspace/project",
    sendRecall: async () => ({ count: 0, items: [] }),
  });
  assert.deepEqual(
    await payload(emptyTool.execute({ projectId: "/workspace/project" }, { sessionID: "ses-1" })),
    { summaries: [] }
  );

  // (d) malformed page: a value, not a throw.
  const noItems = createRecallTool({
    projectId: "/workspace/project",
    sendRecall: async () => ({ count: 0 }),
  });
  assert.deepEqual(
    await payload(noItems.execute({ projectId: "/workspace/project" }, { sessionID: "ses-8" })),
    { success: false, error: "engine_unavailable" }
  );
  const notAnArray = createRecallTool({
    projectId: "/workspace/project",
    sendRecall: async () => ({ count: 1, items: "nope" }),
  });
  assert.deepEqual(
    await payload(notAnArray.execute({ projectId: "/workspace/project" }, { sessionID: "ses-9" })),
    { success: false, error: "engine_unavailable" }
  );
});

test("test_recall_tool_reports_an_engine_failure", async () => {
  const tool = createRecallTool({
    projectId: "/workspace/project",
    // Exactly the shape `handleWireLine` produces on a JSON-RPC error.
    sendRecall: async () => {
      throw Object.assign(new Error("degraded"), { code: -32001 });
    },
  });
  assert.deepEqual(
    await payload(tool.execute({ projectId: "/workspace/project" }, { sessionID: "ses-1" })),
    { success: false, error: "engine_unavailable" }
  );
});

test("test_setup_registers_the_recall_tool", async () => {
  // The fake host is ASYNC, because the real one is: `Transform` returns a
  // Promise and `dispose` returns a Promise, and the callback is deferred so a
  // missing `await` is observable rather than invisible.
  const added = [];
  const lifecycle = [];
  const ctx = {
    location: {
      directory: "/workspace/project",
      project: { directory: "/workspace/project" },
    },
    session: { hook: async () => {} },
    event: {
      subscribe: async function* ({ signal }) {
        signal.addEventListener("abort", () => lifecycle.push("adapter"));
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
    tool: {
      transform: (callback) =>
        Promise.resolve()
          .then(() => callback({ add: (tool) => added.push(tool) }))
          .then(() => ({
            dispose: async () => {
              await new Promise((resolve) => setTimeout(resolve, 0));
              lifecycle.push("tool");
            },
          })),
    },
  };
  const cleanup = await setup(ctx);
  assert.equal(added.length, 1, "setup registers exactly one tool");
  assert.equal(added[0].name, "recall_project_sessions");
  // The host derives the callable name from `options.namespace` when it is
  // set (`effectiveName`): no namespace keeps the bare name callable.
  assert.equal(added[0].options?.namespace, undefined);
  assert.equal(typeof added[0].execute, "function");
  assert.deepEqual(added[0].input, fixture.input);
  await cleanup();
  assert.deepEqual(lifecycle, ["tool", "adapter"], "tool cleanup precedes the adapter cleanup");
});

test("test_setup_survives_a_host_without_a_tool_surface", async () => {
  const ctx = {
    location: {
      directory: "/workspace/project",
      project: { directory: "/workspace/project" },
    },
    // `registerV2Adapter` throws without this: a fake that trips it would be
    // testing the adapter, not the fail-open decision.
    session: { hook: async () => {} },
    event: {
      subscribe: async function* ({ signal }) {
        if (signal.aborted) return;
        await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
      },
    },
  };
  const cleanup = await setup(ctx);
  assert.equal(typeof cleanup, "function", "setup resolves and returns a cleanup");
  await cleanup();
});

test(
  "test_tool_call_reaches_the_engine_as_recall",
  { timeout: 60000 },
  async (t) => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "memory-tools-"));
    const pkgDir = stagePackageTree(tmp);
    const requestLogPath = path.join(tmp, "wire.log");
    const manifestPath = path.join(tmp, "stub.json");
    stageStubEngine(pkgDir, { requestLogPath, manifestPath });
    t.after(() => assertReaped(manifestPath));

    const fixturePath = path.join(tmp, "fixture.json");
    fs.writeFileSync(
      fixturePath,
      JSON.stringify({ directory: "/workspace/project", sessionID: "ses-tool" })
    );
    const donePath = path.join(tmp, "done.json");
    const driverPath = path.join(tmp, "driver.mjs");
    fs.writeFileSync(driverPath, DRIVER_SOURCE);
    const run = await runDriver(driverPath, [pkgDir, fixturePath], {
      ...process.env,
      DRIVER_MODE: "v2-tool",
      DRIVER_DONE_PATH: donePath,
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      STUB_LOG: requestLogPath,
    });
    assert.equal(
      run.code,
      0,
      `the v2-tool driver failed (signal=${String(run.signal)}): stderr=${run.stderr}`
    );
    assert.equal(run.timedOut, false);

    const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
    assert.deepEqual(record.names, ["recall_project_sessions"]);
    assert.deepEqual(JSON.parse(record.payloadText), { summaries: [] });
    const recallLines = record.wire.filter(
      (message) => message && message.method === "recall"
    );
    assert.equal(recallLines.length, 1, "exactly one recall line reached the engine");
    assert.deepEqual(recallLines[0].params, {
      session_id: "ses-tool",
      projectId: "/workspace/project",
    });
  }
);
