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
import path from "node:path";

import {
  DRIVER_SOURCE,
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
  tempRoot,
} from "./harness/bridge.mjs";

const ADAPTER_MODULE = "../src/v2/adapter.js";

// Upstream's hand-driven fake: the generator parks on `nextEvent` and reports
// `handled()` only after the consumer comes back for the next value — i.e.
// after the watcher finished processing the event it was given.
function createHarness() {
  const hooks = new Map();
  const getCalls = [];
  // PRD 171 §4: the host transcript the adapter's fallback probes. The default
  // arm is `[]` — a session with no conversation — so a row that reads an
  // uncached session proves the fail-closed floor, not a lucky lookup.
  const contextCalls = [];
  let transcript = [];
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
      context: async (input) => {
        contextCalls.push(input);
        return transcript;
      },
      get: async (input) => {
        getCalls.push(input);
        if (input?.sessionID === "ses-boom") throw new Error("host lookup failed");
        // The only fixture that resolves to a DIFFERENT project through the
        // session fallback: every directory-bearing envelope returns at the
        // directory branch, so the session comparison is decided here.
        if (input?.sessionID === "ses-foreign") return { location: { directory: "/workspace/elsewhere" } };
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
          // Deferred by a macrotask so the assertion can tell an awaited
          // teardown from a cleanup that returns before the watcher ends.
          await new Promise((resolve) => setTimeout(resolve, 0));
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

  return {
    ctx,
    hooks,
    getCalls,
    contextCalls,
    closed: () => closed,
    deliver,
    setTranscript: (messages) => {
      transcript = messages;
    },
  };
}

test("test_v2_adapter_prompt_hook_runs_the_v1_chat_message_hook", { timeout: 5000 }, async () => {
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

test("test_v2_adapter_context_hook_injects_only_the_cached_text", { timeout: 5000 }, async () => {
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

test("test_v2_adapter_invalidates_the_session_cache_only_for_compaction_at_this_location", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  let revision = 1;
  const delivered = [];
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (input, output) => {
      output.message.parts.push({ type: "text", text: `${input.sessionID}-${revision}` });
    },
    event: async ({ event }) => { delivered.push(event); },
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
    ["ses-1-1"],
    "issue 124: preserve same-turn cache; historical test name retained"
  );
  assert.deepEqual(await context("ses-2"), ["ses-2-1"], "other sessions are untouched");

  assert.equal(delivered.length, 1);
  assert.equal(delivered[0].type, "session.compacted");
  await prompt("ses-1");
  assert.deepEqual(await context("ses-1"), ["ses-1-2"], "a later prompt re-populates the cache");

  await cleanup();
});

test("test_v2_adapter_forwards_location_filtered_events_to_the_v1_event_hook", { timeout: 5000 }, async () => {
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

  // The upstream envelope shapes, each one decision the filter must make.
  // A trailing slash is the same project after resolve().
  await harness.deliver({
    type: "session.execution.succeeded",
    location: { directory: "/workspace/project/" },
    data: { sessionID: "ses-1" },
  });
  assert.equal(delivered.length, 3, "a trailing slash compares equal after resolve()");

  // A payload-wrapped envelope: the wrapper is unwrapped, the version suffix
  // stripped, and the location decides.
  await harness.deliver({
    payload: {
      type: "session.execution.succeeded.1",
      location: { directory: "/workspace/project" },
      data: { sessionID: "ses-1" },
    },
  });
  assert.deepEqual(
    delivered[3],
    {
      type: "session.idle",
      properties: { sessionID: "ses-1", type: "session.idle" },
    },
    "a payload-wrapped envelope is unwrapped and its location decides"
  );

  // An id behind `data.session.id` resolves through the host.
  await harness.deliver({ type: "message.updated", data: { session: { id: "ses-1" } } });
  assert.equal(delivered.length, 5, "an id at data.session.id resolves by session");

  // Every directory-bearing shape is read BEFORE the session fallback: a
  // foreign directory carried alongside a session that resolves HERE is
  // dropped only when its own directory source is consulted.
  for (const foreign of [
    { type: "message.updated", directory: "/elsewhere", data: { sessionID: "ses-1" } },
    {
      type: "message.updated",
      payload: { type: "message.updated", directory: "/elsewhere", data: { sessionID: "ses-1" } },
    },
    {
      type: "message.updated",
      payload: {
        type: "message.updated",
        location: { directory: "/elsewhere" },
        data: { sessionID: "ses-1" },
      },
    },
    {
      type: "message.updated",
      data: { info: { directory: "/elsewhere", id: "ses-1" } },
    },
  ]) {
    await harness.deliver(foreign);
  }
  assert.equal(
    delivered.length,
    5,
    "every upstream directory source decides before the session fallback"
  );

  // A host lookup error drops that event and never kills the subscription.
  await harness.deliver({ type: "message.updated", data: { sessionID: "ses-boom" } });
  assert.equal(delivered.length, 5, "a throwing session lookup drops the event");

  // The session fallback's own decision, which every directory-bearing shape
  // above returns before reaching: a session that lives in another project is
  // dropped. A successful lookup that forwards regardless would capture this
  // project's transcript under the foreign session's idle event.
  await harness.deliver({ type: "message.updated", data: { sessionID: "ses-foreign" } });
  assert.equal(delivered.length, 5, "a session that lives in another project is dropped");

  await harness.deliver({
    type: "message.updated",
    location: { directory: "/workspace/project" },
    data: { sessionID: "ses-1" },
  });
  assert.equal(delivered.length, 6, "the subscription survives a lookup error");

  await harness.deliver({ type: "message.updated" });
  assert.equal(delivered.length, 6, "an event with no directory and no session id is dropped");

  await cleanup();
});

test("test_v2_adapter_cleanup_aborts_the_subscription_and_is_idempotent", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const delivered = [];
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (_input, output) => {
      output.message.parts.push({ type: "text", text: "cached before cleanup" });
    },
    event: async ({ event }) => {
      delivered.push(event);
    },
  });
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  await harness.deliver({
    type: "message.updated",
    location: { directory: "/workspace/project" },
    data: { sessionID: "ses-1" },
  });
  assert.equal(delivered.length, 1);

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-1", prompt: { text: "prompt" } });
  assert.deepEqual(await readContext("ses-1"), ["cached before cleanup"]);

  await cleanup();
  assert.equal(harness.closed(), true, "cleanup aborts the controller and awaits the watcher");
  // PRD 171 §4: a host transcript exists AFTER teardown, so only the disposed
  // flag stands between this late hook and a fallback replay.
  harness.ctx.session.context = async () => [{ type: "user", text: "x" }];
  assert.deepEqual(
    await readContext("ses-1"),
    [],
    "cleanup drops the cached block, so a late context hook injects nothing"
  );
  await cleanup();
  assert.equal(harness.closed(), true, "a second cleanup must not throw or reopen the subscription");

  // A stream that throws is contained: the watcher resolves, cleanup returns,
  // and no unhandled rejection reaches the host loop.
  const failing = createHarness();
  failing.ctx.event.subscribe = async function* () {
    throw new Error("stream failed");
  };
  const failingCleanup = await registerV2Adapter(failing.ctx, { event: async () => {} });
  await failingCleanup();
});

test("test_v2_adapter_registers_no_tool_and_no_skills", { timeout: 5000 }, async () => {
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

  assert.deepEqual(transformed, [], "tool registration is issue 22's; skill registration is issue 69's (setup, not the adapter)");
  assert.deepEqual(
    [...harness.hooks.keys()].sort(),
    ["compaction", "context", "prompt"],
    "the adapter registers exactly the three supported session hooks: prompt, context and compaction"
  );

  await cleanup();
});

// The end-to-end case (blueprint Step 4.1 / AC10): a V2
// `session.execution.succeeded` event produces exactly one wire `capture`,
// through ONE staged engine child, carrying the mapped prompt and the host
// directory as the project id — the same V1 closure, the same six methods.
// It also fires the recorded `prompt` hook against an erroring stub, so the
// toast path (`setup` → legacy client → `notifyHost` → RPC emit) is exercised
// under V2, the one route the V1-only driver can never reach.
// It drives `mode: "v2"` of the shared harness, so the engine child belongs to
// a subprocess and never sits on `node:test`'s event loop.
test(
  "test_v2_adapter_idle_event_captures_the_transcript_through_the_legacy_client",
  { timeout: 60000 },
  async (t) => {
    const tmp = tempRoot("om-v2-adapter-e2e-");
    const pkgDir = stagePackageTree(tmp);
    const requestLogPath = path.join(tmp, "stub-requests.ndjson");
    const manifestPath = path.join(tmp, "stub-manifest.json");
    const donePath = path.join(tmp, "driver-done.json");
    stageStubEngine(pkgDir, { requestLogPath, manifestPath, injectMode: "error" });
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
        prompt: { sessionID: "ses-1", messageID: "msg-p1", prompt: { text: "first turn" } },
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

    // The V2 toast path: the prompt hook's failing inject reached the host
    // through the legacy client's RPC emit — one inject on the wire, one toast.
    assert.equal(
      record.injectCount,
      1,
      "the prompt hook reached the engine's inject exactly once"
    );
    assert.deepEqual(
      record.toastEmits,
      [
        {
          name: "toast",
          data: {
            title: "opencode-mem",
            message: "Memory injection failed; memory may be disabled.",
            variant: "warning",
          },
        },
      ],
      "a failed inject under V2 emits one toast intent — never the error text, a path or a transcript"
    );
  }
);

// Issue 155: the two answers a THROWING or EMPTY refresh can get. Both cases
// run over the same first-mode harness — `shouldRefresh: ({ hasCache }) =>
// !hasCache` — so a refresh that returns nothing must be forced by the adapter's
// own marks (compaction, retry), never by the pin answering true.
function createFirstModeLegacy(handle) {
  const legacy = { "chat.message": handle };
  Object.defineProperty(legacy, "shouldRefresh", {
    value: ({ hasCache }) => !hasCache,
    enumerable: false,
  });
  return legacy;
}

test("test_v2_empty_answer_clears_the_kept_block", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  let calls = 0;
  let answer = "ok";
  const legacy = createFirstModeLegacy(async (_input, output) => {
    calls += 1;
    if (answer === "empty") {
      // What the wrapper does for an empty SUCCESS: no part, `injectStatus`.
      Object.defineProperty(output, "injectStatus", { value: "empty", enumerable: false });
      return;
    }
    output.message.parts.push({ type: "text", text: `BLOCK-${calls}` });
    Object.defineProperty(output, "injectStatus", { value: "ok", enumerable: false });
  });
  const cleanup = await registerV2Adapter(harness.ctx, legacy);
  const prompt = (messageID) => harness.hooks.get("prompt")({ sessionID: "ses-1", messageID, prompt: { text: "q" } });
  const context = async () => {
    const event = { sessionID: "ses-1", system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  await prompt("m1");
  assert.deepEqual(await context(), ["BLOCK-1"], "prompt 0 fills the cache");
  assert.equal(calls, 1);

  // The compaction mark forces prompt 1 to refresh even with a live cache.
  await harness.hooks.get("compaction")({ sessionID: "ses-1", system: [] });
  answer = "empty";
  await prompt("m2");
  assert.equal(calls, 2, "the compaction mark forced the refresh");
  assert.deepEqual(await context(), [], "an empty SUCCESS clears the withdrawn block");

  answer = "ok";
  await prompt("m3");
  assert.equal(calls, 3, "with no cache the next prompt refreshes");
  assert.deepEqual(await context(), ["BLOCK-3"], "and re-populates it");

  await cleanup();
});

test("test_v2_kept_block_forces_one_retry", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  let calls = 0;
  let answer = "ok";
  const legacy = createFirstModeLegacy(async (_input, output) => {
    calls += 1;
    if (answer === "throw") throw new Error("refresh lookup failed");
    output.message.parts.push({ type: "text", text: `BLOCK-${calls}` });
  });
  const cleanup = await registerV2Adapter(harness.ctx, legacy);
  const prompt = (messageID) => harness.hooks.get("prompt")({ sessionID: "ses-1", messageID, prompt: { text: "q" } });
  const context = async () => {
    const event = { sessionID: "ses-1", system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  await prompt("m1");
  assert.deepEqual(await context(), ["BLOCK-1"], "prompt 0 fills the cache");
  assert.equal(calls, 1);

  await harness.hooks.get("compaction")({ sessionID: "ses-1", system: [] });
  answer = "throw";
  // RED before issue 155: this throw rejected the prompt hook. It must resolve.
  await prompt("m2");
  assert.equal(calls, 2, "the compaction mark forced the throwing refresh");
  assert.deepEqual(await context(), ["BLOCK-1"], "a failed refresh keeps the block");

  answer = "ok";
  await prompt("m3");
  assert.equal(calls, 3, "the retry mark forces the next prompt to ask again");
  assert.deepEqual(await context(), ["BLOCK-3"], "the successful retry replaces the block");

  await cleanup();
});

// Issue 149: after a compaction, the NEXT prompt refreshes the block. In
// `first` mode a session replays its cached block until a refresh; the
// compaction hook marks the session stale and the next prompt drops the cache.
// The `shouldRefresh` stub is first-mode (`!hasCache || turnIndex === 0`), so
// without the stale mark this prompt replays `ses-1-1` and `calls` stays 1.
test("test_v2_adapter_next_prompt_refreshes_after_compaction", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  let revision = 1;
  let calls = 0;
  const legacy = {
    "chat.message": async (input, output) => {
      calls += 1;
      output.message.parts.push({ type: "text", text: `${input.sessionID}-${revision}` });
    },
  };
  // Non-enumerable so it is not mistaken for a hook entry.
  Object.defineProperty(legacy, "shouldRefresh", {
    value: ({ turnIndex, hasCache }) => !hasCache || turnIndex === 0,
    enumerable: false,
  });
  const cleanup = await registerV2Adapter(harness.ctx, legacy);

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
  assert.equal(calls, 1);
  assert.deepEqual(await context("ses-1"), ["ses-1-1"]);

  await harness.hooks.get("compaction")({ sessionID: "ses-1", system: [] });
  assert.deepEqual(
    await context("ses-1"),
    ["ses-1-1"],
    "issue 124: the compaction hook itself must not drop the block mid-turn"
  );

  revision += 1;
  await prompt("ses-1");
  assert.equal(calls, 2, "the prompt after compaction must refresh, not replay");
  assert.deepEqual(await context("ses-1"), ["ses-1-2"]);

  await cleanup();
});
// Issue 151: the `context`-hook instrument. One diagnostic line per `context`
// event, carrying the three fields the population reading needs
// (`session_id`, `has_block`, `turn_index`). The adapter never reads the
// debug flag: it branches on whether a logger was injected, so the flag-off
// path is `if (log)` and its silence is a plain unit assertion here.
test("test_context_hook_default_is_silence_and_an_injected_logger_is_called", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);

  // Default arm: no third argument. The mutant that matters here installs a
  // default writer whose line carries NO event name, so a substring check
  // would stay green — the stub records the calls themselves.
  const quiet = createHarness();
  const quietCleanup = await registerV2Adapter(quiet.ctx, { event: async () => {} });
  const writes = [];
  const originalWrite = process.stderr.write;
  process.stderr.write = (chunk) => {
    writes.push(String(chunk));
    return true;
  };
  try {
    const event = { sessionID: "ses-never", system: [] };
    await quiet.hooks.get("context")(event);
    assert.deepEqual(event.system, [], "an unprompted session has no block to push");
  } finally {
    process.stderr.write = originalWrite;
  }
  assert.deepEqual(writes, [], "without a logger the context hook writes nothing");
  await quietCleanup();

  // Logger arm: one call, the three fields, the state the hook found.
  const harness = createHarness();
  const calls = [];
  const cleanup = await registerV2Adapter(harness.ctx, { event: async () => {} }, {
    log: (event, fields) => calls.push({ event, fields }),
  });
  const event = { sessionID: "ses-never", system: [] };
  await harness.hooks.get("context")(event);
  assert.deepEqual(
    calls,
    [{ event: "context.hook", fields: { session_id: "ses-never", has_block: false, turn_index: 0 } }],
    "one line per context event, naming the unprompted session and the empty cache"
  );
  assert.deepEqual(event.system, []);
  await cleanup();
});

// Issue 151: `has_block:false` alone cannot separate the two populations.
// `turn_index: 0` is a session no prompt has ever touched (the real gap);
// `turn_index: 1` with an empty cache is a refresh that answered empty — issue
// 155's deliberate purge — and counting it as a witness would send the fallback
// design at a path that works.
test("test_context_hook_line_names_the_never_prompted_and_empty_refresh_populations", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const calls = [];
  const legacy = {
    "chat.message": async (input, output) => {
      if (input.sessionID === "ses-empty") {
        // The wrapper's empty SUCCESS answer: no part, `injectStatus` empty.
        Object.defineProperty(output, "injectStatus", { value: "empty", enumerable: false });
        return;
      }
      output.message.parts.push({ type: "text", text: "BLOCK" });
    },
    event: async () => {},
  };
  const cleanup = await registerV2Adapter(harness.ctx, legacy, {
    log: (event, fields) => calls.push({ event, fields }),
  });
  const context = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  // `ses-never` is never prompted: no turn counted, no cache written.
  await harness.hooks.get("prompt")({ sessionID: "ses-empty", messageID: "m1", prompt: { text: "q" } });
  await harness.hooks.get("prompt")({ sessionID: "ses-cached", messageID: "m2", prompt: { text: "q" } });
  await context("ses-never");
  await context("ses-empty");
  await context("ses-cached");

  assert.deepEqual(
    calls.map((call) => call.fields),
    [
      { session_id: "ses-never", has_block: false, turn_index: 0 },
      { session_id: "ses-empty", has_block: false, turn_index: 1 },
      { session_id: "ses-cached", has_block: true, turn_index: 1 },
    ],
    "never prompted reads 0, an empty refresh reads above 0, a cached session reads true"
  );
  await cleanup();
});

// Issue 151: the line is an observation, not a behaviour change. A cached
// session pushes the identical block with a logger attached, and exactly one
// call is made per context event.
test("test_context_hook_logs_without_changing_the_pushed_block", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const calls = [];
  const cleanup = await registerV2Adapter(
    harness.ctx,
    {
      "chat.message": async (_input, output) => {
        output.message.parts.push({ type: "text", text: "BLOCK" });
      },
      event: async () => {},
    },
    { log: (event, fields) => calls.push({ event, fields }) }
  );

  await harness.hooks.get("prompt")({ sessionID: "ses-cached", messageID: "m1", prompt: { text: "q" } });
  const event = { sessionID: "ses-cached", system: [] };
  await harness.hooks.get("context")(event);

  assert.deepEqual(
    event.system,
    [{ type: "text", text: "BLOCK" }],
    "the pushed block is byte-identical with a logger attached"
  );
  assert.deepEqual(
    calls,
    [{ event: "context.hook", fields: { session_id: "ses-cached", has_block: true, turn_index: 1 } }],
    "exactly one line per context event"
  );
  await cleanup();
});

// PRD 171 §4 (issues 151/166): a `context` event with no cached block is not
// always "no memory to inject" — a plugin re-activation (a `dist/**` write, no
// host restart) clears `contexts` mid-turn. The adapter replays the V1 inject
// with NO query text, at most once per session per activation, and only for a
// session the host still has a transcript for. Every row below reads the
// harness's default `[]` transcript unless it sets one, so an accidental
// fallback in a pre-existing row would have to beat the fail-closed floor.
test("test_v2_context_fallback_rehydrates_a_lost_block", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  harness.setTranscript([{ type: "user", text: "an earlier turn" }]);
  let calls = 0;
  const seen = [];
  const legacy = {
    "chat.message": async (input, output) => {
      calls += 1;
      // Snapshot at entry: the query-less pin is the length the WRAPPER sees,
      // before this hook appends anything.
      seen.push({ input, partsAtEntry: output.message.parts.length });
      output.message.parts.push({ type: "text", text: calls === 1 ? "block A" : "rehydrated" });
    },
    event: async () => {},
  };
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  const cleanupA = await registerV2Adapter(harness.ctx, legacy);
  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "m1", prompt: { text: "q" } });
  assert.equal(calls, 1);
  assert.deepEqual(await readContext("ses-1"), ["block A"], "activation A serves its own cache");

  // The re-activation seam: the fake `session.hook` overwrites the map entry,
  // so the second adapter starts with empty state beside a live transcript.
  const cleanupB = await registerV2Adapter(harness.ctx, legacy);
  harness.contextCalls.length = 0;

  assert.deepEqual(
    await readContext("ses-1"),
    ["rehydrated"],
    "a cache-less context event for a prompted session replays the V1 inject"
  );
  assert.equal(harness.contextCalls.length, 1, "the fallback probed the host exactly once");
  assert.deepEqual(
    await readContext("ses-1"),
    ["rehydrated"],
    "the rehydrated block is cached, so the next model step reads it from memory"
  );
  assert.equal(
    harness.contextCalls.length,
    1,
    "the second event is served from the cache, never a second replay or host read"
  );

  // The replay is query-less BY CONSTRUCTION: an empty parts array makes
  // `queryFromParts` (plugin.js:1639) undefined and `injectParams` (:1665)
  // omit `include.query`, so the engine lists the container's tags instead of
  // scoring a search. The prompt call is the in-instance positive control.
  assert.equal(seen[1].partsAtEntry, 0, "the replay carries no query text");
  assert.equal(seen[0].partsAtEntry, 1, "the prompt path carries its original part");
  assert.deepEqual(seen[1].input, { sessionID: "ses-1" }, "the replay passes the session id, nothing else");

  await cleanupB();
  await cleanupA();
});

test("test_v2_context_fallback_requires_host_evidence_of_a_turn", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  harness.ctx.session.context = async ({ sessionID }) =>
    sessionID === "ses-known" ? [{ type: "user", text: "an earlier turn" }] : [];
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (_input, output) => {
      output.message.parts.push({ type: "text", text: "BLOCK" });
    },
    event: async () => {},
  });
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  assert.deepEqual(
    await readContext("ses-known"),
    ["BLOCK"],
    "the control: the fallback is live in this very adapter"
  );
  assert.deepEqual(
    await readContext("ses-fresh"),
    [],
    "a session with no host transcript gets nothing"
  );

  await cleanup();
});

test("test_v2_context_fallback_runs_at_most_once_per_session_per_activation", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  harness.setTranscript([{ type: "user", text: "an earlier turn" }]);
  let calls = 0;
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async () => {
      calls += 1; // answers nothing: an empty engine state, not an error
    },
    event: async () => {},
  });
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  assert.deepEqual(await readContext("ses-1"), []);
  assert.deepEqual(await readContext("ses-1"), []);
  assert.deepEqual(await readContext("ses-1"), []);
  assert.equal(calls, 1, "one replay per session per activation, not one per model step");
  assert.equal(harness.contextCalls.length, 1, "and one host read behind it");

  await cleanup();
});

// The same memo, observed where its placement is visible: row 16's events are
// sequential, so moving `rehydrated.add` after the host-read await would still
// read 1 there. Two OVERLAPPING events are the distinguishing input — with the
// memo before the first await the second event sees the mark and returns; with
// it after, both park on the host read and both replay (2 reads, 2 injects).
// The reviewer's probe (.tmp/scratch/review171s4-concurrency-probe.mjs) measured
// exactly that pair; this row ships the assertion through the existing fake.
test("test_v2_context_fallback_memoizes_before_the_first_await", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  let hostReads = 0;
  let releaseHostRead;
  const hostGate = new Promise((resolve) => {
    releaseHostRead = resolve;
  });
  // A resolvable gate, never a timer: the host read parks until the test
  // releases it, and the release happens only after both hooks were invoked.
  harness.ctx.session.context = async () => {
    hostReads += 1;
    await hostGate;
    return [{ type: "user", text: "an earlier turn" }];
  };
  let calls = 0;
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (_input, output) => {
      calls += 1;
      output.message.parts.push({ type: "text", text: "BLOCK" });
    },
    event: async () => {},
  });

  // An async function body runs to its first await before returning, so `p1` is
  // already parked inside the host read when `p2` is invoked, and the gate is
  // released only on the next line, after both. No scheduling assumption.
  const first = { sessionID: "ses-1", system: [] };
  const second = { sessionID: "ses-1", system: [] };
  const p1 = harness.hooks.get("context")(first);
  const p2 = harness.hooks.get("context")(second);
  releaseHostRead();
  await Promise.all([p1, p2]);

  assert.equal(hostReads, 1, "the second overlapping event must not park on a second host read");
  assert.equal(calls, 1, "and must not run a second replay");
  assert.deepEqual(
    first.system.map((part) => part.text),
    ["BLOCK"],
    "the control: the first event's replay served the block"
  );

  await cleanup();
});

test("test_v2_context_fallback_leaves_a_withdrawn_block_withdrawn", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  harness.setTranscript([{ type: "user", text: "an earlier turn" }]);
  let mode = "empty";
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (input, output) => {
      if (mode === "empty") {
        // What the wrapper records for an empty SUCCESS (plugin.js:1466).
        Object.defineProperty(output, "injectStatus", { value: "empty", enumerable: false });
        return;
      }
      output.message.parts.push({
        type: "text",
        text: input.sessionID === "ses-withdrawn" ? "resurrected" : "CONTROL",
      });
    },
    event: async () => {},
  });
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  await harness.hooks.get("prompt")({ sessionID: "ses-withdrawn", messageID: "m1", prompt: { text: "q" } });
  mode = "answer";

  assert.deepEqual(
    await readContext("ses-withdrawn"),
    [],
    "a block a prompt withdrew in this activation is not replayed back in"
  );
  assert.deepEqual(
    await readContext("ses-control"),
    ["CONTROL"],
    "the control: the same adapter rehydrates a session with no withdrawal"
  );

  await cleanup();
});

test("test_v2_context_fallback_never_serves_another_session_block", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  harness.setTranscript([{ type: "user", text: "an earlier turn" }]);
  const answers = ["A", "B"];
  let calls = 0;
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (_input, output) => {
      output.message.parts.push({ type: "text", text: answers[calls] ?? "?" });
      calls += 1;
    },
    event: async () => {},
  });
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  assert.deepEqual(await readContext("ses-1"), ["A"], "the first session reads its own answer");
  assert.deepEqual(await readContext("ses-2"), ["B"], "the second reads its own, never the first's");

  await cleanup();
});

test("test_v2_context_fallback_does_not_overwrite_a_prompt_that_landed_while_it_awaited", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  harness.setTranscript([{ type: "user", text: "an earlier turn" }]);
  let release;
  let parkedResolve;
  const parked = new Promise((resolve) => {
    parkedResolve = resolve;
  });
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const cleanup = await registerV2Adapter(harness.ctx, {
    "chat.message": async (_input, output) => {
      // The replay is the query-less shape: empty parts at entry.
      if (output.message.parts.length === 0) {
        parkedResolve();
        await gate;
        output.message.parts.push({ type: "text", text: "fallback-block" });
        return;
      }
      output.message.parts.push({ type: "text", text: "prompt-block" });
    },
    event: async () => {},
  });
  const readContext = async (sessionID) => {
    const event = { sessionID, system: [] };
    await harness.hooks.get("context")(event);
    return event.system.map((part) => part.text);
  };

  const inFlight = harness.hooks.get("context")({ sessionID: "ses-1", system: [] });
  let parkedSeen = false;
  await Promise.race([
    parked.then(() => {
      parkedSeen = true;
    }),
    new Promise((resolve) => setTimeout(resolve, 1000)),
  ]);
  assert.equal(parkedSeen, true, "the cache-less event enters the replay (this row's premise)");
  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "m1", prompt: { text: "q" } });
  release();
  await inFlight;

  assert.deepEqual(
    await readContext("ses-1"),
    ["prompt-block"],
    "a prompt that filled the cache while the replay awaited wins"
  );

  await cleanup();
});
