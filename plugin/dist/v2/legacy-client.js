// V2→V1 legacy client (W5.2, blueprint D2/D3/D7) — adapts the V2 plugin context
// to the *narrow* slice of the V1 client shape the ported wrapper actually
// consumes:
//   - `session.get({ path: { id } })` → the session ROW (`agent`, `model`,
//     `parentID`). Read by the compaction seam's child gate and agent/model
//     resolution (`plugin/src/compaction.js`) and by the idle capture's
//     attribution — never written;
//   - `session.messages({ path: { id } })` → the transcript the idle capture
//     parses (`plugin/src/plugin.js:668-677`), in the V1 `{ info, parts }` shape;
//   - `session.prompt({ path, body })` → the compaction restore's V1 send: a
//     `noReply` body becomes a `ctx.session.synthetic` write (description
//     `memory context`), everything else a queued host prompt;
//   - `tui.showToast({ body })` → the toast RPC event (`./toast-rpc.js`).
//
// Everything else upstream's 19-case legacy client carries — structured output,
// session create/abort/delete, the provider directory, `legacyToolResult` — is
// deliberately absent: the port's V1 behaviour consumes no such capability, the
// summariser lives in Rust, and provider connectivity is issue 62's (never a
// `client.provider` surface here).
//
// `session.get` was absent until issue 114, which reversed issue 112's D-112-4
// deferral: `ctx.session.context` is bounded to the latest completed compaction
// (`packages/core/src/session/store.ts:194`), so after a compaction the
// transcript is `[compaction, idle]` — no agent-bearing row — and the restore's
// agent gate could never resolve. The row is the only source of truth for the
// gate; agent/model stay advisory on the V2 send (the synthetic route drops
// them, see `prompt`), and the V1 route pins both in its body.
//
// Upstream reference: example/opencode-mem/src/v2/legacy-client.ts.

import { randomUUID } from "node:crypto";
import { resolve } from "node:path";

import { toastPayload } from "./toast-rpc.js";

// opencode v2 no longer publishes `session.idle`, which is what auto-capture is
// built on. The event stream emits `session.execution.{succeeded,failed,
// interrupted}` with `data.sessionID`; forwarding them under the legacy names
// is enough. Names are also stripped of a trailing `.1` version suffix.
const LEGACY_EVENT_ALIASES = {
  "session.compaction.ended": "session.compacted",
  "session.execution.succeeded": "session.idle",
  "session.execution.failed": "session.idle",
  "session.execution.interrupted": "session.idle",
};

// The V1 transcript shape the wrapper's `parseIdleTranscript` reads: `info.role`
// plus `parts[*].type`/`text`/`tool`. `user`/`synthetic`/`system` become a user
// message whose single text part carries the synthetic flag; `assistant` keeps
// its content parts; `compaction` is a summary the parser skips.
function legacyMessage(message, sessionID) {
  if (message.type === "user" || message.type === "synthetic" || message.type === "system") {
    return {
      info: { id: message.id, sessionID, role: "user", agent: message.agent },
      parts: [
        {
          id: `${message.id}-text`,
          sessionID,
          messageID: message.id,
          type: "text",
          text: message.text ?? "",
          synthetic: message.type !== "user",
        },
      ],
    };
  }
  if (message.type === "assistant") {
    return {
      info: {
        id: message.id,
        sessionID,
        role: "assistant",
        agent: message.agent,
        mode: message.agent,
      },
      parts: (message.content ?? []).map((part, index) => ({
        id: part.id ?? `${message.id}-${index}`,
        sessionID,
        messageID: message.id,
        ...part,
      })),
    };
  }
  if (message.type === "compaction") {
    return {
      info: {
        id: message.id,
        sessionID,
        role: "assistant",
        summary: true,
        mode: "compaction",
      },
      parts: [],
    };
  }
  return {
    info: { id: message.id ?? randomUUID(), sessionID, role: "assistant" },
    parts: [],
  };
}

function sessionIDFrom(input) {
  return input?.sessionID ?? input?.path?.id;
}

// One console line, never a throw: a host with no RPC bridge still shows the
// intent somewhere, and the message path is never disturbed.
function toastFallback(input) {
  const body = input?.body ?? input ?? {};
  const text = [body.title, body.message].filter(Boolean).join(": ");
  if (text) {
    const method = body.variant === "error" ? "error" : body.variant === "warning" ? "warn" : "info";
    console[method](`[opencode-mem] ${text}`);
  }
  return { data: false };
}

async function showToastViaRpc(toastRpc, input) {
  if (!toastRpc) return toastFallback(input);
  const payload = toastPayload(input);
  if (payload === null) return toastFallback(input);
  try {
    await toastRpc.events.emit("toast", payload);
    return { data: true };
  } catch {
    return toastFallback(input);
  }
}

/** The V1 send body carries parts; the V2 synthetic route takes one string. */
function textFromParts(parts) {
  if (!Array.isArray(parts)) return "";
  return parts
    .filter((part) => part && part.type === "text" && typeof part.text === "string")
    .map((part) => part.text)
    .join("\n");
}

/** Upstream's description string (`example/opencode-mem/src/v2/legacy-client.ts:308`). */
export const MEMORY_CONTEXT_DESCRIPTION = "memory context";

/**
 * Adapts the V2 plugin context to the two V1 client capabilities the wrapper
 * uses. `toastRpc` is the emitter `ctx.rpc.register(MemToast, {})` returned, or
 * `undefined` when the host has no RPC registry.
 */
export function createLegacyClient(ctx, toastRpc) {
  return {
    session: {
      // Upstream `session.get` (`example/opencode-mem/src/v2/legacy-client.ts:265`),
      // restored (issue 114). The host answers with `Session.Info`
      // (`packages/schema/src/session.ts:32-63`), which carries `agent`, `model`
      // and `parentID`. `readSession` unwraps `{data}`; the host takes
      // `{sessionID}`, the V1 SDK takes `{path:{id}}` — `sessionIDFrom` accepts both.
      // Read for the GATE, not for the write: the synthetic route drops
      // agent/model (see `prompt` below). Do not "simplify" this by deleting the
      // gate — the V1 route pins the resolved agent in its send body.
      get: async (input) => {
        const sessionID = sessionIDFrom(input);
        if (!sessionID) throw new Error("session id required");
        if (typeof ctx?.session?.get !== "function") throw new Error("session.get unavailable");
        return { data: await ctx.session.get({ sessionID }) };
      },
      messages: async (input) => {
        const sessionID = sessionIDFrom(input);
        if (!sessionID) throw new Error("session id required");
        const messages = await ctx.session.context({ sessionID });
        return { data: messages.map((message) => legacyMessage(message, sessionID)) };
      },
      prompt: async (input) => {
        const sessionID = sessionIDFrom(input);
        if (!sessionID) throw new Error("session id required");
        const body = input?.body ?? {};
        const text = textFromParts(body.parts);
        if (!text) throw new Error("prompt text required");
        if (body.noReply === true) {
          if (typeof ctx?.session?.synthetic !== "function") {
            throw new Error("session.synthetic unavailable");
          }
          return {
            data: await ctx.session.synthetic({
              sessionID,
              text,
              description: MEMORY_CONTEXT_DESCRIPTION,
              resume: false,
            }),
          };
        }
        if (typeof ctx?.session?.prompt !== "function") throw new Error("session.prompt unavailable");
        return { data: await ctx.session.prompt({ sessionID, text, delivery: "queue" }) };
      },
    },
    tui: {
      showToast: async (input) => showToastViaRpc(toastRpc, input),
    },
  };
}

/**
 * Maps a raw V2 host event onto the V1 event envelope. Upstream returns three
 * different property shapes; the port must merge the legacy `type` into
 * whichever one it returns — replacing the object drops `properties.sessionID`
 * and silently kills the adapter's compaction cache invalidation (H2).
 */
export function toLegacyEvent(raw) {
  const envelope = raw?.payload ?? raw;
  const source = envelope?.type === "sync" && envelope.syncEvent ? envelope.syncEvent : envelope;
  const rawType = typeof source?.type === "string" ? source.type.replace(/\.1$/, "") : source?.type;
  const type = typeof rawType === "string" ? (LEGACY_EVENT_ALIASES[rawType] ?? rawType) : rawType;
  const data = source?.data ?? {};

  const base =
    source && typeof source === "object" && "properties" in source
      ? source.properties
      : type === "session.created" || type === "session.updated"
        ? { info: data.session ?? data.info ?? data }
        : data;
  return { type, properties: { ...base, type } };
}

/**
 * The global event stream is process-scoped; only events for this project may
 * reach the V1 event hook. Directory-bearing events are decided directly;
 * otherwise the event's session is resolved through the host and its directory
 * compared. Errors and a missing id are `false` — never a forwarded event.
 */
export async function eventBelongsToLocation(ctx, raw) {
  const directory =
    raw?.location?.directory ??
    raw?.directory ??
    raw?.payload?.location?.directory ??
    raw?.payload?.directory ??
    raw?.data?.info?.directory;
  if (typeof directory === "string") {
    return resolve(directory) === resolve(ctx.location.directory);
  }

  const data = raw?.data ?? raw?.payload?.data ?? raw?.properties;
  const sessionID = data?.sessionID ?? data?.session?.id ?? data?.info?.id;
  if (sessionID) {
    try {
      const session = await ctx.session.get({ sessionID });
      const sessionDirectory =
        session?.location?.directory ?? session?.directory ?? session?.data?.directory;
      return (
        typeof sessionDirectory === "string" &&
        resolve(sessionDirectory) === resolve(ctx.location.directory)
      );
    } catch {
      return false;
    }
  }

  return false;
}
