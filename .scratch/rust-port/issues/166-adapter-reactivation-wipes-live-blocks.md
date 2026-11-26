# 166 — a plugin re-activation wipes the cached block for every live session mid-turn

Status: ready-for-human — 2026-11-26 (triage `.tmp/progress/issue-166-triage.debug-1.md`: the observation reproduces, but the mechanism is host-side and **no code in this tree calls the cleanup**, so the fix surface is outside the repository. Needs one operator answer: **is every `plugin/dist/**` deploy followed by a host restart?** Yes ⇒ `wontfix` as *measurement* — the loss is bounded by deploy practice and recovers at the next prompt. No ⇒ the adapter's `context` hook needs a cache-empty path, which is the same code surface as issue 151. See `## Triage 2026-11-26` for the corrected population.)
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
04:21:45Z. The caches are per `registerV2Adapter` invocation, not module-scoped, and only the returned cleanup clears them
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
