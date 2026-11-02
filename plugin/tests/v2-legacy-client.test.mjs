// W5.2 `js` shard — the V2→V1 legacy client, ported from
// example/opencode-mem/src/v2/legacy-client.ts. Twelve cases (blueprint D6.2):
// four upstream cases (`:387` execution alias, `:448` toast emit, `:482`
// no-RPC fallback, `:494` emit-rejects fallback), two transcript-mapping cases
// (upstream's `legacyMessage` has no direct case), four send-path cases (the
// `noReply`→`synthetic` mapping, `resume:false`, the unavailable-synthetic
// rejection and the text-parts-only body), one missing-session-id guard, and
// one for `session.get` (issue 114 restored the capability upstream has at
// `:265`). The 13 remaining upstream cases are N-A: the port's V1 behaviour
// consumes `session.get`, `session.messages` and `tui.showToast` only — no
// structured output, no session create/prompt/abort/delete, no provider/model
// pass-through.
//
// The module is imported dynamically in each case so a missing module reports
// once per case instead of failing the file at load.

import test from "node:test";
import assert from "node:assert/strict";

const CLIENT_MODULE = "../src/v2/legacy-client.js";

function createContext(overrides = {}) {
  return {
    location: {
      directory: "/workspace/project",
      project: { id: "project", directory: "/workspace/project" },
    },
    session: {
      get: async () => ({ location: { directory: "/workspace/project" } }),
      context: async () => [],
    },
    ...overrides,
  };
}

// The V1 client capability calls `console[method]` on the fallback path; keep
// the test output clean and make the line observable.
function captureConsole(t) {
  const lines = [];
  const originals = { info: console.info, warn: console.warn, error: console.error };
  for (const name of Object.keys(originals)) {
    console[name] = (...args) => {
      lines.push(args.join(" "));
    };
  }
  t.after(() => {
    for (const [name, original] of Object.entries(originals)) console[name] = original;
  });
  return lines;
}

test("test_v2_legacy_client_aliases_execution_events_to_session_idle", async () => {
  const { toLegacyEvent } = await import(CLIENT_MODULE);
  const typeOf = (raw) => toLegacyEvent({ type: raw, data: { sessionID: "s1" } }).type;

  assert.equal(typeOf("session.execution.succeeded"), "session.idle");
  assert.equal(typeOf("session.execution.failed"), "session.idle");
  assert.equal(typeOf("session.execution.interrupted"), "session.idle");
  assert.equal(typeOf("session.compaction.ended"), "session.compacted");
  assert.equal(typeOf("message.updated"), "message.updated");
  assert.equal(typeOf("session.execution.succeeded.1"), "session.idle", "a trailing .1 is stripped");

  // The sync-envelope form carries the event under `payload.syncEvent`.
  assert.equal(
    toLegacyEvent({ type: "sync", syncEvent: { type: "session.execution.succeeded" } }).type,
    "session.idle"
  );

  // H2: the type is merged INTO whichever property object the host supplied.
  assert.deepEqual(
    toLegacyEvent({ type: "message.updated", properties: { sessionID: "s1" } }),
    { type: "message.updated", properties: { sessionID: "s1", type: "message.updated" } },
    "the properties form keeps its fields and gains the type"
  );
  assert.deepEqual(
    toLegacyEvent({ type: "session.created", data: { session: { id: "s1" } } }),
    { type: "session.created", properties: { info: { id: "s1" }, type: "session.created" } },
    "the session-created wrapping keeps its info and gains the type"
  );
});

test("test_v2_legacy_client_emits_toast_intents_over_the_rpc_bridge", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const emitted = [];
  const toastRpc = {
    events: {
      emit: async (name, data) => {
        emitted.push({ name, data });
      },
    },
  };
  const client = createLegacyClient(createContext(), toastRpc);

  const result = await client.tui.showToast({
    body: {
      title: "Memory Captured",
      message: "Project memory saved from conversation",
      variant: "success",
      duration: 3000,
    },
  });

  assert.deepEqual(result, { data: true });
  assert.deepEqual(emitted, [
    {
      name: "toast",
      data: {
        title: "Memory Captured",
        message: "Project memory saved from conversation",
        variant: "success",
        duration: 3000,
      },
    },
  ]);
});

test("test_v2_legacy_client_falls_back_when_no_toast_rpc_is_registered", async (t) => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const lines = captureConsole(t);
  const client = createLegacyClient(createContext());

  const result = await client.tui.showToast({
    body: {
      title: "Memory Captured",
      message: "Project memory saved from conversation",
      variant: "success",
    },
  });

  assert.deepEqual(result, { data: false }, "no RPC registration answers false, never throws");
  assert.equal(lines.length, 1, "the fallback writes exactly one console line");
  assert.match(lines[0], /Memory Captured: Project memory saved from conversation/);
});

test("test_v2_legacy_client_falls_back_when_the_toast_emit_rejects", async (t) => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const lines = captureConsole(t);
  const toastRpc = {
    events: {
      emit: async () => {
        throw new Error("rpc unavailable");
      },
    },
  };
  const client = createLegacyClient(createContext(), toastRpc);

  const result = await client.tui.showToast({
    body: { title: "Auto Capture Failed", message: "timeout", variant: "error" },
  });

  assert.deepEqual(result, { data: false }, "a rejected emit degrades, never throws");
  assert.equal(lines.length, 1, "the fallback writes exactly one console line");
});

test("test_v2_legacy_client_maps_context_messages_to_the_v1_transcript_shape", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const context = {
    type: "user",
    id: "msg-u1",
    text: "fix the login bug",
    agent: "build",
  };
  const response = {
    type: "assistant",
    id: "msg-a1",
    agent: "build",
    content: [
      { type: "text", text: "done — updated the handler" },
      { type: "tool", tool: "edit", state: { input: { path: "src/h.js" } } },
    ],
  };
  const calls = [];
  const client = createLegacyClient(
    createContext({
      session: {
        context: async (input) => {
          calls.push(input);
          return [context, response];
        },
      },
    })
  );

  const expected = {
    data: [
      {
        info: { id: "msg-u1", sessionID: "ses-1", role: "user", agent: "build" },
        parts: [
          {
            id: "msg-u1-text",
            sessionID: "ses-1",
            messageID: "msg-u1",
            type: "text",
            text: "fix the login bug",
            synthetic: false,
          },
        ],
      },
      {
        info: {
          id: "msg-a1",
          sessionID: "ses-1",
          role: "assistant",
          agent: "build",
          mode: "build",
        },
        parts: [
          {
            id: "msg-a1-0",
            sessionID: "ses-1",
            messageID: "msg-a1",
            type: "text",
            text: "done — updated the handler",
          },
          {
            id: "msg-a1-1",
            sessionID: "ses-1",
            messageID: "msg-a1",
            type: "tool",
            tool: "edit",
            state: { input: { path: "src/h.js" } },
          },
        ],
      },
    ],
  };

  // H3: the wrapper calls `{ path: { id } }`; hosts may hand `{ sessionID }`.
  assert.deepEqual(await client.session.messages({ path: { id: "ses-1" } }), expected);
  assert.deepEqual(calls[0], { sessionID: "ses-1" }, "the path id is unwrapped for the V2 context");
  assert.deepEqual(await client.session.messages({ sessionID: "ses-1" }), expected);
  assert.deepEqual(calls[1], { sessionID: "ses-1" });

  // D7: exactly the two capabilities the ported V1 behaviour consumes — no
  // provider surface is created (that is issue 62's, not this lane's).
  assert.deepEqual(Object.keys(client).sort(), ["session", "tui"]);
  assert.equal(client.provider, undefined, "client.provider must stay absent");
});

test("test_v2_legacy_client_marks_synthetic_and_compaction_messages", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const messages = [
    { type: "synthetic", id: "msg-s1", text: "remembered context" },
    { type: "system", id: "msg-s2", text: "system text" },
    { type: "compaction", id: "msg-c1" },
    { type: "mystery" },
    { type: "assistant", id: "msg-a0", agent: "build" },
  ];
  const client = createLegacyClient(
    createContext({ session: { context: async () => messages } })
  );

  const { data } = await client.session.messages({ sessionID: "ses-2" });

  assert.equal(data[0].info.role, "user");
  assert.equal(data[0].parts[0].synthetic, true, "a synthetic message is a synthetic user part");
  assert.equal(data[0].parts[0].text, "remembered context");
  assert.equal(data[1].parts[0].synthetic, true, "a system message is a synthetic user part");
  assert.deepEqual(data[2], {
    info: { id: "msg-c1", sessionID: "ses-2", role: "assistant", summary: true, mode: "compaction" },
    parts: [],
  });
  assert.match(
    data[3].info.id,
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    "an unidentifiable message gets a minted uuid"
  );
  assert.equal(data[3].info.role, "assistant");
  assert.deepEqual(data[3].parts, []);
  // A host that emits an assistant message with no content array must not
  // throw here: one throw fails the whole transcript and silently drops the
  // turn's capture.
  assert.deepEqual(data[4].parts, [], "a content-less assistant message maps to no parts");
});

test("test_v2_legacy_client_routes_no_reply_and_plain_bodies", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const syntheticCalls = [];
  const promptCalls = [];
  const client = createLegacyClient(
    createContext({
      session: {
        synthetic: async (input) => {
          syntheticCalls.push(input);
          return { data: { id: "msg-synthetic" } };
        },
        prompt: async (input) => {
          promptCalls.push(input);
          return { data: { id: "msg-prompt" } };
        },
      },
    }),
    undefined
  );

  await client.session.prompt({
    path: { id: "ses-A" },
    body: { parts: [{ type: "text", text: "M" }], noReply: true },
  });
  await client.session.prompt({
    path: { id: "ses-A" },
    body: { parts: [{ type: "text", text: "U" }] },
  });

  assert.equal(syntheticCalls.length, 1, "the no-reply body is a synthetic write");
  assert.equal(promptCalls.length, 1, "a plain body is a queued prompt, not a synthetic");
  assert.deepEqual(syntheticCalls[0].sessionID, "ses-A");
  assert.equal(syntheticCalls[0].text, "M");
  assert.deepEqual(promptCalls[0], { sessionID: "ses-A", text: "U", delivery: "queue" });
});

test("test_v2_legacy_client_synthetic_does_not_wake_the_runner", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const syntheticCalls = [];
  const client = createLegacyClient(
    createContext({
      session: {
        synthetic: async (input) => {
          syntheticCalls.push(input);
          return { data: {} };
        },
      },
    })
  );

  await client.session.prompt({
    path: { id: "ses-A" },
    body: { parts: [{ type: "text", text: "M" }], noReply: true },
  });

  assert.equal(syntheticCalls.length, 1);
  // Strict, not truthiness: an omitted field is `undefined`, the mutant.
  assert.equal(syntheticCalls[0].resume, false, "the restore must not start a runner turn");
  assert.equal(syntheticCalls[0].description, "memory context");
});

test("test_v2_legacy_client_prompt_rejects_when_synthetic_is_unavailable", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const client = createLegacyClient(
    createContext({ session: { prompt: async () => ({ data: {} }) } })
  );

  await assert.rejects(
    () =>
      client.session.prompt({
        path: { id: "ses-A" },
        body: { parts: [{ type: "text", text: "M" }], noReply: true },
      }),
    /session\.synthetic unavailable/,
    "a host with no synthetic route must throw, never resolve a silent skip"
  );
});

test("test_v2_legacy_client_prompt_uses_only_text_parts", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const syntheticCalls = [];
  const client = createLegacyClient(
    createContext({
      session: {
        synthetic: async (input) => {
          syntheticCalls.push(input);
          return { data: {} };
        },
      },
    })
  );

  await client.session.prompt({
    path: { id: "ses-A" },
    body: {
      parts: [{ type: "text", text: "A" }, { type: "tool", name: "x" }, { type: "text", text: "B" }],
      noReply: true,
    },
  });

  assert.equal(syntheticCalls[0].text, "A\nB", "a tool part contributes no text and no 'undefined'");
});

test("test_v2_legacy_client_get_reads_the_session_row", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const calls = [];
  const row = {
    agent: "main",
    model: { providerID: "opencode-go", id: "deepseek-v4.1-flash" },
    parentID: "ses-parent",
  };
  const client = createLegacyClient(
    createContext({
      session: {
        get: async (input) => {
          calls.push(input);
          return row;
        },
      },
    })
  );

  // The wrapper calls the V1 shape `{ path: { id } }` (compaction.js:35); the
  // host takes `{ sessionID }`. Passing the input through is the mutant.
  assert.deepEqual(
    await client.session.get({ path: { id: "ses-A" } }),
    { data: row },
    "the row is returned inside the SDK's {data} envelope"
  );
  assert.deepEqual(calls[0], { sessionID: "ses-A" }, "the host receives {sessionID}, never {path}");
  // A host that hands the V2 shape directly must work too.
  assert.deepEqual(await client.session.get({ sessionID: "ses-A" }), { data: row });
  assert.deepEqual(calls[1], { sessionID: "ses-A" });

  await assert.rejects(
    () => client.session.get({}),
    /session id required/,
    "a session read without an id is a caller bug, not an empty row"
  );
  await assert.rejects(
    () => client.session.get({ path: {} }),
    /session id required/
  );

  // readSession degrades a throw to `null` ("not a child, proceed"), so the
  // capability must announce its own absence rather than resolve undefined.
  const noGet = createLegacyClient(createContext({ session: { context: async () => [] } }));
  await assert.rejects(
    () => noGet.session.get({ sessionID: "ses-A" }),
    /session\.get unavailable/,
    "a host with no session read must throw, never resolve a silent skip"
  );
});

test("test_v2_legacy_client_requires_a_session_id", async () => {
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const client = createLegacyClient(createContext());

  await assert.rejects(
    () => client.session.messages({}),
    /session id required/,
    "a transcript request without a session id is a caller bug, not an empty transcript"
  );
  await assert.rejects(() => client.session.messages({ path: {} }), /session id required/);
  await assert.rejects(() => client.session.messages(undefined), /session id required/);
});
