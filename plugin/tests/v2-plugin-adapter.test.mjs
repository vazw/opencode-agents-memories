// W5.2 `js` shard — the V2→V1 adapter, ported from
// example/opencode-mem/src/v2/adapter.ts and tests/v2-plugin-adapter.test.ts.
// Six cases here (blueprint D6, Step 3) + the end-to-end capture case appended
// at Step 4. The fakes are the upstream ones: a `hooks` map, a fake ctx whose
// `session.hook` records callbacks, and an event generator the test drives by
// hand (upstream `v2-plugin-adapter.test.ts:5-66`).
//
// Two port-specific hazards are asserted, not assumed:
//   H1 — the injected part lives on `output.message.parts` (the port's V1 hook
//        pushes there), never `output.parts`;
//   H2 — `toLegacyEvent` merges the legacy type INTO the object it returns, so
//        `properties.sessionID` survives compaction invalidation.
//
// The tool and `chat.params` halves of upstream's file are N-A (issues 22/26):
// the port's V1 wrapper has neither, and an adapter that registered them would
// create a surface no engine path backs.

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

const ADAPTER_MODULE = "../src/v2/adapter.js";

// Upstream's hand-driven fake: the generator parks on `nextEvent` and reports
// `handled()` only after the consumer comes back for the next value — i.e.
// after the watcher finished processing the event it was given.
function createHarness() {
  const hooks = new Map();
  const getCalls = [];
  let nextEvent = null;
  let handled = null;
  let closed = false;
  const ctx = {
    location: {
      directory: "/workspace/project",
      project: { id: "project", directory: "/workspace/project" },
    },
    session: {
      hook: async (name, callback) => {
        hooks.set(name, callback);
      },
      get: async (input) => {
        getCalls.push(input);
        return { location: { directory: "/workspace/project" } };
      },
    },
    event: {
      async *subscribe({ signal }) {
        try {
          while (!signal.aborted) {
            const event = await new Promise((resolve) => {
              nextEvent = resolve;
              signal.addEventListener("abort", () => resolve(null), { once: true });
            });
            if (event) yield event;
            if (handled) handled();
          }
        } finally {
          closed = true;
        }
      },
    },
  };

  async function deliver(raw) {
    const deadline = Date.now() + 2000;
    while (typeof nextEvent !== "function") {
      if (Date.now() > deadline) throw new Error("the event subscription never parked");
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
    const seen = new Promise((resolve) => {
      handled = resolve;
    });
    nextEvent(raw);
    await seen;
  }

  return { ctx, hooks, getCalls, closed: () => closed, deliver };
}

test("test_v2_adapter_prompt_hook_runs_the_v1_chat_message_hook", async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);

  await assert.rejects(
    () => registerV2Adapter({ session: {} }, {}),
    /v2 adapter requires ctx\.session\.hook/,
    "a host that is not V2 must not silently load the V2 entry"
  );

  const harness = createHarness();
  const calls = [];
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (input, output) => {
      calls.push({ input, output });
      output.message.parts.push({ type: "text", text: "remembered context", synthetic: true });
      output.message.parts.push({ type: "tool", tool: "edit" });
    },
    event: async () => {},
  });

  await harness.hooks.get("prompt")({
    sessionID: "ses-1",
    messageID: "msg-1",
    prompt: { text: "Implement V2" },
  });

  assert.equal(calls.length, 1, "the V1 chat.message hook ran once");
  assert.deepEqual(calls[0].input, { sessionID: "ses-1" }, "H4: the session id is hook data");
  // H1: the output carries `message.parts`, the shape the port's V1 hook pushes
  // onto (`plugin/src/plugin.js:598-603`).
  assert.deepEqual(calls[0].output, {
    message: {
      id: "msg-1",
      parts: [
        { type: "text", text: "Implement V2" },
        { type: "text", text: "remembered context", synthetic: true },
        { type: "tool", tool: "edit" },
      ],
    },
  });

  const contextEvent = { sessionID: "ses-1", system: [] };
  await harness.hooks.get("context")(contextEvent);
  assert.deepEqual(
    contextEvent.system,
    [{ type: "text", text: "remembered context" }],
    "only the injected text parts are harvested, joined by newline"
  );

  await cleanup();
});

test("test_v2_adapter_context_hook_injects_only_the_cached_text", async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (_input, output) => {
      output.message.parts.push({ type: "text", text: "first block" });
      output.message.parts.push({ type: "text", text: "second block" });
    },
    event: async () => {},
  });

  await harness.hooks.get("prompt")({
    sessionID: "ses-1",
    messageID: "msg-1",
    prompt: { text: "prompt" },
  });

  const sameSession = { sessionID: "ses-1", system: [] };
  await harness.hooks.get("context")(sameSession);
  assert.deepEqual(
    sameSession.system,
    [{ type: "text", text: "first block\nsecond block" }],
    "every injected text part is stored, joined by newline"
  );

  const otherSession = { sessionID: "ses-2", system: [] };
  await harness.hooks.get("context")(otherSession);
  assert.deepEqual(otherSession.system, [], "a session with no cached injection gets nothing");

  await cleanup();
});

test("test_v2_adapter_invalidates_the_session_cache_only_for_compaction_at_this_location", async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  let revision = 1;
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (input, output) => {
      output.message.parts.push({ type: "text", text: `${input.sessionID}-${revision}` });
    },
    event: async () => {},
  });

  const prompt = (sessionID) =>
    harness.hooks.get("prompt")({
      sessionID,
      messageID: `msg-${sessionID}`,
      prompt: { text: "prompt" },
    });
  const context = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  await prompt("ses-1");
  await prompt("ses-2");
  assert.deepEqual(await context("ses-1"), ["ses-1-1"]);
  assert.deepEqual(await context("ses-2"), ["ses-2-1"]);

  revision += 1;
  await harness.deliver({
    type: "session.compaction.ended",
    location: { directory: "/other/project" },
    data: { sessionID: "ses-1" },
  });
  assert.deepEqual(
    await context("ses-1"),
    ["ses-1-1"],
    "an event for another project must not invalidate this session's cache"
  );

  await harness.deliver({
    type: "session.compaction.ended",
    location: { directory: "/workspace/project" },
    data: { sessionID: "ses-1" },
  });
  assert.deepEqual(
    await context("ses-1"),
    [],
    "a compaction at this location drops the session's cache (H2: properties.sessionID survived)"
  );
  assert.deepEqual(await context("ses-2"), ["ses-2-1"], "other sessions are untouched");

  await prompt("ses-1");
  assert.deepEqual(await context("ses-1"), ["ses-1-2"], "a later prompt re-populates the cache");

  await cleanup();
});

test("test_v2_adapter_forwards_location_filtered_events_to_the_v1_event_hook", async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const delivered = [];
  const cleanup = await registerV2Adapter(harness.ctx, {
    event: async ({ event }) => {
      delivered.push(event);
    },
  });

  await harness.deliver({
    type: "session.execution.succeeded",
    location: { directory: "/elsewhere" },
    data: { sessionID: "ses-1" },
  });
  assert.deepEqual(delivered, [], "an event outside this project never reaches the V1 hook");

  await harness.deliver({ type: "session.execution.succeeded", data: { sessionID: "ses-1" } });
  assert.deepEqual(harness.getCalls, [{ sessionID: "ses-1" }], "a location-less event is resolved by its session");
  assert.deepEqual(
    delivered,
    [{ type: "session.idle", properties: { sessionID: "ses-1", type: "session.idle" } }],
    "H2: the alias and the merged type reach the V1 hook together"
  );

  await harness.deliver({
    type: "message.updated",
    location: { directory: "/workspace/project" },
    data: { sessionID: "ses-1" },
  });
  assert.deepEqual(delivered[1], {
    type: "message.updated",
    properties: { sessionID: "ses-1", type: "message.updated" },
  });

  await cleanup();
});

test("test_v2_adapter_cleanup_aborts_the_subscription_and_is_idempotent", async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const delivered = [];
  const cleanup = await registerV2Adapter(harness.ctx, {
    event: async ({ event }) => {
      delivered.push(event);
    },
  });

  await harness.deliver({
    type: "message.updated",
    location: { directory: "/workspace/project" },
    data: { sessionID: "ses-1" },
  });
  assert.equal(delivered.length, 1);

  await cleanup();
  assert.equal(harness.closed(), true, "cleanup aborts the controller and awaits the watcher");
  await cleanup();
  assert.equal(harness.closed(), true, "a second cleanup must not throw or reopen the subscription");
});

test("test_v2_adapter_registers_no_tool_and_no_skills", async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const transformed = [];
  harness.ctx.tool = {
    transform: async () => {
      transformed.push("tool");
    },
  };
  harness.ctx.skill = {
    transform: async () => {
      transformed.push("skill");
    },
    reload: async () => {},
  };

  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async () => {},
    event: async () => {},
  });

  assert.deepEqual(transformed, [], "tool registration is issue 22's; skill registration is issue 26's");
  assert.deepEqual(
    [...harness.hooks.keys()].sort(),
    ["context", "prompt"],
    "the adapter registers exactly the two session hooks the port's V1 wrapper answers"
  );

  await cleanup();
});

// The end-to-end case (blueprint Step 4.1 / AC10): a V2
// `session.execution.succeeded` event produces exactly one wire `capture`,
// through ONE staged engine child, carrying the mapped prompt and the host
// directory as the project id — the same V1 closure, the same five methods.
// It drives `mode: "v2"` of the shared harness, so the engine child belongs to
// a subprocess and never sits on `node:test`'s event loop.
test(
  "test_v2_adapter_idle_event_captures_the_transcript_through_the_legacy_client",
  { timeout: 60000 },
  async (t) => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-v2-adapter-e2e-"));
    const pkgDir = stagePackageTree(tmp);
    const requestLogPath = path.join(tmp, "stub-requests.ndjson");
    const manifestPath = path.join(tmp, "stub-manifest.json");
    const donePath = path.join(tmp, "driver-done.json");
    stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode: "success" });
    t.after(() => assertReaped(manifestPath));

    const directory = path.join(tmp, "project");
    const fixturePath = path.join(tmp, "fixture.json");
    fs.writeFileSync(
      fixturePath,
      JSON.stringify({
        location: { directory, project: { id: "project-1", directory } },
        session: {
          // Fixture precondition: a `user` message carrying `text`, then an
          // `assistant` message with a non-empty text part.
          // `fireIdleCapture` has three early returns between the schema and a
          // capture (`plugin/src/plugin.js:683`, `:684`, `:720`).
          context: [
            { type: "user", id: "msg-u1", text: "fix the login bug", agent: "build" },
            {
              type: "assistant",
              id: "msg-a1",
              agent: "build",
              content: [{ type: "text", text: "done — updated the handler" }],
            },
          ],
          get: { "ses-1": { location: { directory } } },
        },
        events: [
          {
            type: "session.execution.succeeded",
            location: { directory },
            data: { sessionID: "ses-1" },
          },
        ],
        rpc: { registered: true },
      })
    );

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
    assert.equal(
      record.helloCount,
      1,
      "one engine client: a V2 setup spawns exactly one child and sends exactly one hello"
    );
    assert.ok(record.captureLine !== null, "the idle event captured the transcript");
    const capture = record.captureLine;
    const context = capture.params.metadata.autoCaptureContext;
    assert.equal(
      context.userPrompt,
      "fix the login bug",
      "the mapped user prompt reaches the capture"
    );
    assert.deepEqual(
      context.textResponses,
      ["done — updated the handler"],
      "the mapped assistant text reaches the capture"
    );
    assert.equal(capture.params.project_id, directory, "project_id is ctx.location.directory");
    assert.equal(
      capture.params.session_id,
      "ses-1",
      "H4: the hook's session id, never the synthetic oc-bridge-<pid>"
    );
  }
);
