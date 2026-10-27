// W5.2 `js` shard — the adapter's memory-context cache lifecycle, the half of
// upstream `tests/v2-memory-context.test.ts` this lane owns (blueprint D6.1).
// Five cases: three assert upstream case intents (`:28` retained across model
// steps, `:44` sessions isolated, `:58` empty/failed refresh) and two are
// net-new adapter semantics with no upstream oracle (refresh per prompt,
// compaction drop). The file's *store-backed* subject has no port counterpart:
// its nine upstream cases are partitioned with an owner each in
// `docs/agents/testing.md` — `:35`/`:49`/`:53`/`:65`/`:70` → issue 63, `:83` →
// issue 63 (`inject_on`) plus the unowned transcript predicate, `:70`'s
// injected-only half → issue 52.

import test from "node:test";
import assert from "node:assert/strict";

const ADAPTER_MODULE = "../src/v2/adapter.js";

function createHarness() {
  const hooks = new Map();
  let nextEvent = null;
  let handled = null;
  const ctx = {
    location: {
      directory: "/workspace/project",
      project: { id: "project", directory: "/workspace/project" },
    },
    session: {
      hook: async (name, callback) => {
        hooks.set(name, callback);
      },
      get: async () => ({ location: { directory: "/workspace/project" } }),
    },
    event: {
      async *subscribe({ signal }) {
        while (!signal.aborted) {
          const event = await new Promise((resolve) => {
            nextEvent = resolve;
            signal.addEventListener("abort", () => resolve(null), { once: true });
          });
          if (event) yield event;
          if (handled) handled();
        }
      },
    },
  };

  async function deliver(raw) {
    const deadline = Date.now() + 2000;
    while (typeof nextEvent !== "function") {
      if (Date.now() > deadline) throw new Error("the event subscription never parked");
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
    const seen = new Promise((resolve) => {
      handled = resolve;
    });
    nextEvent(raw);
    await seen;
  }

  return { ctx, hooks, deliver };
}

// A V1-shaped hooks map: each prompt consumes the next injection, and the
// adapter harvests whatever landed on `output.message.parts`.
function createLegacyHooks(injections, prompts = []) {
  return {
    "chat.message": async (input, output) => {
      prompts.push(input);
      const block = injections.shift();
      if (block) output.message.parts.push({ type: "text", text: block, synthetic: true });
    },
    event: async () => {},
  };
}

function contextOf(hooks, sessionID) {
  const event = { sessionID, system: [] };
  return hooks.get("context")(event).then(() => event.system.map((part) => part.text));
}

test("test_v2_memory_context_retains_the_injection_across_model_steps", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const prompts = [];
  const cleanup = await registerV2Adapter(
    harness.ctx,
    createLegacyHooks(["original project knowledge"], prompts)
  );

  await harness.hooks.get("prompt")({
    sessionID: "ses-1",
    messageID: "msg-1",
    prompt: { text: "first turn" },
  });

  assert.deepEqual(await contextOf(harness.hooks, "ses-1"), ["original project knowledge"]);
  assert.deepEqual(
    await contextOf(harness.hooks, "ses-1"),
    ["original project knowledge"],
    "a second model step sees the same retained injection"
  );
  assert.equal(prompts.length, 1, "the V1 hook ran once — the context hook never replays it");

  await cleanup();
});

test("test_v2_memory_context_isolates_sessions", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const cleanup = await registerV2Adapter(
    harness.ctx,
    createLegacyHooks(["session one knowledge", "session two knowledge"])
  );

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-1", prompt: { text: "one" } });
  await harness.hooks.get("prompt")({ sessionID: "ses-2", messageID: "msg-2", prompt: { text: "two" } });

  assert.deepEqual(await contextOf(harness.hooks, "ses-1"), ["session one knowledge"]);
  assert.deepEqual(await contextOf(harness.hooks, "ses-2"), ["session two knowledge"]);

  await cleanup();
});

test("test_v2_memory_context_injects_nothing_when_the_v1_hook_injects_nothing", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const legacy = createLegacyHooks(["stale block", null, "after empty lookup"]);
  const cleanup = await registerV2Adapter(harness.ctx, legacy);

  // Seed the cache FIRST: with a fresh adapter, "removed the stale block" and
  // "there never was a block" are indistinguishable.
  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-1", prompt: { text: "first" } });
  assert.deepEqual(await contextOf(harness.hooks, "ses-1"), ["stale block"]);

  // Upstream `:58`, empty half: a refresh that injects nothing drops it.
  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-2", prompt: { text: "second" } });
  assert.deepEqual(
    await contextOf(harness.hooks, "ses-1"),
    [],
    "an empty injection leaves no stale context behind"
  );

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-3", prompt: { text: "third" } });
  assert.deepEqual(
    await contextOf(harness.hooks, "ses-1"),
    ["after empty lookup"],
    "a later prompt recovers"
  );

  // Upstream `:58`, failed half: the adapter drops the block BEFORE the V1 hook
  // runs, so a throwing refresh cannot leave the previous block behind.
  legacy["chat.message"] = async () => {
    throw new Error("refresh lookup failed");
  };
  await assert.rejects(
    () =>
      harness.hooks.get("prompt")({
        sessionID: "ses-1",
        messageID: "msg-4",
        prompt: { text: "fourth" },
      }),
    /refresh lookup failed/,
    "the V1 hook's failure surfaces — the adapter does not swallow it"
  );
  assert.deepEqual(
    await contextOf(harness.hooks, "ses-1"),
    [],
    "a failed refresh drops the stale block too"
  );

  await cleanup();
});

test("test_v2_memory_context_refreshes_the_injection_on_each_prompt", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const cleanup = await registerV2Adapter(
    harness.ctx,
    createLegacyHooks(["first prompt block", "second prompt block"])
  );

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-1", prompt: { text: "first" } });
  assert.deepEqual(await contextOf(harness.hooks, "ses-1"), ["first prompt block"]);

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-2", prompt: { text: "second" } });
  assert.deepEqual(
    await contextOf(harness.hooks, "ses-1"),
    ["second prompt block"],
    "each prompt replaces the cached block — V2 system parts live for one request, so they must not accumulate"
  );

  await cleanup();
});

test("test_v2_memory_context_clears_the_injection_after_a_compaction", { timeout: 5000 }, async () => {
  const { registerV2Adapter } = await import(ADAPTER_MODULE);
  const harness = createHarness();
  const cleanup = await registerV2Adapter(harness.ctx, createLegacyHooks(["before compaction", "after compaction"]));

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-1", prompt: { text: "first" } });
  assert.deepEqual(await contextOf(harness.hooks, "ses-1"), ["before compaction"]);

  // The `properties` form of the host envelope: the merge must keep the id.
  await harness.deliver({
    type: "session.compaction.ended",
    location: { directory: "/workspace/project" },
    properties: { sessionID: "ses-1" },
  });
  assert.deepEqual(
    await contextOf(harness.hooks, "ses-1"),
    [],
    "a compacted session's stale injection is dropped, not re-injected"
  );

  await harness.hooks.get("prompt")({ sessionID: "ses-1", messageID: "msg-2", prompt: { text: "second" } });
  assert.deepEqual(await contextOf(harness.hooks, "ses-1"), ["after compaction"]);

  await cleanup();
});
