// V2→V1 adapter (W5.2, blueprint D2) — translates V2 session hooks and the
// event stream onto the hooks map `server()` returns, so the engine surface
// cannot fork: under V2 the very same V1 closure mints the capture id, spawns
// one child, one hello, the same six runtime methods.
//
// Crossing INTO V1: the prompt text and its injected parts, event envelopes,
// the session id, the project directory. Crossing OUT: the injected text block.
// Not crossing: a second spawn, a store path, provider/model state, or any tool
// and skill registration (issues 22/26 own those surfaces).
//
// Two port-specific shapes are load-bearing (H1/H2):
//   - the port's V1 hook pushes injected text onto `output.message.parts`, not
//     `output.parts`, so the adapter builds and harvests that shape;
//   - the port's V1 event hook keys on `properties.type`, which
//     `toLegacyEvent` merges in.
//
// Upstream reference: example/opencode-mem/src/v2/adapter.ts.

import { eventBelongsToLocation, toLegacyEvent } from "./legacy-client.js";

/**
 * Registers the V2 session hooks and the event watcher, and returns an
 * idempotent cleanup. Throws only for a contract violation: a host without
 * `ctx.session.hook` is not a V2 host and must not silently load this entry.
 */
export async function registerV2Adapter(ctx, legacyHooks) {
  if (typeof ctx?.session?.hook !== "function") {
    throw new TypeError("v2 adapter requires ctx.session.hook");
  }

  // sessionID → the injected text block, as it is retained across the model
  // steps of one turn. Plain strings only: this map is the one state that
  // touches both the message path and the event path.
  const contexts = new Map();
  // Issue 99: per-session zero-based count of accepted prompt-hook invocations
  // in this adapter lifetime — the `turnIndex` the wrapper's `shouldRefresh`
  // receives (plugin/src/plugin.js). Cleared on cleanup.
  const turns = new Map();
  // Same-session prompts run one after another (a promise chain per session),
  // so an older completion can never overwrite a newer prompt's cache.
  // Different sessions never wait on each other.
  const queues = new Map();

  function serialized(sessionID, work) {
    const run = (queues.get(sessionID) ?? Promise.resolve()).then(work);
    const tail = run.catch(() => {});
    queues.set(sessionID, tail);
    tail.then(() => {
      if (queues.get(sessionID) === tail) queues.delete(sessionID);
    });
    return run;
  }

  // Absent hook (older wrapper) means refresh every prompt: the legacy behaviour.
  const shouldRefresh = (arg) =>
    typeof legacyHooks.shouldRefresh === "function" ? legacyHooks.shouldRefresh(arg) !== false : true;

  await ctx.session.hook("prompt", (event) => serialized(event.sessionID, () => onPrompt(event)));

  async function onPrompt(event) {
    const turnIndex = turns.get(event.sessionID) ?? 0;
    turns.set(event.sessionID, turnIndex + 1);
    const hasCache = contexts.has(event.sessionID);
    // `first` mode keeps a nonempty cache; the cache lives until a refresh,
    // a compaction-location cleanup or adapter cleanup (staleness is the
    // documented first-mode tradeoff).
    if (!shouldRefresh({ turnIndex, hasCache })) return;
    // A refreshing turn: whatever the previous turn injected is stale (a failed
    // or empty refresh leaves no block, so the next prompt retries).
    contexts.delete(event.sessionID);

    const original = { type: "text", text: event.prompt.text };
    const output = { message: { id: event.messageID, parts: [original] } };

    if (typeof legacyHooks["chat.message"] !== "function") return;
    await legacyHooks["chat.message"]({ sessionID: event.sessionID }, output);

    // Harvest what the V1 hook appended; the original part is never a block.
    const injected = output.message.parts
      .filter((part) => part !== original && part?.type === "text")
      .map((part) => part.text)
      .join("\n");
    if (injected) contexts.set(event.sessionID, injected);
  }

  await ctx.session.hook("context", async (event) => {
    const injected = contexts.get(event.sessionID);
    if (injected) event.system.push({ type: "text", text: injected });
  });

  // Issue 67: the host's V2 compaction hook is a SINGLE input with no output
  // pair, so the adapter delegates to the V1 closure — the issue-68 capability
  // gate and the 2 s request timeout are inherited, not re-implemented — and
  // copies the render onto `event.system`, the array the request builder reads
  // back. Never `event.messages` (a user-role transcript message), never
  // `event.result` (it skips the model call and replaces the host's summary).
  await ctx.session.hook("compaction", async (event) => {
    try {
      const handler = legacyHooks["experimental.session.compacting"];
      if (typeof handler !== "function") return;
      const output = { context: [] };
      await handler({ sessionID: event?.sessionID }, output);
      if (!Array.isArray(event?.system) || !Array.isArray(output.context)) return;
      for (const text of output.context) {
        if (typeof text === "string" && text.length > 0) event.system.push({ type: "text", text });
      }
    } catch {
      // The host bridges this callback with `Effect.promise`: a rejection is a
      // defect that aborts its compaction. One fixed line — never the error
      // object, never the render (it is memory text, possibly user-private).
      console.error("opencode-mem compaction ingestion failed");
    }
  });

  const controller = new AbortController();
  const watcher = (async () => {
    if (typeof legacyHooks.event !== "function" || typeof ctx.event?.subscribe !== "function") return;
    try {
      for await (const raw of ctx.event.subscribe({ signal: controller.signal })) {
        if (!(await eventBelongsToLocation(ctx, raw))) continue;
        const event = toLegacyEvent(raw);
        // Compaction is mid-turn: retain context until policy refresh/cleanup.
        await legacyHooks.event({ event });
      }
    } catch (error) {
      // Failures never reach the host loop; an abort is an expected teardown.
      if (!controller.signal.aborted) {
        console.error("opencode-mem event bridge failed", error);
      }
    }
  })();

  return async () => {
    controller.abort();
    await watcher;
    contexts.clear();
    turns.clear();
    queues.clear();
  };
}
