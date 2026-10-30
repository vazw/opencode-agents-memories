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
// The wave-2 surface is read off the module namespace: a missing export is then
// a RED assertion (`... is not a function`), not a module-link SyntaxError, so
// each test can go red for the behaviour it names before the code exists.
import * as toolSurface from "../src/tools.js";
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

/**
 * A recording `request` fake (the wave-2 transport seam). The envelope may be
 * a value or a `(method, params) => envelope` function, so one fake can answer
 * different methods. `{ throws: true }`-style failures use a plain throwing
 * function instead.
 */
function recordingRequest(envelope = { ok: true, result: {} }) {
  const calls = [];
  const request = async (method, params) => {
    calls.push({ method, params });
    return typeof envelope === "function" ? envelope(method, params) : envelope;
  };
  return { calls, request };
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

  // A non-normalised host directory must travel RAW (D5): capture writes the
  // raw directory as `project_id`, so a resolved wire value queries nothing.
  const rawWire = recordingSendRecall();
  const rawTool = createRecallTool({
    projectId: "/workspace/project/",
    sendRecall: rawWire.sendRecall,
  });
  await rawTool.execute({ projectId: "/workspace/project" }, { sessionID: "ses-4" });
  assert.equal(
    rawWire.calls[0].projectId,
    "/workspace/project/",
    "the host directory is sent raw; resolving it would query a different project_id"
  );
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

// ── the memory tool (issue 70 wave 2) ─────────────────────────────────
// The descriptor and the local guide first: the guide must answer before the
// scope guard, so anything about the host is a later step's subject.

test("test_memory_tool_declares_the_ten_mode_descriptor", () => {
  assert.equal(toolSurface.MEMORY_TOOL_NAME, "memory");
  assert.deepEqual(
    toolSurface.MEMORY_TOOL_MODES,
    ["add", "search", "profile", "list", "forget", "help", "migrate", "list-shards", "export", "import"],
    "the enum declares all ten upstream modes, routed or not"
  );

  const input = toolSurface.MEMORY_TOOL_INPUT;
  assert.equal(input.type, "object");
  assert.equal(input.additionalProperties, false, "the model cannot smuggle an undeclared key");
  assert.equal(
    Object.keys(input.properties).length,
    15,
    "fourteen upstream args plus the scope-discipline `projectId`"
  );
  assert.deepEqual(
    Object.keys(input.properties).sort(),
    [
      "allowLinkedSource",
      "content",
      "dryRun",
      "fromHash",
      "fromPath",
      "inputPath",
      "limit",
      "memoryId",
      "mode",
      "outputPath",
      "projectId",
      "query",
      "scope",
      "tags",
      "type",
    ],
    "the arg names are the model-facing contract"
  );
  assert.deepEqual(input.properties.mode.enum, toolSurface.MEMORY_TOOL_MODES);
  assert.deepEqual(input.properties.scope.enum, ["project", "all-projects"]);
  assert.equal("required" in input, false, "an absent mode is `help`");
  assert.ok(
    toolSurface.MEMORY_TOOL_DESCRIPTION.includes("import"),
    "the description names the wave-3 modes it declares"
  );
});

test("test_memory_tool_help_lists_the_routed_and_unavailable_modes", async () => {
  const { calls, request } = recordingRequest();
  // Host-less ON PURPOSE: `help` must answer before the scope guard, so the
  // guide proves precedence 2 before 3.
  const tool = toolSurface.createMemoryTool({ projectId: undefined, request });

  const fromEmpty = await payload(tool.execute({}, undefined));
  assert.equal(fromEmpty.success, true);
  assert.equal(fromEmpty.message, "Memory System Usage Guide");
  assert.equal(fromEmpty.tagGuidance, "Use technical keywords for search. Tags rank highest.");
  assert.deepEqual(
    fromEmpty.unavailable,
    ["export", "import", "list-shards", "migrate"],
    "the guide is honest about the four modes this build does not route"
  );
  assert.deepEqual(
    fromEmpty.commands.map((entry) => entry.command),
    ["add", "search", "profile", "list", "forget"],
    "five commands, and `help` is not listed (upstream's precedent)"
  );
  assert.deepEqual(
    fromEmpty.commands.map((entry) => entry.args),
    [
      ["content", "type?", "tags?"],
      ["query", "scope?", "limit?"],
      ["content?"],
      ["scope?", "limit?"],
      ["memoryId", "scope?"],
    ],
    "each command lists only what this build accepts"
  );

  // An absent args object is `help` too, and an explicit help stays local even
  // with a session and a host.
  assert.deepEqual(await payload(tool.execute(undefined, undefined)), fromEmpty);
  assert.deepEqual(await payload(tool.execute({ mode: "help" }, { sessionID: "ses-1" })), fromEmpty);
  assert.equal(calls.length, 0, "help never reaches the engine");
});

// ── scope and session discipline ──────────────────────────────────────
// Two wrapper-owned values never come from the model: the wire `project_id`
// (the raw host directory) and the envelope `session_id` (the call context).

test("test_memory_tool_compares_the_model_project_and_sends_the_host_directory_raw", async () => {
  const { calls, request } = recordingRequest(() => ({
    ok: true,
    result: { id: "mem-1" },
  }));
  // Host keeps its trailing slash: capture wrote that raw string, so the wire
  // value must be byte-identical to it, not a resolved path.
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p/", request });
  // The RESULT of this call is `test_memory_tool_routes_add_to_the_add_method`'s
  // subject; here only the wire value matters.
  await tool.execute({ mode: "add", content: "x", projectId: "/w/p" }, { sessionID: "ses-1" });
  assert.deepEqual(calls[0], {
    method: "add",
    params: { session_id: "ses-1", project_id: "/w/p/", content: "x" },
  });

  // Normalised comparison, raw wire: a foreign id is refused before any send.
  const rejected = await payload(
    tool.execute({ mode: "add", content: "x", projectId: "/w/other" }, { sessionID: "ses-1" })
  );
  assert.deepEqual(rejected, { success: false, error: "scope_rejected" });

  // A non-string `projectId` is a caller fault, not a scope mismatch.
  const malformed = await payload(
    tool.execute({ mode: "add", content: "x", projectId: 7 }, { sessionID: "ses-1" })
  );
  assert.deepEqual(malformed, { success: false, error: "invalid_params" });
  assert.equal(calls.length, 1, "only the accepted call ever reaches the engine");
});

test("test_memory_tool_fails_closed_without_a_host_project", async () => {
  const { calls, request } = recordingRequest();
  const tool = toolSurface.createMemoryTool({ projectId: undefined, request });

  // The host check precedes every argument check: a host with no directory is
  // not reported as a malformed call and never compares the model's id.
  assert.deepEqual(
    await payload(
      tool.execute({ mode: "add", content: "x", projectId: "/w/other" }, { sessionID: "ses-1" })
    ),
    { success: false, error: "scope_unavailable" }
  );
  assert.deepEqual(
    await payload(tool.execute({ mode: "list" }, { sessionID: "ses-1" })),
    { success: false, error: "scope_unavailable" }
  );
  assert.equal(calls.length, 0);
});

test("test_memory_tool_reads_the_session_from_the_call_context", async () => {
  const { calls, request } = recordingRequest(() => ({ ok: true, result: { count: 0, items: [] } }));
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  await tool.execute({ mode: "list" }, { sessionID: "ses-1" });
  await tool.execute({ mode: "list" }, { sessionID: "ses-2" });
  // The model can NAME a session id, but it cannot supply one: an undeclared
  // key is not copied, and the envelope field is assigned from the context.
  await tool.execute({ mode: "list", session_id: "forged", sessionId: "forged" }, { sessionID: "ses-3" });

  assert.deepEqual(
    calls.map((call) => call.params.session_id),
    ["ses-1", "ses-2", "ses-3"],
    "the envelope session id is per call, read from the tool context"
  );
  assert.deepEqual(calls[2].params, {
    session_id: "ses-3",
    project_id: "/w/p",
  });
});

test("test_memory_tool_requires_a_call_session", async () => {
  const { calls, request } = recordingRequest();
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  // No fallback to `oc-bridge-<pid>`: `add` PERSISTS session_id, so a
  // fabricated id would write a lie into the store.
  for (const toolContext of [
    undefined,
    {},
    { sessionID: undefined },
    { sessionID: "" },
    { sessionID: "   " },
  ]) {
    assert.deepEqual(
      await payload(tool.execute({ mode: "list" }, toolContext)),
      { success: false, error: "session_unavailable" }
    );
  }
  assert.equal(calls.length, 0, "a refused call sends nothing");
});

// ── the five routed wire modes ────────────────────────────────────────

test("test_memory_tool_routes_add_to_the_add_method", async () => {
  const { calls, request } = recordingRequest({ ok: true, result: { id: "mem-1" } });
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  const result = await payload(
    tool.execute({ mode: "add", content: "x", type: "fact" }, { sessionID: "ses-1" })
  );
  assert.deepEqual(calls[0], {
    method: "add",
    params: {
      session_id: "ses-1",
      project_id: "/w/p",
      content: "x",
      type: "fact",
    },
  });
  assert.equal("tags" in calls[0].params, false, "an absent optional key must not travel as undefined");
  assert.deepEqual(result, { success: true, id: "mem-1" });
});

test("test_memory_tool_routes_search_with_the_resolved_scope", async () => {
  const rows = Array.from({ length: 12 }, (_, index) => ({
    id: `m${index}`,
    content: `c${index}`,
    similarity: 0.9,
    rank: index + 1,
  }));
  const { calls, request } = recordingRequest(() => ({
    ok: true,
    result: { count: rows.length, items: rows },
  }));
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  const first = await payload(
    tool.execute({ mode: "search", query: "q" }, { sessionID: "ses-1" })
  );
  assert.deepEqual(calls[0], {
    method: "search",
    params: {
      session_id: "ses-1",
      project_id: "/w/p",
      scope: "project",
      query: "q",
    },
    // `scope` is required on the wire, so the wrapper resolves its default.
  });

  await tool.execute(
    { mode: "search", query: "q", scope: "all-projects", limit: 25 },
    { sessionID: "ses-1" }
  );
  assert.deepEqual(calls[1].params, {
    session_id: "ses-1",
    project_id: "/w/p",
    scope: "all-projects",
    query: "q",
    limit: 25,
  });

  // No client-side `slice(0, limit || 10)` (upstream's): the wire applied the
  // limit and assigned `rank`, so a second slice would fork `count`.
  assert.deepEqual(first, { success: true, count: 12, items: rows });
  assert.equal(first.items.length, 12);
});

test("test_memory_tool_rejects_an_unknown_mode", async () => {
  const { calls, request } = recordingRequest();
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  for (const mode of ["bogus", null, 7, "", "MEMORY"]) {
    assert.deepEqual(
      await payload(tool.execute({ mode }, { sessionID: "ses-1" })),
      { success: false, error: "unknown_mode" }
    );
  }
  assert.equal(calls.length, 0, "an unknown mode never reaches the engine");
});

test("test_memory_tool_routes_list_to_the_list_method", async () => {
  const { calls, request } = recordingRequest(() => ({
    ok: true,
    result: { count: 0, items: [] },
  }));
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  const empty = await payload(tool.execute({ mode: "list" }, { sessionID: "ses-1" }));
  assert.deepEqual(calls[0], {
    method: "list",
    params: { session_id: "ses-1", project_id: "/w/p" },
  });
  assert.equal("limit" in calls[0].params, false, "an absent limit is not sent as undefined");
  assert.deepEqual(empty, { success: true, count: 0, items: [] });

  await tool.execute({ mode: "list", limit: 5 }, { sessionID: "ses-1" });
  assert.equal(calls[1].params.limit, 5);
});

test("test_memory_tool_routes_forget_with_the_scope_rule", async () => {
  const { calls, request } = recordingRequest({ ok: true, result: { deleted: 0 } });
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  // An explicit `all-projects` travels; `deleted:0` is a SUCCESS ("not found").
  const notFound = await payload(
    tool.execute({ mode: "forget", memoryId: "m", scope: "all-projects" }, { sessionID: "ses-1" })
  );
  assert.deepEqual(calls[0], {
    method: "forget",
    params: {
      session_id: "ses-1",
      project_id: "/w/p",
      memoryId: "m",
      scope: "all-projects",
    },
  });
  assert.deepEqual(notFound, { success: true, deleted: 0 });

  // Absent scope stays absent: the wire's own default is `project`, and a
  // wrapper-sent "project" would be a second owner of that rule.
  await tool.execute({ mode: "forget", memoryId: "m" }, { sessionID: "ses-1" });
  assert.equal("scope" in calls[1].params, false);
});

test("test_memory_tool_reads_and_writes_the_profile_by_the_presence_of_content", async () => {
  const { calls, request } = recordingRequest((method, params) =>
    "content" in params
      ? { ok: true, result: { saved: true } }
      : { ok: true, result: { profile: null } }
  );
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  const read = await payload(tool.execute({ mode: "profile" }, { sessionID: "ses-1" }));
  assert.equal("content" in calls[0].params, false, "an absent content is a read");
  assert.deepEqual(read, { success: true, profile: null });

  // Presence, not truthiness: `""` is a WRITE the wire refuses as `blank`; a
  // truthiness test would silently turn it into a read.
  const write = await payload(
    tool.execute({ mode: "profile", content: "" }, { sessionID: "ses-1" })
  );
  assert.deepEqual(calls[1].params, {
    session_id: "ses-1",
    project_id: "/w/p",
    content: "",
  });
  assert.deepEqual(write, { success: true, saved: true });
});

test("test_memory_tool_rejects_query_in_profile_mode", async () => {
  const { calls, request } = recordingRequest({ ok: true, result: { profile: null } });
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  // `query` is not a profile parameter at all (upstream parity): it is refused
  // rather than silently dropped into a read.
  assert.deepEqual(
    await payload(tool.execute({ mode: "profile", query: "x" }, { sessionID: "ses-1" })),
    { success: false, error: "invalid_params" }
  );
  assert.deepEqual(
    await payload(tool.execute({ mode: "profile", query: "" }, { sessionID: "ses-1" })),
    { success: false, error: "invalid_params" }
  );
  assert.equal(calls.length, 0);
});

test("test_memory_tool_returns_the_capability_no_op_for_the_four_portability_modes", async () => {
  const { calls, request } = recordingRequest();
  const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });

  // (i) Every argument supplied and `projectId` pinned to the host directory:
  // the mode is DECLARED and never routed, even though this build's hello
  // advertises all four keys. A foreign id here would make this sub-case (ii).
  const supplied = {
    migrate: {
      fromPath: "/old",
      fromHash: "0123456789abcdef",
      dryRun: true,
      allowLinkedSource: true,
    },
    "list-shards": {},
    export: { outputPath: "/tmp/out.json" },
    import: { inputPath: "/tmp/in.json", dryRun: true },
  };
  for (const mode of toolSurface.MEMORY_TOOL_UNAVAILABLE_MODES) {
    assert.deepEqual(
      await payload(
        tool.execute({ mode, projectId: "/w/p", ...supplied[mode] }, { sessionID: "ses-1" })
      ),
      { success: false, error: "capability_unavailable", capability: mode },
      `${mode} must answer the structured no-op`
    );
  }
  assert.equal(calls.length, 0, "no portability mode ever sends a request");

  // (ii) sub-case (a): a foreign id is a SCOPE refusal before the boundary —
  // precedence 5 before 6, pinned by input.
  assert.deepEqual(
    await payload(tool.execute({ mode: "migrate", projectId: "/w/other" }, { sessionID: "ses-1" })),
    { success: false, error: "scope_rejected" }
  );
  assert.equal(calls.length, 0);

  // (iii) sub-case (b): an absent call context and a blank id are the
  // capability no-op — precedence 6 before 7, pinned by input.
  assert.deepEqual(
    await payload(tool.execute({ mode: "migrate", fromPath: "/old" }, undefined)),
    { success: false, error: "capability_unavailable", capability: "migrate" }
  );
  assert.deepEqual(
    await payload(tool.execute({ mode: "migrate", fromPath: "/old" }, { sessionID: "" })),
    { success: false, error: "capability_unavailable", capability: "migrate" }
  );
  assert.equal(calls.length, 0);
});

test("test_memory_tool_maps_request_failures_to_values", async () => {
  // The wire's own §4 refusal becomes the tool's `error`, verbatim: no local
  // whitelist, so a new reason needs no wrapper change.
  for (const error of ["private", "learning_locked"]) {
    const { calls, request } = recordingRequest({ ok: false, error });
    const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });
    assert.deepEqual(
      await payload(tool.execute({ mode: "add", content: "x" }, { sessionID: "ses-1" })),
      { success: false, error }
    );
    assert.equal(calls.length, 1, "a wire refusal means the request did run");
  }

  // A missing capability keeps its third key.
  const capability = recordingRequest({
    ok: false,
    error: "capability_unavailable",
    capability: "add",
  });
  assert.deepEqual(
    await payload(
      toolSurface
        .createMemoryTool({ projectId: "/w/p", request: capability.request })
        .execute({ mode: "add", content: "x" }, { sessionID: "ses-1" })
    ),
    { success: false, error: "capability_unavailable", capability: "add" }
  );

  // A throwing seam (transport failure, timeout) is not a token.
  const { calls, request } = recordingRequest(async () => {
    throw Object.assign(new Error("huge request timed out"), { code: -32001 });
  });
  assert.deepEqual(
    await payload(
      toolSurface
        .createMemoryTool({ projectId: "/w/p", request })
        .execute({ mode: "search", query: "q" }, { sessionID: "ses-1" })
    ),
    { success: false, error: "engine_unavailable" }
  );
  assert.equal(calls.length, 1);
});

test("test_memory_tool_maps_a_malformed_result_to_engine_unavailable", async () => {
  // `{}` for all five routed modes: every shape check must bite.
  const emptyPage = [
    { mode: "add", args: { content: "x" } },
    { mode: "search", args: { query: "q" } },
    { mode: "list", args: {} },
    { mode: "forget", args: { memoryId: "m" } },
    { mode: "profile", args: {} },
  ];
  for (const { mode, args } of emptyPage) {
    const { calls, request } = recordingRequest({ ok: true, result: {} });
    const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });
    assert.deepEqual(
      await payload(tool.execute({ ...args, mode }, { sessionID: "ses-1" })),
      { success: false, error: "engine_unavailable" },
      `${mode} must reject an empty result`
    );
    assert.equal(calls.length, 1);
  }

  const malformed = [
    ["add", { content: "x" }, { id: "" }],
    ["add", { content: "x" }, { id: 7 }],
    ["search", { query: "q" }, { count: 1 }],
    ["search", { query: "q" }, { items: [] }],
    ["search", { query: "q" }, { count: 1, items: "nope" }],
    ["list", {}, { items: [] }],
    ["forget", { memoryId: "m" }, { deleted: "1" }],
    ["forget", { memoryId: "m" }, { deleted: 1.5 }],
    ["profile", {}, { profile_data: {} }],
  ];
  for (const [mode, args, result] of malformed) {
    const { request } = recordingRequest({ ok: true, result });
    const tool = toolSurface.createMemoryTool({ projectId: "/w/p", request });
    assert.deepEqual(
      await payload(tool.execute({ ...args, mode }, { sessionID: "ses-1" })),
      { success: false, error: "engine_unavailable" },
      `${mode} with ${JSON.stringify(result)} must be malformed`
    );
  }
});

test("test_tool_request_never_sends_without_the_capability", async () => {
  const sends = [];
  const send = async (method, params) => {
    sends.push({ method, params });
    return { id: "mem-1" };
  };
  const request = toolSurface.createToolRequest({
    ensureStarted: async () => true,
    capability: () => false,
    send,
  });

  assert.deepEqual(await request("add", { content: "x" }), {
    ok: false,
    error: "capability_unavailable",
    capability: "add",
  });
  assert.equal(sends.length, 0, "a false capability must not send");

  // `=== true` is the gate, not truthiness: a non-boolean "yes" is not an
  // advertised capability, and an inverted check would send here.
  const truthy = toolSurface.createToolRequest({
    ensureStarted: async () => true,
    capability: () => "yes",
    send,
  });
  assert.deepEqual(await truthy("add", { content: "x" }), {
    ok: false,
    error: "capability_unavailable",
    capability: "add",
  });
  assert.equal(sends.length, 0);

  // The positive control: with the capability, the same seam sends once.
  const allowed = toolSurface.createToolRequest({
    ensureStarted: async () => true,
    capability: (method) => method === "add",
    send,
  });
  assert.deepEqual(await allowed("add", { content: "x" }), {
    ok: true,
    result: { id: "mem-1" },
  });
  assert.deepEqual(sends, [{ method: "add", params: { content: "x" } }]);
});

test("test_tool_request_gates_on_the_handshake_before_the_capability", async () => {
  const capabilityCalls = [];
  const sends = [];
  const request = toolSurface.createToolRequest({
    ensureStarted: async () => false,
    capability: (method) => {
      capabilityCalls.push(method);
      return true;
    },
    send: async (method) => {
      sends.push(method);
      return {};
    },
  });

  assert.deepEqual(await request("search", { query: "q" }), {
    ok: false,
    error: "engine_unavailable",
  });
  assert.equal(capabilityCalls.length, 0, "the capability is unreadable before the handshake");
  assert.equal(sends.length, 0);
});

test("test_tool_request_surfaces_the_wire_reason", async () => {
  const seam = (send) =>
    toolSurface.createToolRequest({
      ensureStarted: async () => true,
      capability: () => true,
      send,
    });

  // `handleWireLine` attaches `{code, data}`; only a string `data.reason` is a
  // token. A code-only error (today's revert point) is not.
  const withReason = seam(async () => {
    throw Object.assign(new Error("Invalid params"), {
      code: -32602,
      data: { reason: "blank" },
    });
  });
  assert.deepEqual(await withReason("profile", {}), { ok: false, error: "blank" });

  const withoutData = seam(async () => {
    throw Object.assign(new Error("transport failed"), { code: -32001 });
  });
  assert.deepEqual(await withoutData("profile", {}), {
    ok: false,
    error: "engine_unavailable",
  });

  const nonStringReason = seam(async () => {
    throw Object.assign(new Error("Invalid params"), { data: { reason: 7 } });
  });
  assert.deepEqual(await nonStringReason("profile", {}), {
    ok: false,
    error: "engine_unavailable",
  });

  // A result that is not an object is a malformed answer, never a payload.
  assert.deepEqual(await seam(async () => null)("list", {}), {
    ok: false,
    error: "engine_unavailable",
  });
  assert.deepEqual(await seam(async () => [])("list", {}), {
    ok: false,
    error: "engine_unavailable",
  });

  // `ensureStarted` rejecting is a transport failure, not a throw out of the seam.
  assert.deepEqual(
    await toolSurface.createToolRequest({
      ensureStarted: async () => {
        throw new Error("spawn failed");
      },
      capability: () => true,
      send: async () => ({}),
    })("list", {}),
    { ok: false, error: "engine_unavailable" }
  );
});

test("test_setup_registers_both_agent_tools", async () => {
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
  assert.deepEqual(
    added.map((tool) => tool.name),
    ["recall_project_sessions", "memory"],
    "recall registers first, then the ten-mode memory toolkit"
  );
  // The host derives the callable name from `options.namespace` when it is
  // set (`effectiveName`): no namespace keeps the bare name callable.
  assert.equal(added[0].options?.namespace, undefined);
  assert.equal(added[1].options?.namespace, undefined);
  assert.equal(typeof added[0].execute, "function");
  assert.equal(typeof added[1].execute, "function");
  assert.deepEqual(added[0].input, fixture.input);
  assert.deepEqual(added[1].input, toolSurface.MEMORY_TOOL_INPUT);
  await cleanup();
  assert.deepEqual(lifecycle, ["tool", "adapter"], "tool cleanup precedes the adapter cleanup");
  await cleanup();
  assert.deepEqual(lifecycle, ["tool", "adapter"], "the cleanup is idempotent");
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
    assert.deepEqual(record.names, ["recall_project_sessions", "memory"]);
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

test(
  "test_memory_tool_call_reaches_the_engine_as_the_routed_methods",
  { timeout: 60000 },
  async (t) => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "memory-tool-e2e-"));
    const pkgDir = stagePackageTree(tmp);
    const requestLogPath = path.join(tmp, "wire.log");
    const manifestPath = path.join(tmp, "stub.json");
    // The stub ADVERTISES the four portability keys and OMITS `forget`, so one
    // run proves both no-op causes: the mode is unrouted, the key is missing.
    stageStubEngine(pkgDir, {
      requestLogPath,
      manifestPath,
      capabilities: {
        add: true,
        compaction: true,
        export: true,
        fire_and_forget_capture: true,
        forget: undefined,
        import: true,
        list: true,
        "list-shards": true,
        migrate: true,
        profile: true,
        search: true,
        skills: true,
        standalone: true,
        web: true,
      },
    });
    t.after(() => assertReaped(manifestPath));

    const calls = [
      { args: {} }, // help
      { args: { mode: "add", content: "the deploy key lives in the vault" } },
      { args: { mode: "search", query: "deploy key" } },
      { args: { mode: "list", limit: 5 } },
      { args: { mode: "profile" } }, // read
      { args: { mode: "profile", content: "Prefer short answers" } }, // write
      { args: { mode: "forget", memoryId: "mem-1" } }, // capability omitted
      { args: { mode: "migrate", fromPath: "/old" } }, // declared, never routed
    ].map((call) => ({ args: call.args, toolContext: { sessionID: "ses-tool" } }));
    const fixturePath = path.join(tmp, "fixture.json");
    fs.writeFileSync(fixturePath, JSON.stringify({ directory: "/workspace/project", calls }));
    const donePath = path.join(tmp, "done.json");
    const driverPath = path.join(tmp, "driver.mjs");
    fs.writeFileSync(driverPath, DRIVER_SOURCE);
    const run = await runDriver(driverPath, [pkgDir, fixturePath], {
      ...process.env,
      DRIVER_MODE: "v2-memory-tool",
      DRIVER_DONE_PATH: donePath,
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      STUB_LOG: requestLogPath,
    });
    assert.equal(
      run.code,
      0,
      `the v2-memory-tool driver failed (signal=${String(run.signal)}): stderr=${run.stderr}`
    );
    assert.equal(run.timedOut, false);

    const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
    assert.deepEqual(record.names, ["recall_project_sessions", "memory"]);
    // Call 1: `help` on a host that has a directory and a session — still local.
    assert.equal(record.payloads[0].success, true);
    assert.equal(record.payloads[0].commands.length, 5);
    assert.deepEqual(record.payloads[0].unavailable, ["export", "import", "list-shards", "migrate"]);
    assert.deepEqual(record.payloads[1], { success: true, id: "mem-stub-1" });
    assert.deepEqual(record.payloads[2], {
      success: true,
      count: 1,
      items: [{ id: "mem-stub-1", content: "stub row", similarity: 0.9, rank: 1 }],
    });
    assert.deepEqual(record.payloads[3], { success: true, count: 0, items: [] });
    assert.deepEqual(record.payloads[4], { success: true, profile: null });
    assert.deepEqual(record.payloads[5], { success: true, saved: true });
    // The two structured no-ops, for the two different causes.
    assert.deepEqual(record.payloads[6], {
      success: false,
      error: "capability_unavailable",
      capability: "forget",
    });
    assert.deepEqual(record.payloads[7], {
      success: false,
      error: "capability_unavailable",
      capability: "migrate",
    });

    const TOOL_METHODS = [
      "add",
      "search",
      "forget",
      "list",
      "profile",
      "list-shards",
      "migrate",
      "export",
      "import",
    ];
    const toolLines = record.wire.filter((message) => message && TOOL_METHODS.includes(message.method));
    assert.deepEqual(
      toolLines.map((message) => message.method),
      ["add", "search", "list", "profile", "profile"],
      "help/forget/migrate sent nothing; the five routed calls did"
    );
    for (const line of toolLines) {
      assert.equal(line.params.session_id, "ses-tool");
      assert.equal(
        line.params.project_id,
        "/workspace/project",
        "the host directory travels raw; the model's projectId never does"
      );
    }
    // What the stub actually advertised: the hello answer is not on the wire
    // log (it records requests), so the stub writes its own capability record.
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    assert.equal(
      manifest.capabilities.migrate,
      true,
      "the stub advertised the portability key the tool must not use"
    );
    assert.equal(manifest.capabilities.forget, undefined, "the stub omitted the forget key");
  }
);

test(
  "test_memory_tool_surfaces_the_wire_reason_over_the_bridge",
  { timeout: 60000 },
  async (t) => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "memory-tool-e2e-"));
    const pkgDir = stagePackageTree(tmp);
    const requestLogPath = path.join(tmp, "wire.log");
    const manifestPath = path.join(tmp, "stub.json");
    stageStubEngine(pkgDir, {
      requestLogPath,
      manifestPath,
      capabilities: {
        compaction: true,
        fire_and_forget_capture: true,
        profile: true,
        skills: true,
        standalone: true,
        web: true,
      },
      wireErrors: {
        profile: {
          code: -32603,
          message: "profile learning lock is held",
          data: { reason: "learning_locked", retryable: true },
        },
      },
    });
    t.after(() => assertReaped(manifestPath));

    const fixturePath = path.join(tmp, "fixture.json");
    fs.writeFileSync(
      fixturePath,
      JSON.stringify({
        directory: "/workspace/project",
        calls: [
          {
            args: { mode: "profile", content: "Prefer short answers" },
            toolContext: { sessionID: "ses-tool" },
          },
        ],
      })
    );
    const donePath = path.join(tmp, "done.json");
    const driverPath = path.join(tmp, "driver.mjs");
    fs.writeFileSync(driverPath, DRIVER_SOURCE);
    const run = await runDriver(driverPath, [pkgDir, fixturePath], {
      ...process.env,
      DRIVER_MODE: "v2-memory-tool",
      DRIVER_DONE_PATH: donePath,
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      STUB_LOG: requestLogPath,
    });
    assert.equal(
      run.code,
      0,
      `the v2-memory-tool driver failed (signal=${String(run.signal)}): stderr=${run.stderr}`
    );
    assert.equal(run.timedOut, false);

    const record = JSON.parse(fs.readFileSync(donePath, "utf8"));
    assert.deepEqual(record.payloads[0], { success: false, error: "learning_locked" });
    const profileLines = record.wire.filter((message) => message && message.method === "profile");
    assert.equal(profileLines.length, 1, "exactly one profile line was sent");
    assert.deepEqual(profileLines[0].params, {
      session_id: "ses-tool",
      project_id: "/workspace/project",
      content: "Prefer short answers",
    });
  }
);
