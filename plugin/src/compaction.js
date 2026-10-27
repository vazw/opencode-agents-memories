// W5.5 — the compaction seam's READ side (V1 wrapper).
//
// This module holds no host-write surface: `notifyRestore` and
// `compactionParams` live in `plugin.js` beside `notifyHost` and `NOTIFIED`,
// because a sibling import back would be a cycle. Everything here is either a
// capability probe or a pure shape transform, so a failure degrades to
// "unknown" (never a throw, never a skip by accident).
//
// The child gate is POSITIVE-EVIDENCE-ONLY (ADR-0017 DECISION 2): a non-empty
// `parentID` means child; an absent `session.get` (the V2 legacy client exposes
// none), a null response or a throw means "not a child, proceed". Treating
// "unknown" as "child" would silently disable the restore for every V2 session.

/** Unwraps the SDK's `{data}` envelope (the V1 SDK returns `{data: ...}`). */
function unwrap(response) {
  return response && typeof response === "object" && "data" in response
    ? response.data
    : response;
}

function trimmedString(value) {
  return typeof value === "string" && value.trim().length > 0
    ? value.trim()
    : undefined;
}

/**
 * One `session.get({path:{id}})`, unwrapped. `null` means "unknown": the host
 * exposes no `get`, the call threw, or the response was not an object.
 */
export async function readSession(client, sessionID) {
  const sessionClient = client?.session;
  if (typeof sessionClient?.get !== "function") return null;
  try {
    const session = unwrap(await sessionClient.get({ path: { id: sessionID } }));
    return session !== null && typeof session === "object" ? session : null;
  } catch {
    return null;
  }
}

/** True exactly when `parentID` is a non-empty string (see the module note). */
export function isChildSession(session) {
  const parentID = session?.parentID;
  return typeof parentID === "string" && parentID.length > 0;
}

/**
 * Upstream `resolveSessionAgent` (`example/opencode-mem/src/index.ts:75-144`):
 * `session.agent`, then the latest non-compaction user message's `agent`, then
 * the latest non-compaction / non-summary assistant `agent` or `mode`.
 *
 * The session object is the one `readSession` already fetched — the hook does
 * not pay for a second `session.get`.
 */
export async function resolveSessionAgent(client, sessionID, session) {
  const direct = trimmedString(session?.agent);
  if (direct !== undefined) return direct;

  const sessionClient = client?.session;
  if (typeof sessionClient?.messages !== "function") return undefined;
  try {
    const messages = unwrap(await sessionClient.messages({ path: { id: sessionID } }));
    if (!Array.isArray(messages)) return undefined;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const info = messages[index]?.info;
      if (!info) continue;
      if (info.role === "user") {
        const agent = trimmedString(info.agent);
        if (agent !== undefined) return agent;
        continue;
      }
      if (info.role === "assistant") {
        if (info.summary === true || info.mode === "compaction") continue;
        const agent = trimmedString(info.agent) ?? trimmedString(info.mode);
        if (agent !== undefined) return agent;
      }
    }
  } catch {
    return undefined;
  }
  return undefined;
}

/**
 * Upstream `resolveSessionModel` (`example/opencode-mem/src/index.ts:150-178`):
 * `{model: {providerID, modelID}, variant?}`; `variant` is omitted when absent
 * or `"default"`. Compaction must pass it explicitly, or the host resolves the
 * injected message's model from the agent and switches the session.
 */
export function resolveSessionModel(_client, _sessionID, session) {
  const model = session?.model;
  if (typeof model?.providerID !== "string" || typeof model?.id !== "string") {
    return undefined;
  }
  const variant =
    typeof model.variant === "string" && model.variant.length > 0 && model.variant !== "default"
      ? model.variant
      : undefined;
  return {
    model: { providerID: model.providerID, modelID: model.id },
    ...(variant !== undefined ? { variant } : {}),
  };
}

/**
 * The synthetic no-reply prompt body. `now` is a process-unique COUNTER
 * (never `Date.now()`, which collides on a back-to-back pair).
 */
export function restorePromptBody({ text, agent, model, variant, now }) {
  const body = {
    parts: [{ id: "prt-compaction-" + now, type: "text", text, synthetic: true }],
    noReply: true,
    agent,
  };
  if (model !== undefined) body.model = model;
  if (variant !== undefined) body.variant = variant;
  return body;
}
