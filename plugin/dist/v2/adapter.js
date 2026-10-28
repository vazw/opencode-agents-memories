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

  await ctx.session.hook("prompt", async (event) => {
    // A fresh user turn: whatever the previous turn injected is stale.
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
  });

  await ctx.session.hook("context", async (event) => {
    const injected = contexts.get(event.sessionID);
    if (injected) event.system.push({ type: "text", text: injected });
  });

  const controller = new AbortController();
  const watcher = (async () => {
    if (typeof legacyHooks.event !== "function" || typeof ctx.event?.subscribe !== "function") return;
    try {
      for await (const raw of ctx.event.subscribe({ signal: controller.signal })) {
        if (!(await eventBelongsToLocation(ctx, raw))) continue;
        const event = toLegacyEvent(raw);
        if (event.type === "session.compacted" && event.properties?.sessionID) {
          contexts.delete(event.properties.sessionID);
        }
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
  };
}
