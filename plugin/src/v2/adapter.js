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
export async function registerV2Adapter(ctx, legacyHooks, options = {}) {
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
  // Issue 149: sessions that compacted since their last prompt. The compaction
  // hook only marks; the mark is consumed at the top of the next `onPrompt`
  // (inside the per-session chain), so a refresh already in flight is never
  // lost. `first` mode keeps replaying a cache that a compaction has made stale.
  const stale = new Set();
  // Issue 155: sessions whose last refresh answered nothing. The next prompt
  // asks again even in `first` mode, where a live cache would otherwise replay
  // the block that refresh failed to replace.
  const retry = new Set();
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

  // Issue 151: the diagnostic writer is INJECTED, never resolved here — the
  // caller decides whether one exists, so flag-off is `if (log)` rather than an
  // environment read inside the adapter, and its silence is a plain unit test.
  const { log } = options;

  await ctx.session.hook("prompt", (event) => serialized(event.sessionID, () => onPrompt(event)));

  async function onPrompt(event) {
    const turnIndex = turns.get(event.sessionID) ?? 0;
    turns.set(event.sessionID, turnIndex + 1);
    let status = null;
    // Issue 155: the compaction mark forces one refresh; it no longer deletes
    // the block in anticipation of a replacement the engine may never send.
    const compacted = stale.delete(event.sessionID);
    const retried = retry.delete(event.sessionID);
    const hasCache = contexts.has(event.sessionID);
    // `first` mode keeps a nonempty cache; the cache lives until a refresh, a
    // compaction mark or adapter cleanup (staleness is the documented
    // first-mode tradeoff). A compaction or a retry refreshes regardless.
    if (!compacted && !retried && !shouldRefresh({ turnIndex, hasCache })) return;

    const original = { type: "text", text: event.prompt.text };
    const output = { message: { id: event.messageID, parts: [original] } };
    // Non-enumerable: deepEqual on the hook output must not see it (issue 150).
    Object.defineProperty(output, "turnIndex", { value: turnIndex, enumerable: false });

    if (typeof legacyHooks["chat.message"] !== "function") return;
    // Issue 155: the wrapper cannot distinguish a transport failure from an
    // empty answer except through `injectStatus`, and an unhandled rejection
    // would reach the host's turn loop. A throw therefore keeps the block and
    // asks again next prompt.
    try {
      await legacyHooks["chat.message"]({ sessionID: event.sessionID }, output);
      status = output.injectStatus;
    } catch {
      status = "error";
    }

    // Harvest what the V1 hook appended; the original part is never a block.
    const injected = output.message.parts
      .filter((part) => part !== original && part?.type === "text")
      .map((part) => part.text)
      .join("\n");
    if (injected) contexts.set(event.sessionID, injected);
    // An empty SUCCESS also reports a purge, an expiry or a scope change, so
    // the withdrawn block is dropped. Anything else keeps it.
    if (status === "empty") contexts.delete(event.sessionID);
    if (!injected && status !== "empty" && contexts.has(event.sessionID)) retry.add(event.sessionID);
  }

  await ctx.session.hook("context", async (event) => {
    // Issue 151: measured before the push, so the line records the state the
    // hook found. Three fields only — no prompt text, no block text, no
    // directory (memory text is user-private).
    if (log) log("context.hook", { session_id: event.sessionID, has_block: contexts.has(event.sessionID), turn_index: turns.get(event.sessionID) ?? 0 });
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
    if (typeof event?.sessionID === "string" && event.sessionID) stale.add(event.sessionID);
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
        const event = toLegacyEvent(raw);
        // Issue 152: `shell.*` envelopes carry neither a project directory nor
        // a top-level session id — their `info.id` is a SHELL id, so the
        // location filter refuses them by construction. The bypass is exactly
        // the `shell.` prefix; every other event keeps today's filter.
        if (!event.type.startsWith("shell.") && !(await eventBelongsToLocation(ctx, raw))) continue;
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
    stale.clear();
    retry.clear();
  };
}
