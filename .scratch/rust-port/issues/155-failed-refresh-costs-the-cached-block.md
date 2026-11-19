# 155 — a failed or empty refresh deletes the cached block, so one bad inject costs the session its memory for a turn

Status: needs-triage — 2026-11-19 (defect named by issue 150's plan gate; the fix is an adapter change, and whether stale-but-working beats absent is a product call)

*(Filed from `GATE: REJECTED` on `blueprint-20261119-0026-issue-150-refresh-rule-r1.md`, required modification 7. Issue 150's criterion 12 pins the current loss so the fix will flip it.)*

## The defect

`plugin/src/v2/adapter.js` clears the cached block **before** it calls the engine:

1. `contexts.delete(event.sessionID)` runs once the cache is deemed stale;
2. the `chat.message` call then asks the engine for a fresh block;
3. if that call throws, times out, or answers with an empty block, nothing is cached — so the `context` hook has
   nothing to push onto `event.system` for that prompt.

The session loses a block it had a moment earlier and was going to use anyway. It recovers on the next prompt, because
`shouldRefresh` returns true when `!hasCache` — so the loss is bounded to one turn. It is still a loss: the turn that
just compacted, or just crossed a refresh boundary, is the turn that most needs the memory.

## Why it is being filed now rather than ignored

In the shipped default (`inject_on: "first"`) this fires once per session, at turn 0, where there is no previous block to
lose. Issue 150 bounds staleness with a refresh every 20th prompt, which turns a once-per-session edge into a
once-per-twenty-prompts one — every one of which can now destroy a block that was working. So 150 does not *cause* this
defect, it makes it recurring, which is why its gate refused to let the risk stay deferred and unbound.

## The tension the fix has to settle

"Keep the previous block when the refresh comes back empty" is not free. An empty block is also how the engine reports
that the session's memories are **gone** — purged, expired, or scope-changed. Serving the previous block then delivers
content the engine has just told us not to deliver. So the issue is not "restore the old block"; it is "distinguish
`refresh failed` from `refresh legitimately empty`", and today the wrapper cannot tell them apart:

- a transport or engine error is distinguishable (`status` is not `ok`) and could keep the previous block;
- a successful answer carrying zero memories is not distinguishable from a purge without asking the engine, which costs
  another inject.

## What it would touch

- `plugin/src/v2/adapter.js` — `onPrompt` (the delete at the staleness gate, the `chat.message` call after it) and
  `plugin/dist/v2/adapter.js` as its byte-identical mirror.
- `plugin/tests/v2-plugin-adapter.test.mjs` — the pinned contracts for the no-cache case and cleanup.
- Nothing in `crates/`. The block text arrives whole; the decision is entirely wrapper-side.

## Acceptance shape, not a prescription

Issue 150's criterion 12 pins the present behaviour under the stub's `"silent"` inject mode: prompt 20 yields no context,
prompt 21 re-injects. The fix flips that assertion for the transport-error case at minimum, and the issue is settled by
whichever of these the owner chooses:

1. **Keep on error only.** `status !== "ok"` retains the previous block; a clean empty answer still clears it. No privacy
   question, no extra round trip, and it covers the case that actually happens (engine down, timeout).
2. **Keep on any empty answer**, with a one-turn staleness cap so a purge cannot be served indefinitely. Bounds the loss
   harder, weakens the purge guarantee for one turn, and needs the cap to be tested.
3. **Ask the engine**, adding a cheap "does this session still have memories" call, so `purged` and `failed` are truly
   distinguishable. Most correct, most expensive, and it needs a new engine capability.

## Inverted world

If a live `first`-mode child whose prompt-20 inject fails still renders a block on prompt 20 (because the host replays an
earlier message, or the adapter's delete happens after the call), then the delete-before-call reading above is wrong and
this closes as measurement. The observation to run: stub `"silent"` at prompt 20 and read the `context` hook's argument —
which is exactly what criterion 12 does offline.
