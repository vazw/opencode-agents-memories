# 112 — the V2 post-compaction restore sends a visible synthetic and toasts

Status: done — 2026-11-02 — implemented in the wave that created this file (local commit, nothing pushed).

The 2026-10-21 ruling ("the compaction hook does BOTH halves … the restore mirrors upstream via a
synthetic no-reply prompt") stands; issue 67's no-send deviation was the outlier. This issue restores
compliance: under V2 the restore had never delivered a message — `createLegacyClient` exposed no
`session.prompt`, so `handleCompacted` returned silently at its capability check
(`plugin/src/plugin.js`) and `notifyRestore` was unreachable.

## What changed

- `plugin/src/v2/legacy-client.js` gains `session.prompt` (+ `textFromParts`,
  `MEMORY_CONTEXT_DESCRIPTION`). A V1 `{parts, noReply: true}` body maps onto
  `ctx.session.synthetic({sessionID, text, description: "memory context", resume: false})`; every other
  body maps onto `ctx.session.prompt({sessionID, text, delivery: "queue"})`. The function throws on every
  failure path (no session id, empty text, no `synthetic`, no `prompt`), so `handleCompacted`'s catch
  logs `compaction.restore_failed` and the toast is never reached.
- `plugin/src/v2/adapter.js` drops the unconditional `compaction.restore_skipped` line and its now-unused
  `logWarn` import. The event watcher still invalidates the cached ingest on `session.compacted`.
- `plugin/src/plugin.js` changes by one line only: the surviving skip (agent unresolved) becomes
  observable as `logWarn("compaction.restore_skipped", {reason: "agent unresolved"})`.
- `plugin/src/v2/plugin.js` changes only its header comment: the module is reached solely through
  `plugin/src/plugin.js:1260`'s `await import("./v2/plugin.js")` inside `server().setup`; the package's
  `./v2` export is inert (D-112-8, issue 113).
- `plugin/dist/**` twins copied in the same step; `plugin/package.json`, `plugin/src/tui.js`,
  `plugin/src/v2/toast-rpc.js` and `plugin/src/compaction.js` are byte-unchanged.

## Decisions (D-112-x)

1. **D-112-1 — the restore is a WRITE.** Through `ctx.session.synthetic`; shape copied from
   `example/opencode-mem/src/v2/legacy-client.ts:294-323`.
2. **D-112-2 — `description: "memory context"` is load-bearing in both renderers.** The app/desktop gate
   is `packages/session-ui/src/timeline/projection.ts:1179-1185`; the terminal gate is
   `packages/tui/src/routes/session/rows.ts:392` plus the live-append gate at `:259-275`. The TUI label is
   `description ?? ""` (`packages/tui/src/routes/session/index.tsx:2195`).
3. **D-112-3 — `resume: false`, a deliberate divergence from upstream, with a stated consequence.**
   `packages/core/src/session/session.ts:298-326` admits the row unconditionally
   (`delivery` defaults `"steer"`) and wakes the runner only `if (input.resume !== false && !session.revert)`.
   The row is visible at admission; its `text` enters the model request only on promotion inside the
   runner (`packages/core/src/session/runner/llm.ts:190`). So after a manual `/compact` the restore is
   visible immediately and reaches the model on the next promotion — never describe it as "not
   delivered".
4. **D-112-4 — `session.get` is deliberately NOT added this wave.** (1) The agent gate resolves from the
   transcript (assistant rows carry `$.agent` 146670/146670 all sessions, 5932/5932 live session; user
   rows 0/429 — measured 2026-11-02T05:10:17Z). (2) `readSession` has three callers and adding `get`
   would fire the child gate on the ingest (reversing issue 67) and change idle-capture attribution.
   (3) Neither send carries agent/model/variant, so `get` could not buy anything for the restore; the
   only levers publish a visible notice. Guard: `test_v2_compaction_hook_does_not_apply_the_child_gate`
   and the new send case both pin `sessionGetCalls === []`.
5. **D-112-5 — the adapter's unconditional skip line is retired; one typed line remains** for the one
   surviving skip (agent unresolved) in `plugin/src/plugin.js`. The old line was invisible anyway:
   `logWarn` writes JSON to stderr, and the host log captures no plugin stderr.
6. **D-112-6 — one notification mechanism: the existing toast.** `ctx.attention.notify` is a recorded
   non-use (ADR-0017 DECISION 11).
7. **D-112-7 — `plugin/src/plugin.js` changes by exactly one line** (the agent-unresolved skip).
8. **D-112-8 — the package's `./v2` export is dead; recorded, not removed.** `packages/plugin/src/host.ts:57`
   resolves `./server` then `.`; `./v2` is not in the list. The export stays because
   `plugin/tests/v2-loader-contract.test.mjs:30` pins it. Finding tracked as issue 113.
9. **D-112-9 — `plugin/src/tui.js` needs no change.** Measured: `context.client.rpc(Descriptor).events.on`
   is the live API (`packages/tui/src/feature-plugins/prompt/goal.tsx:14`, `:29`), and the toast options
   shape is current (`packages/plugin/src/tui/context.ts:269-282`; implementation
   `packages/tui/src/plugin/api.tsx:108-135`).

## Acceptance

- `plugin/tests/v2-legacy-client.test.mjs` 7 → 11 cases (routing, `resume`, rejection, text-parts).
- `plugin/tests/v2-compaction-hook.test.mjs` 8 → 10 cases (send + toast replaces the no-send case; the
  agent-skip line; toast-only-after-write).
- `plugin/tests/compaction-v2-inheritance.test.mjs` 1 case, fixture gains an agent-bearing `context`
  message; assertion (b) re-pointed to one synthetic + one toast; (a) unchanged.
- Wrapper suite `pass 179 / fail 0` over 27 files.
- **Live verification is owner-run and pending**: `/compact` in a session with a visible user message,
  then the read-only `session_message` query. The exact command block and the replay path are in the
  handoff; the DB row plus the toast are the gate, not a log line.

## Measurement

`docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md` — the verbatim agent-coverage queries with
their output, the pre-fix row census, and the host-source line re-derivations.

## Parent

ADR-0017 DECISION 5 (amended 2026-11-02) and DECISION 11; reverses issue 67's send-path decision.
