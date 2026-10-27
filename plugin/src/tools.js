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

/**
 * Registers the lane's one tool on the V2 `ctx.tool.transform` door and
 * returns an idempotent async cleanup.
 *
 * Fail-open (D8): a host without a tool surface, or a registration that
 * throws, must not take capture and injection down to deliver one tool.
 */
export async function registerMemoryTools(ctx, { sendRecall } = {}) {
  const transform = ctx?.tool?.transform;
  if (typeof transform !== "function") {
    console.error("opencode-mem tool registration skipped: no tool surface");
    return async () => {};
  }
  let registration;
  try {
    registration = await transform((editor) =>
      editor.add(
        createRecallTool({ projectId: ctx?.location?.directory, sendRecall })
      )
    );
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
