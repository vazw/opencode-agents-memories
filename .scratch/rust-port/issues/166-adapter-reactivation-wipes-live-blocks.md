# 166 — a plugin re-activation wipes the cached block for every live session mid-turn

Status: **closed 2026-11-26** — fixed by PRD 171 §4. The adapter now rehydrates a lost block on a cache-empty
`context` event: replay the V1 inject with no query text, at most once per session per activation, behind a
fail-closed host read. See `## Closeout 2026-11-26` at the end of this file.
Prior status (the owner's answer that unblocked it): 2026-11-26 deploys are **not** followed by host restarts
(*"no, we'll ship the plugin later"*), so the adapter must survive re-activation itself; the fix is the
cache-empty path in the `context` hook — the same code surface as issue 151 and PRD 171 §4. **One fix, three
triggers**, and it landed in PRD 171 §4 rather than as a second patch. The original `ready-for-human` triage follows,
superseded only on the point it was waiting for.
Filed: 2026-11-25, from the issue-151 instrument (`DEBUG` `.tmp/progress/issue-151-context-instrument.debug-1.md`)

## The observation

The issue-151 `context.hook` instrument measured 140 `context` events over 7 sessions, of which 76 read
`has_block:false` with `turn_index:0`. None of them is a session that was never prompted: all 7 sessions carry an
`inject.requested`, and six carry a successful `inject.received` with non-zero `block_bytes` *before* their first
false line.

## The mechanism, MEASURED

- `setup(ctx)` runs once per plugin **activation**, not per session: `packages/plugin/src/promise/adapter.ts:717-719`
  wraps it in `Effect.acquireRelease`, so the returned cleanup runs when that scope closes;
  `packages/core/src/plugin.ts:141-156` closes the previous slot's scope before loading a new revision.
- Activation triggers (`packages/core/src/plugin/supervisor.ts:245-257`) are `sources.changes()`,
  `modules.changes()`, package updates, `Plugin.Event.Updated` and `SdkPlugins.Updated`. **There is no session
  path** — and a file watcher on `plugin/dist/**` is one of the sources.
- Consequently every write under `plugin/dist/**` — a deploy `cp -p`, a formatter, and every one of the six
  mutation/restore pairs of a test sweep — re-instantiates the adapter and clears `contexts` and `turns`.
- MEASURED in the running host: 119 plugin loads while only 7 sessions produced `context.hook` lines (per-minute
  loads 64/16/23/16 across 04:07–04:10Z), and `opencode.log` shows `watcher subscribe … plugin/dist/v2/plugin.js`
  plus `loading plugin` 0.18 s after the wave's `cp -p`.
- Log-only proof that each false line is post-boundary (the "inject floor"): a completed `onPrompt` sets
  `turns[sid]` to `t+1` while `inject.requested` reports the pre-increment `t`, so any later `context.hook` in the
  same activation must read `>= t+1`. All 76 false lines violate the floor; two sessions show a direct
  `turn_index` decrease (`1 → 0`) across the 04:21:39Z restart.

**Consequence.** Any dist write on a live host silently strips the memory block from the remainder of every
in-flight turn — the steps keep running, the block is gone, and nothing is logged as an error. It recovers at the
next prompt. On a production host this is only reachable by deploying without restarting; on this machine it was
reached by our own test sweep.

## Why it matters beyond the instrument

This is the population the issue-151 fallback was contemplated for, with a measured size (76 events, 7 sessions) —
but its trigger is *deployment*, not compaction. Building a query-less `context` fallback now would fix a defect
that exists only while a plugin file is being rewritten underneath a running host.

## Possible dispositions

1. **Accept as a ceiling** if a production host only ever changes `plugin/dist/**` together with a restart, so no
   session survives the write. Cheapest; needs the operator statement that deploys always restart.
2. **Persist `contexts` across activations** (a module-level map keyed by session, discarded only on process exit).
   Removes the loss but leaks state across plugin revisions — the new revision would serve the previous
   revision's block.
3. **Query-less re-inject on activation** — the issue-151 fallback, now with a measured trigger. Only worth it if
   disposition 1 is refused.

## Acceptance criteria (if it becomes a deliverable)

1. A test re-instantiates the adapter mid-turn (simulating a dist write) and asserts the session's block survives
   the re-activation by whichever route disposition 2 or 3 chooses.
2. No `context` event for a session that has a completed prompt in a *previous* activation reads
   `has_block:false` unless a refresh answered empty (`turn_index >= 1` in the current activation).
3. The deploy path is documented: a write under `plugin/dist/**` is a contract boundary, not a no-op.

## Inverted world

Closed as *measurement, not a deliverable* if the operator confirms every deploy of `plugin/dist/**` is followed
by a host restart (so no session can survive the write): the loss is then unobservable outside a test sweep, and
the fixed cost of carrying state across revisions is not bought.

## Not in scope

- Issue 151's never-prompted premise (measured 0 witnesses; see its tracker note).
- The hosted/local embedding model question.

## Triage 2026-11-26 — `ready-for-human`, premise partially confirmed

Full record: `.tmp/progress/issue-166-triage.debug-1.md`. Re-derived read-only; pid 8953 and 8878 not signalled; nothing
under `~/.opencode-mem/` written; `git status --porcelain` empty.

**What holds.** The observation reproduces on the rotated log: 141 `context.hook` lines over 7 sessions, 77 of them
`has_block:false` with `turn_index:0`, 64 true `turn_index:1`, 0 never-prompted sessions, and both exact `1 → 0` drops at
04:21:45Z. **Population refreshed 2026-11-26** over every `2026-11-*` log (`docs/evidence/issue-151-context-reading-20261126/`):
3,135 `context.hook` lines / 62 sessions / **252 `has_block:false, turn_index:0`** / 0 `has_block:false` at `turn_index >= 1`
/ 0 never-prompted. Issue 151 closed as measurement on that reading, so this population now belongs solely to this issue.
The caches are per `registerV2Adapter` invocation, not module-scoped, and only the returned cleanup clears them
(`contexts` is otherwise only `set` or `delete`), so a `turn_index` decrease for one session is possible only through that
cleanup. The chain is `plugin/src/plugin.js:1951` → `:1953` `mod.setup(ctx)` → `plugin/src/v2/plugin.js:57-59` capturing
the adapter cleanup → `:65-68` returning a function that awaits it.

**What does not hold as filed — three corrections.**

1. **The cited mechanism was upstream's.** The `## The mechanism, MEASURED` section above quotes
   `packages/plugin/src/promise/adapter.ts:717-719`, `packages/core/src/plugin.ts:141-156` and
   `packages/core/src/plugin/supervisor.ts:245-257`. **No `packages/` directory exists in this repository.** Those paths
   describe the project this repo ports, and the section is superseded on that point by this triage, not deleted: the
   behaviour it describes is real, its source citation was not this tree's.
2. **No code in this tree calls the cleanup.** `plugin/index.js` only re-exports the definition; the tests and `dist/`
   mirror it. The activation boundary is the **host's**, so any fix that changes when the cleanup runs is not ours to
   make here.
3. **The 76-line population is three different things**, not one: 18 host-restart lines, 31 dist-write wipe lines, and 29
   ordinary cold-cache lines (78 counted in the re-derivation; the filed 76 is within the same family, not the same set).
   The filed **"119 plugin loads" does not reproduce at all** — it is 714 host log lines divided by 6 plugin ids, a line
   count whose per-line meaning was never settled. Treat the load figure as withdrawn.

**Disposition.** `ready-for-human` because the one decisive input is an operator statement, not a measurement this tree can
produce: whether every `plugin/dist/**` deploy is followed by a host restart. If yes, the defect is bounded by deploy
practice, recovers at the next prompt, and closes as *measurement, not a deliverable*. If no, the adapter needs a
cache-empty path in its `context` hook — which is **issue 151's M1**, the same code surface (`adapter.js:117-124`) arriving
by a different trigger. 166 and 151 are therefore one fix but two issues: 151 stays `needs-info`, because its trigger
(compaction, sessions that did prompt) is distinct from this one's (re-activation).

## Closeout 2026-11-26 — fixed by PRD 171 §4

Shipped with PRD 171 §4. Blueprint `.tmp/blueprints/blueprint-20261126-0917-issue-171-s4-context-fallback.md`
(body `81cb1a0bfd7e`, SYS-APPROVED 2026-11-26T09:25Z, revised once after a rejection on 11 points); review round 2
**APPROVED** (`.tmp/reviews/review-20261126-issue-171-s4-round2.md`). Two files: `plugin/src/v2/adapter.js` +66/−3,
`plugin/tests/v2-plugin-adapter.test.mjs` +320/−1.

**Disposition 3 was taken, not 1 or 2** — the owner refused disposition 1 (*"no, we'll ship the plugin later"*), and
disposition 2 (persist `contexts` across activations) was falsified twice: the maps are per `registerV2Adapter`
invocation, so a fresh revision needs a busted module URL and module-level state dies with re-evaluation, and
`globalThis` would leak a previous revision's block into the new one.

**What the fix does.** On a `context` event with no cached block, the adapter replays the V1 inject with an **empty
parts array** — `queryFromParts` (`plugin/src/plugin.js:1639`) yields `undefined`, `injectParams` (`:1665`) omits
`include.query`, so the engine answers from the container-tag list and no text is synthesised. Guarded fail-closed by
`ctx.session.context` returning at least one message; memoised in `rehydrated` **before** the first `await`, so it runs
at most once per session per activation; `withdrawn` refuses a block the engine has emptied; `disposed` short-circuits
after teardown; the post-`await` write is conditional on `!contexts.has && !withdrawn.has` so a prompt landing during
the probe always wins.

**Acceptance criteria, re-derived against what shipped.**

1. *"A test re-instantiates the adapter mid-turn and asserts the session's block survives"* — **not satisfied as
   written, and deliberately so.** Nothing can make the block survive an in-process re-activation (see disposition 2),
   so the shipped contract is that the next model step **rebuilds** it. Tests pin the rebuild, not survival:
   `test_v2_context_fallback_rehydrates_a_lost_block`, `..._requires_host_evidence_of_a_turn`,
   `..._runs_at_most_once_per_session_per_activation`, `..._memoizes_before_the_first_await`,
   `..._leaves_a_withdrawn_block_withdrawn`, `..._never_serves_another_session_block`,
   `..._does_not_overwrite_a_newer_prompt_block`. RED on the pre-fix tree: `# pass 13 # fail 7`; GREEN after: `# pass 20
   # fail 0`.
2. *"No `context` event for a session with a completed prompt reads `has_block:false` unless a refresh answered empty"*
   — **not verifiable in this tree**, and the reason is measured: the population is not separable in-process. Every one
   of the 252 `has_block:false, turn_index:0` lines belongs to a **prompted** session, so the host read is true for all
   of them — including the 18 host-restart and 29 ordinary-cold partitions. The observation that settles it is
   post-deploy and is written into the blueprint's `[TEST PLAN & ACCEPTANCE CRITERIA]`: a session whose first
   `context.hook` in an activation reads `has_block:false, turn_index:0` followed in the same activation by
   `has_block:true, turn_index:0`, plus an `inject.requested` with `query_chars:0` and no `turn_index`. Zero such
   sequences ⇒ the host read returns no messages mid-turn ⇒ dead code ⇒ close §4 as measurement.
3. *"The deploy path is documented: a write under `plugin/dist/**` is a contract boundary, not a no-op"* — **open.**
   The only written record is the approved blueprint's Step 8, which lives under gitignored `.tmp/` and will not
   survive cleanup. `shortcut: the dist-write contract boundary is documented only in gitignored .tmp; add a tracked
   note (plugin/README or the adapter's header) when plugin docs are next touched. Ceiling: the fact is unrecorded;
   upgrade path: three lines beside the existing dist-sync invariant.`

**Accepted ceilings, recorded not hidden.** The fallback fires on every cache-less `turn_index:0` event of a session
with ≥1 host message — including the 29 ordinary-cold lines — and injects one project-listing block on that session's
first model step after activation. `withdrawn` dies with the activation, so a withdrawal recorded before a dist write
is not seen by the next one; it is self-correcting because the replayed inject asks the same engine, which answers
`empty` again unless its state legitimately changed. `hasPriorTurn` has **no timeout**, so a hanging
`ctx.session.context` parks the model step. The replay costs one inject request — and possibly one engine start — on
the model-request critical path, once per session per activation, failing open on error. The replayed path writes the
wrapper's own `inject.requested` (`query_chars:0`, no `turn_index`) and `inject.received`
(`host_contract:"setup"`), so a future audit must not read a fallback-written `inject.requested` as a prompt.

**Live reproduction and repair, 2026-11-26 16:43 +0700.** The deploy of the fixed adapter at `16:43:05.435` cleared the
live caches exactly as this issue describes: `ses_e9d5dd698…` went from `has_block:true, turn_index:8` to
`has_block:false, turn_index:0` within 1.5 s, the fallback replayed a query-less inject 8 ms later, and the session read
`has_block:true, turn_index:0` from 16:43:10.710 onward. Evidence: `docs/evidence/issue-171-s4-postdeploy-20261126/`.
