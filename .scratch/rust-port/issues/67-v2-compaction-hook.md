# 67 — V2 first-class compaction hook (pre-compaction ingest)

Status: needs-triage — 2026-10-27 (planning-created by issue 33; the next wave triages it)

Issue 33 wired the compaction seam for V1: `experimental.session.compacting` for the pre-compaction
ingest, and the `session.compacted` event for the post-compaction restore. Under V2 the **restore** half
is inherited (the adapter forwards every location-matching host event to the V1 `event` hook, and
`LEGACY_EVENT_ALIASES` maps `session.compaction.ended` → `session.compacted`), so the read happens and
`plugin/tests/compaction-v2-inheritance.test.mjs` pins it. The **ingest** half is not wired: the V2 host
exposes compaction under a different extension point, and issue 20's carve froze `plugin/src/v2/**`.

## What, located precisely

- Register `ctx.session.hook("compaction", cb)` in `plugin/src/v2/` so V2 gets a first-class
  pre-compaction ingest. Per `@opencode/plugin@2.0.27` `dist/promise/session.d.ts`:
  `SessionHooks { readonly compaction: SessionCompaction }`, where
  `SessionCompaction extends SessionContext` = `{sessionID, model, system: SystemPart[], messages:
  Message[], options, agent, tools}` with `result?: SessionCompactionResult {summary, native?,
  metadata?, tokens?}`. The callback is a single input and has **no output pair**: it either mutates
  `input.messages` / `input.system` or sets `input.result` to supply the summary and skip the model call.
- The V1 `experimental.session.compacting` contract is an `(input, output)` pair with
  `output.context: string[]`; the two are not interchangeable.

## Three preconditions, all owned here

1. **Confirm host-side V2 dispatch.** Issue 33's P18 confirms the *registrable surface* only, from
   published typings (`@opencode/plugin@2.0.27` and `2.1.4`); no V2 typings are installed locally and
   no host-side caller was observed. Measure that the host invokes a registered `compaction` hook before
   relying on it.
2. **Deliberately unfreeze `plugin/src/v2/**`.** That carve belongs to issue 20. Do not edit the tree
   until the owner records the unfreeze.
3. **Decide the V2 child gate and the send path.** Both currently depend on capabilities
   `createLegacyClient` deliberately omits (`plugin/src/v2/legacy-client.js:117-134` exposes only
   `session.messages` and `tui.showToast`). The V1 handler proceeds on an unknown session (ADR-0017
   DECISION 2) and skips the send when `session.prompt` is absent; the V2 wiring must state its own
   answer, not inherit a silent one.

## Acceptance

- A V2 test proving the registered `compaction` hook receives the session and appends (or sets) the
  session-scoped render — over the real `plugin.setup(ctx)`, with a complete fake `ctx`.
- The child-gate and no-send decisions pinned by tests, not prose.
- ADR-0017's DECISION 5 updated with the measured dispatch result (or the follow-up closed as
  *measurement, not a deliverable* if the host does not dispatch).

## Blocked by / Blocks

- Blocked by: an unfreeze of `plugin/src/v2/**` (issue 20) and a host that dispatches the hook.
- Blocks: nothing in issue 33; the V1 lane is complete without it.
