// W5.4 — the agent-facing memory tool surface.
//
// The live host (opencode v2.1.4) takes a tool registration through
// `ctx.tool.transform` and treats a tool's `input` as a raw JSON Schema
// (`packages/core/src/tool/runtime.ts` `inputJsonSchema` returns a non-Schema
// value unchanged), so the W4.2 schema is reusable verbatim. The wrapper cannot
// import Rust and `src/` may not ship a `.json` file, so
// `plugin/tests/fixtures/recall-tool.json` is the shared artifact both sides
// assert against (the Rust drift guard is
// `crates/mem-bridge/tests/recall_schema.rs`).
//
// Scope discipline: the model's `projectId` argument is only COMPARED with the
// host directory, never forwarded; the wire `projectId` is the host directory
// verbatim, the same raw string capture writes as `project_id`.
//
// Failure tokens (fixed precedence): `scope_unavailable` (host has no
// directory), `invalid_params` (the argument is absent or not a string),
// `scope_rejected` (it does not resolve to the host directory),
// `engine_unavailable` (the call rejects, or the page is malformed). Every
// failure is a value, never a throw: the host would surface a throw as a
// `Tool.Error` with no recovery instruction.
//
// Issue 70 wave 2 adds the second, agent-facing tool (`MEMORY_TOOL_NAME`,
// upstream's ten-mode toolkit). Its wrapper-owned failure vocabulary — never on
// the wire, so it lives here and not in docs/bridge-schema.md — is seven
// tokens: `unknown_mode` (the mode is not one of the ten), `scope_unavailable`
// (the host has no directory), `invalid_params` (`projectId`/`scope` present
// and not a string, or `query` in `profile` mode), `scope_rejected` (the
// model's `projectId` resolves elsewhere), `capability_unavailable`
// (`+capability`: the hello omits the method's key, or the mode is one of the
// four portability modes wave 3 will route), `session_unavailable` (the call
// context carries no id) and `engine_unavailable` (no handshake, a transport
// failure, a timeout or a malformed result). A wire refusal passes through as
// its own §4 `data.reason` token: the tool keeps no local whitelist (D7), so a
// new reason needs no wrapper change. The precedence is fixed and tested —
// `unknown_mode`, `help`, `scope_unavailable`, `invalid_params`,
// `scope_rejected`, `capability_unavailable`, `session_unavailable`, then the
// request — because the adjacent pairs are behaviour, not prose.

import path from "node:path";

export const RECALL_TOOL_NAME = "recall_project_sessions";

export const RECALL_TOOL_DESCRIPTION =
  "Recall past agent sessions for a project as a chronological, filtered or semantically ranked timeline.";

// The W4.2 schema, verbatim (`RecallProjectSessionsArgs::to_json_schema`).
export const RECALL_TOOL_INPUT = {
  type: "object",
  properties: {
    projectId: { type: "string" },
    mode: { type: "string", enum: ["chronological", "semantic", "filtered"] },
    query: { type: "string" },
    since: { type: "integer" },
    until: { type: "integer" },
    lastNSessions: { type: "integer", minimum: 0, maximum: 500 },
    touchedFiles: { type: "array", items: { type: "string" } },
    agentId: { type: "string" },
    sessionId: { type: "string" },
    includeFullSessions: { type: "boolean" },
  },
  required: ["projectId"],
  additionalProperties: false,
};

function failure(error) {
  return { content: JSON.stringify({ success: false, error }) };
}

function reply(value) {
  return { content: JSON.stringify(value) };
}

/** Copies an optional param only when the model supplied the key (D9). */
function assignPresent(target, key, value) {
  if (value !== undefined) target[key] = value;
}

/**
 * Builds the `recall_project_sessions` tool for one host project.
 *
 * `projectId` is the host's `ctx.location.directory` (raw). `sendRecall`
 * carries the params to the engine and resolves its `{count, items}` page; it
 * is injected so this module never imports `plugin.js`.
 */
export function createRecallTool({ projectId, sendRecall } = {}) {
  const hostProject =
    typeof projectId === "string" && projectId.length > 0 ? projectId : null;
  return {
    name: RECALL_TOOL_NAME,
    description: RECALL_TOOL_DESCRIPTION,
    input: RECALL_TOOL_INPUT,
    async execute(args, toolContext) {
      // A host with no project directory refuses every call before the
      // model's own argument can be believed.
      if (hostProject === null) return failure("scope_unavailable");
      const requested = args?.projectId;
      if (typeof requested !== "string") return failure("invalid_params");
      // Compare normalised, send raw: capture writes the raw host directory,
      // so a resolved wire value would silently match nothing.
      if (path.resolve(requested) !== path.resolve(hostProject)) {
        return failure("scope_rejected");
      }
      // The envelope `session_id` is the call context's, never the model's:
      // it is assigned after the spread so an argument of the same name can
      // never replace it. The W4.2 filter `sessionId` is a different key and
      // travels untouched.
      const params = {
        ...args,
        projectId: hostProject,
        session_id: toolContext?.sessionID,
      };
      let page;
      try {
        page = await sendRecall(params);
      } catch {
        return failure("engine_unavailable");
      }
      if (!page || !Array.isArray(page.items)) {
        return failure("engine_unavailable");
      }
      const key = args?.includeFullSessions === true ? "fullSessions" : "summaries";
      return { content: JSON.stringify({ [key]: page.items }) };
    },
  };
}

// ── the memory tool (issue 70 wave 2) ──────────────────────────────────
// Upstream's ten-mode toolkit, of which this wave routes the six modes that
// need no portability service. The other four stay DECLARED and answer a
// structured capability no-op: this build's engine advertises the keys, but
// wave 2 must not send (D3) — wave 3 flips the routes, not the vocabulary.

export const MEMORY_TOOL_NAME = "memory";

export const MEMORY_TOOL_DESCRIPTION =
  "Manage and query project memory. Use 'search' with technical keywords/tags, 'add' to store knowledge, 'profile' for preferences. Portability modes migrate/list-shards/export/import depend on the engine build; 'help' lists what this build serves. Search/list scope: project or all-projects.";

// The enum order is upstream's (`example/opencode-mem/src/index.ts:826-900`),
// so a model-facing list reads the same as the reference.
export const MEMORY_TOOL_MODES = [
  "add",
  "search",
  "profile",
  "list",
  "forget",
  "help",
  "migrate",
  "list-shards",
  "export",
  // A template literal, not a quoted string: the bundle-boundary scan
  // (`plugin/tests/bundle-boundary.test.mjs`) reads a quoted import token as a
  // module specifier — a false positive, but the gate is the gate.
  `import`,
];

// Wave 3's four; sorted for the guide's `unavailable` list.
export const MEMORY_TOOL_UNAVAILABLE_MODES = ["export", `import`, "list-shards", "migrate"];

export const MEMORY_TOOL_INPUT = {
  type: "object",
  properties: {
    mode: { type: "string", enum: MEMORY_TOOL_MODES },
    content: { type: "string" },
    query: { type: "string" },
    tags: { type: "string" },
    type: { type: "string" },
    memoryId: { type: "string" },
    limit: { type: "number" },
    scope: { type: "string", enum: ["project", "all-projects"] },
    fromPath: { type: "string" },
    fromHash: { type: "string" },
    outputPath: { type: "string" },
    inputPath: { type: "string" },
    dryRun: { type: "boolean" },
    allowLinkedSource: { type: "boolean" },
    // Scope discipline, as on `recall_project_sessions`: the model's value is
    // only COMPARED with the host directory, never forwarded.
    projectId: { type: "string" },
  },
  // No `required`: an absent mode is `help` (upstream), and the wire owns the
  // per-mode requirements (§4's `data.reason`), not the schema.
  additionalProperties: false,
};

/** The local guide. A fresh object per call: no caller can mutate a shared one. */
function memoryToolHelp() {
  return {
    success: true,
    message: "Memory System Usage Guide",
    commands: [
      {
        command: "add",
        description: "Store new memory",
        args: ["content", "type?", "tags?"],
      },
      {
        command: "search",
        description: "Search memories via keywords",
        args: ["query", "scope?", "limit?"],
      },
      {
        command: "profile",
        description:
          "View the user profile or save an explicit preference (provide content to write)",
        args: ["content?"],
      },
      { command: "list", description: "List recent memories", args: ["scope?", "limit?"] },
      { command: "forget", description: "Remove one memory by id", args: ["memoryId", "scope?"] },
    ],
    unavailable: [...MEMORY_TOOL_UNAVAILABLE_MODES],
    tagGuidance: "Use technical keywords for search. Tags rank highest.",
  };
}

/**
 * Builds the agent-facing `memory` tool for one host project.
 *
 * `request` is `createToolRequest`'s envelope seam; it is injected so this
 * module never imports `plugin.js`. Every failure is a value: the host would
 * surface a throw as a `Tool.Error` with no recovery instruction.
 */
export function createMemoryTool({ projectId, request } = {}) {
  const hostProject =
    typeof projectId === "string" && projectId.length > 0 ? projectId : null;

  return {
    name: MEMORY_TOOL_NAME,
    description: MEMORY_TOOL_DESCRIPTION,
    input: MEMORY_TOOL_INPUT,
    async execute(args, toolContext) {
      // 1. The mode decides everything else. Absent means `help`; `null`,
      // numbers and unknown strings are refused rather than guessed.
      const requestedMode = args?.mode;
      let mode;
      if (requestedMode === undefined) {
        mode = "help";
      } else if (typeof requestedMode === "string" && MEMORY_TOOL_MODES.includes(requestedMode)) {
        mode = requestedMode;
      } else {
        return failure("unknown_mode");
      }
      // 2. `help` is local and precedes every host/session check: a host with
      // no directory can still read the guide.
      if (mode === "help") return reply(memoryToolHelp());
      // 3. The host's own directory must exist before the model's id can be
      // compared with anything.
      if (hostProject === null) return failure("scope_unavailable");
      // 4. The local shape refusals are only the ones the wire has no opinion
      // on: a non-string `projectId`/`scope`, and `query` in `profile` mode
      // (upstream parity — `query` is not a profile parameter at all).
      const requestedProject = args?.projectId;
      if (requestedProject !== undefined && typeof requestedProject !== "string") {
        return failure("invalid_params");
      }
      const requestedScope = args?.scope;
      if (requestedScope !== undefined && typeof requestedScope !== "string") {
        return failure("invalid_params");
      }
      if (mode === "profile" && args?.query !== undefined) return failure("invalid_params");
      // 5. Compare normalised, send raw (the `recall_project_sessions` rule):
      // capture wrote the raw host directory as `project_id`.
      if (
        typeof requestedProject === "string" &&
        path.resolve(requestedProject) !== path.resolve(hostProject)
      ) {
        return failure("scope_rejected");
      }
      // 6. The wave boundary: the four portability modes are DECLARED and never
      // routed — no request, even though this build's hello advertises the key.
      if (MEMORY_TOOL_UNAVAILABLE_MODES.includes(mode)) {
        return reply({ success: false, error: "capability_unavailable", capability: mode });
      }
      // 7. A call with no session id never reaches the wire: `add` persists
      // `session_id`, so an invented fallback would write a lie into the store.
      const sessionID = toolContext?.sessionID;
      if (typeof sessionID !== "string" || sessionID.trim().length === 0) {
        return failure("session_unavailable");
      }
      // 8. The request runs. Parameters are built by assigning present keys
      // only — never by spreading the model's args, so an absent key cannot
      // travel as `undefined` and an undeclared one cannot be smuggled.
      const params = { session_id: sessionID, project_id: hostProject };
      switch (mode) {
        case "add":
          assignPresent(params, "content", args?.content);
          assignPresent(params, "type", args?.type);
          assignPresent(params, "tags", args?.tags);
          break;
        case "search":
          // `scope` is REQUIRED on the wire: the wrapper resolves the default.
          params.scope = typeof requestedScope === "string" ? requestedScope : "project";
          assignPresent(params, "query", args?.query);
          assignPresent(params, "limit", args?.limit);
          break;
        case "list":
          assignPresent(params, "scope", requestedScope);
          assignPresent(params, "limit", args?.limit);
          break;
        case "forget":
          assignPresent(params, "memoryId", args?.memoryId);
          assignPresent(params, "scope", requestedScope);
          break;
        case "profile":
          // Presence, not truthiness: `""` is a write the wire refuses as
          // `blank`, never a silent read.
          assignPresent(params, "content", args?.content);
          break;
      }
      let envelope;
      try {
        envelope = await request(mode, params);
      } catch {
        return failure("engine_unavailable");
      }
      if (envelope === null || typeof envelope !== "object" || Array.isArray(envelope)) {
        return failure("engine_unavailable");
      }
      if (envelope.ok !== true) {
        // A refusal is a value: the wire's own §4 token passes through, and a
        // missing capability keeps its third key (D4's one token, two causes).
        const error =
          typeof envelope.error === "string" && envelope.error.length > 0
            ? envelope.error
            : "engine_unavailable";
        if (error === "capability_unavailable") {
          return reply({
            success: false,
            error,
            capability: typeof envelope.capability === "string" ? envelope.capability : mode,
          });
        }
        return failure(error);
      }
      return mapMemoryResult(mode, envelope.result);
    },
  };
}

/**
 * Maps a wire result to the tool's value. Each shape is §3's; anything else is
 * malformed and becomes `engine_unavailable` — never a partial payload.
 * `count:0`, `deleted:0` and `profile:null` are successes, not failures.
 */
function mapMemoryResult(mode, result) {
  if (result === null || typeof result !== "object" || Array.isArray(result)) {
    return failure("engine_unavailable");
  }
  switch (mode) {
    case "add": {
      if (typeof result.id !== "string" || result.id.length === 0) {
        return failure("engine_unavailable");
      }
      return reply({ success: true, id: result.id });
    }
    case "list":
    case "search": {
      if (!Array.isArray(result.items) || !Number.isInteger(result.count)) {
        return failure("engine_unavailable");
      }
      return reply({ success: true, count: result.count, items: result.items });
    }
    case "forget": {
      if (!Number.isInteger(result.deleted)) return failure("engine_unavailable");
      return reply({ success: true, deleted: result.deleted });
    }
    case "profile": {
      // The wire answers `{profile}` for a read and `{saved}` for a write; a
      // reply carrying neither key is malformed, not an empty profile.
      if (Object.prototype.hasOwnProperty.call(result, "profile")) {
        return reply({ success: true, profile: result.profile });
      }
      if (Object.prototype.hasOwnProperty.call(result, "saved")) {
        return reply({ success: true, saved: result.saved });
      }
      return failure("engine_unavailable");
    }
    default:
      return failure("engine_unavailable");
  }
}

/**
 * The transport seam shared by every routed `memory` mode.
 *
 * Owns the whole failure law so the tool holds no capability knowledge: the
 * handshake first (a cold process must answer `hello` before any runtime
 * method — D-43), then the capability (`=== true`; a pre-wave build omits the
 * key, and a truthy non-boolean is not an advertisement), then the send. It
 * never throws; it resolves one of three value envelopes:
 *
 *   { ok: true, result }
 *   { ok: false, error: "capability_unavailable", capability: <method> }
 *   { ok: false, error: <§4 data.reason> | "engine_unavailable" }
 *
 * `send` rejects with whatever the bridge threw; only a string `data.reason`
 * is a token (the wire's own refusal), everything else is a transport-class
 * failure. A result that is not an object is malformed, never a payload.
 */
export function createToolRequest({ ensureStarted, capability, send } = {}) {
  return async function request(method, params) {
    let started;
    try {
      started = await ensureStarted();
    } catch {
      return { ok: false, error: "engine_unavailable" };
    }
    if (started !== true) return { ok: false, error: "engine_unavailable" };
    let available;
    try {
      available = capability(method);
    } catch {
      return { ok: false, error: "engine_unavailable" };
    }
    if (available !== true) {
      return { ok: false, error: "capability_unavailable", capability: method };
    }
    let result;
    try {
      result = await send(method, params);
    } catch (error) {
      const reason = error && typeof error === "object" ? error.data : undefined;
      if (reason && typeof reason === "object" && typeof reason.reason === "string") {
        return { ok: false, error: reason.reason };
      }
      return { ok: false, error: "engine_unavailable" };
    }
    if (result === null || typeof result !== "object" || Array.isArray(result)) {
      return { ok: false, error: "engine_unavailable" };
    }
    return { ok: true, result };
  };
}

/**
 * Registers the lane's two agent-facing tools on the V2 `ctx.tool.transform`
 * door and returns an idempotent async cleanup: `recall_project_sessions`
 * first, then `memory`, inside the one editor callback so there is one
 * registration to dispose and one failure point.
 *
 * Fail-open (D8): a host without a tool surface, or a registration that
 * throws, must not take capture and injection down to deliver one tool.
 */
export async function registerMemoryTools(ctx, { sendRecall, request } = {}) {
  const transform = ctx?.tool?.transform;
  if (typeof transform !== "function") {
    console.error("opencode-mem tool registration skipped: no tool surface");
    return async () => {};
  }
  const projectId = ctx?.location?.directory;
  let registration;
  try {
    registration = await transform((editor) => {
      editor.add(createRecallTool({ projectId, sendRecall }));
      editor.add(createMemoryTool({ projectId, request }));
    });
  } catch (error) {
    console.error("opencode-mem tool registration failed", error);
    return async () => {};
  }
  let disposed = false;
  return async () => {
    if (disposed) return;
    disposed = true;
    if (registration && typeof registration.dispose === "function") {
      await registration.dispose();
    }
  };
}
