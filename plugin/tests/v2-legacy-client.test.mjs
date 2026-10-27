// W5.2 `js` shard — the V2→V1 legacy client, ported from
// example/opencode-mem/src/v2/legacy-client.ts. Seven cases (blueprint D6.2):
// four upstream cases (`:387` execution alias, `:448` toast emit, `:482`
// no-RPC fallback, `:494` emit-rejects fallback), two transcript-mapping cases
// (upstream's `legacyMessage` has no direct case) and one missing-session-id
// guard. The 13 remaining upstream cases are N-A: the port's V1 behaviour
// consumes `session.messages` and `tui.showToast` only — no structured output,
// no session create/prompt/abort/delete, no provider/model pass-through.
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
