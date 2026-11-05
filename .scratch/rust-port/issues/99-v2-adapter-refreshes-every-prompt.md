# 99 — the V2 adapter throws the injected block away on every prompt; upstream refreshes only on `refreshOnPrompt()`

Status: done — 2026-11-04. Work will modify `plugin/src/v2/adapter.js`, re-copy
`plugin/dist/v2/adapter.js` byte-identically, and create `plugin/tests/v2-context-refresh.test.mjs`.
Currently owned by nobody.

## What to build

In V2, the injected memory block lives for one request and must be re-inserted per turn from a per-session
cache. Upstream keeps that cache across turns and drops it only when the config says so; the port drops it
unconditionally.

MEASURED:

- Port — `plugin/src/v2/adapter.js:37-39`, inside the `prompt` hook:
  ```js
  await ctx.session.hook("prompt", async (event) => {
    // A fresh user turn: whatever the previous turn injected is stale.
    contexts.delete(event.sessionID);
  ```
  Every prompt evicts the session's cached block, so every turn re-assembles it from the engine.
- Upstream — `example/opencode-mem/src/v2/adapter.ts:162`:
  `if (memoryContext.refreshOnPrompt()) contexts.delete(event.sessionID);` and only then
  `if (!contexts.has(...)) contexts.set(..., await memoryContext.load(...))` (`:163-165`). The refresh is
  a config decision (`refreshOnPrompt: () => injectOn === "always"`, `index.ts:721`), not a default.

Consequences: with the upstream default (`injectOn` not `"always"`), upstream injects once per session and
reuses; the port performs a full `inject` round-trip on **every** turn. That is both an extra engine call
per turn and the mechanism by which the model's reference context drifts mid-conversation toward rows this
session just produced (issue 85's `excludeCurrentSession` story — the gate alone does not fix drift if the
block is rebuilt every turn).

`plugin/dist/v2/adapter.js` exists and is what the host imports for the `./v2` export
(`plugin/package.json`), so the copy is part of the change, not an afterthought.

## Acceptance criteria

- [ ] With the default configuration, the block is assembled **once** per session: two consecutive prompts
      in one session produce exactly one `inject` request (asserted at the stub-engine wire, not by
      reading the cache).
- [ ] With `injectOn = "always"` (upstream `refreshOnPrompt()` true), each prompt refreshes: two prompts
      produce two requests.
- [ ] `plugin/dist/v2/adapter.js` is byte-identical to `plugin/src/v2/adapter.js`
      (`plugin/tests/dist-sync.test.mjs` covers `plugin.js`; extend the same invariant to the v2 copy, or
      state why it is exempt).
- [ ] The failure path keeps its current shape: a failed `load` deletes the cache entry rather than
      caching an empty block (upstream `adapter.ts:166-169`).

## Blocked by

- Issue 63 — `inject_on` is one of the six resolved-but-unread keys; without a reader there is nothing for
  `refreshOnPrompt()` to consult, and a refresh policy cannot be implemented against a key nothing reads.

## Evidence

`plugin/src/v2/adapter.js:37-39`; `example/opencode-mem/src/v2/adapter.ts:155-169`;
`example/opencode-mem/src/index.ts:721`; issue 85 (the drift this produces); issue 63 (`inject_on`). Found
by the 2026-11-01 issue-set audit as a finding with no owning issue.

## r3 progress (2026-11-04)

Implemented in `plugin/src/v2/adapter.js` with `hooks.shouldRefresh` supplied by the wrapper. `first` retains a nonempty cache after
turn 0; `always`, unknown, empty and failed results refresh/retry. Same-session prompts are serialized per session. Tests live in
`plugin/tests/v2-inject-delivery.test.mjs`, not a separate `v2-context-refresh` file.

## Comments

[decision] 2026-11-04 — Status flipped ready-for-agent → done (verification pending).
Reason: this issue was GATE APPROVED and coder-closed in the prior wave; its Status: line was never flipped (tracker defect class covered by issue 89). Per docs/agents/issue-tracker.md, a done line names the commit or outcome record under ## Comments; this wave's ledger/handoff/review artifacts live under .tmp/progress/. No source change this session — Status line only.
