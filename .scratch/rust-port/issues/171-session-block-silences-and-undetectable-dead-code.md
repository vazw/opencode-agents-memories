# 171 — session-block silences and the checks that cannot see them

Status: ready-for-agent — **§4 only**. Progress: §1 `cdc57da`, §2 `c28375f`, §3 `3b3827c` shipped; §5 **closed as
measurement** 2026-11-26 (the activation-boundary row already existed and its mutant reds — see §5). Remaining work: a
cached-block fallback in the V2 adapter's `context` hook (`plugin/src/v2/adapter.js:117-124`).
Filed: 2026-11-26, consolidating the recorded-but-unfixed findings from the issue-165 wave and its neighbours.

> A PRD, not five tickets. Each finding below is independently closable and independently testable at an **existing**
> seam; the seams were agreed before writing. If the owner splits this file, each numbered section is a self-contained
> issue.

## Problem Statement

Five things in this repository can report success while the thing they claim did not happen, and a user of the memory
system feels the consequences before any test or lint does.

- **A memory block silently disappears mid-turn.** A plugin re-activation clears the adapter's cached blocks for every
  live session, so the remainder of an in-flight turn runs with no memory in it — steps keep executing, the block is
  gone, and nothing is logged as an error. It recovers at the next prompt, which is exactly why it went unnoticed: the
  symptom is a *worse answer*, not a failure.
- **A session that never prompted gets nothing from the `context` path at all.** The hook pushes the cached block only
  if a cache entry exists; a session with no entry receives nothing from that path and no fallback exists.
- **Tool search can return nothing for an all-projects scope and no test would notice.** The bridge passes an empty
  container tag for `AllProjects`; the search path is live but has never been exercised by a test at that scope, so a
  regression there would be silent.
- **A runbook cell can print a green line while the engine was never there.** §8's outcome prose lists every result code
  except `rc 1`, so one real failure mode is documented as if it were another, or as nothing.
- **Orphaned code is invisible to the compiler.** Fourteen modules under the crate that owns the store carry a
  module-level `#![allow(dead_code)]`, and the item class they hold is `pub` in a `pub mod` besides. A function can lose
  every caller in the tree and no gate in this repository will say so.

The common shape is the one this project has now caught repeatedly: **an instrument that answers a slightly different
question than the one asked**, or a check whose failing input cannot be produced.

## Solution

Close each gap at the seam that already exists, and prove each fix by making the check fail first.

- §8's prose gains its missing `rc 1` row so the outcome list is complete and each code maps to one meaning.
- The `mem-store` dead-code blanket is narrowed to the modules that actually need it, so the crate's own lint reports an
  orphan instead of hiding it; the one function already known to be orphaned is documented as such deliberately.
- The bridge's all-projects search path gains a row that drives the `search` method with `ScopeSelection::AllProjects`
  and asserts rows come back — the regression that a press of a "collapse the empty-tag branch" refactor would cause.
- The V2 adapter's `context` hook gains a defined behaviour when no cached block exists, so a session is never silently
  empty for the whole turn.
- The adapter's caches **already had** the activation-boundary test, which is why §5 closed as measurement rather than
  work: `plugin/tests/v2-plugin-adapter.test.mjs:364` drives construct → `context` → disposal → `context` and reds on
  the block-drop assertion when `contexts.clear()` is removed. The gap was that nobody had shown the row bites, not
  that the row was missing.

## User Stories

1. As a user asking a question mid-turn, I want the injected memory block to survive a plugin reload, so that the answer
   does not silently degrade halfway through a response.
2. As a user, I want a plugin re-activation to re-fetch or retain each live session's block, so that a deploy does not
   cost every in-flight session its context.
3. As an operator, I want a re-activation that drops a live block to say so in the log, so that a degraded session is
   diagnosable after the fact instead of inferred.
4. As a user starting a session in which the prompt hook never fires, I want the `context` path to supply something
   useful, so that the session is not blind for its whole first turn.
5. As a user whose block was withdrawn by the engine, I want the withdrawal respected, so that a fallback never
   resurrects memory the engine deliberately emptied.
6. As an agent using the all-projects memory tool, I want search to return rows across projects, so that a cross-project
   question does not silently return an empty page.
7. As a maintainer refactoring the search path, I want a test to fail if I collapse the empty-container-tag branch, so
   that a plausible simplification cannot zero a user-visible tool.
8. As a reviewer, I want the all-projects search path covered at the wire seam, so that the coverage gap is closed where
   the two existing all-projects tests already live.
9. As an operator running the parity rehearsal, I want every §8 result code documented with one meaning, so that `rc 1`
   cannot be read as a stop, as a refusal, or as nothing.
10. As an operator, I want a §8 cell that fails for an unexpected reason to still fail, and to say which reason, so that
    a green line never covers a red fact.
11. As a maintainer, I want the crate's own lint to report a function that lost every caller, so that orphans surface at
    the commit gate rather than in a review conversation.
12. As a maintainer, I want the dead-code suppression narrowed to what genuinely needs it, so that the suppression does
    not silently expand to new modules.
13. As a reviewer, I want a documented reason for the one function that is deliberately kept without a caller, so that
    "kept for upstream parity" is a recorded decision rather than a guess.
14. As a maintainer, I want each of these five closures to be demonstrated by a failing check first, so that a fix
    cannot be accepted on the strength of a passing check that could never have failed.
15. As an owner reading the tracker, I want the five findings consolidated in one place with their measured sizes, so
    that the debt is prioritised against feature work rather than rediscovered each wave.
16. As a future agent, I want the entry-point documents to name the symbols each closure creates, so that the status line
    is checkable against the tree.
17. As a user, I want a session that lost its block to recover automatically, so that no manual intervention is needed
    after a deploy or a test sweep.
18. As an operator, I want the live engine and host left untouched by this work, so that measuring a defect never
    becomes causing one.

## Implementation Decisions

- **One PRD, five independently closable sections.** Sections 1–5 below are ordered by cost, cheapest first. No section
  blocks another.
- **§8's outcome prose is documentation, so it carries no behavioural oracle of its own.** Its acceptance is that the
  prose lists all six result codes the shipped fence can emit, with one meaning each; the fence's behaviour is already
  covered by issue 168's fifteen executed rows and is not re-litigated here.
  *Closed 2026-11-26:* the six are 0, 1, 3, 4, 5 and 6 — the explicit `exit` branches plus rc 0 and the implicit rc 1
  from the block's final `! ss -ltn | grep -q … && echo …`. rc 1 means the engine was stopped but the port stayed bound,
  and the block prints no message for it. MEASURED on a throwaway listener: port bound → rc 1, empty output; port free →
  rc 0 plus the echo. Documented in the runbook, which also records a **gap left unfixed**: with `ss` absent or unable to
  list sockets, the pipeline yields nothing, the negated test succeeds and the cell prints `port_3749_free_again`
  falsely — measured, rc 0 with the green line and a `command not found` on stderr. The same idiom appears at `:95-96`,
  so a fix would touch three sites and change cell behaviour that issue 168's mutant rows pin; it is recorded here rather
  than edited.
- **Dead-code suppression is narrowed per module, not removed wholesale.** The crate legitimately suppresses the lint in
  modules holding protocol-shaped items; the change is to stop the suppression covering modules where an orphan is a
  real defect.
  **MEASURED 2026-11-26, and it corrects this section's premise.** Stripping all fourteen module-level
  `#![allow(dead_code)]` in a throwaway worktree and running `cargo clippy -p mem-store` produced exactly **two**
  diagnostics, both in one file and both private: `shard_path_migration.rs:549 count_connected` (a one-line wrapper over
  `count_live_memories`, zero callers) and `shard_path_migration.rs:234 RollbackOutcome::failed` (written, never read —
  callers branch on `first_error`). It did **not** produce one for `keyword_scores`, the case that motivated the section:
  that item is `pub` in a `pub mod`, and rustc exempts that visibility *before* the `allow` is consulted. So the hiding
  mechanism is **visibility, not the suppression**: deleting fourteen `allow` lines buys coverage of private orphans in
  those modules and would still have missed the orphan this section was written for. The section therefore shrinks to:
  remove the two now-visible dead items, strip the module-level allows, and keep the gate green — with the pub-orphan
  class explicitly left uncovered, because the instrument that would cover it is a visibility audit (`pub(crate)` where
  the item has no external consumer) and not a lint configuration. The already-orphaned store function stays, documented as a deliberate upstream-parity port — the finding
  is that nothing could have noticed it, not that keeping it is wrong.
- **The bridge keeps the conditional container-tag clause.** An empty tag means "every project", and the clause is
  load-bearing for that scope. The decision recorded by this PRD is explicitly **not** to collapse it; the test exists to
  make the collapse fail.
- **The `context` fallback is query-less by construction.** It must not re-run a search with invented text; it answers
  from state the adapter already holds. A withdrawn block stays withdrawn — the engine's `empty` status deletes the
  cache entry and the fallback must not resurrect it.
- **The activation-boundary test drives the adapter's disposal path directly** rather than restarting a host: the
  disposal function is the thing that clears the caches, so a test that calls it and then drives a `context` event is the
  highest existing seam for that behaviour.
- **No new test infrastructure, no new dependency.** Every closure uses a seam that already exists in the tree.
- **Nothing in this PRD writes to the live store, the live log, or the live host.**

## Testing Decisions

A good test here checks an **external behaviour** — what a user or an operator observes — and fails on a plausible wrong
implementation, not merely on the absence of the right one. Every row below names the mutant it must kill.

1. **§8 prose (documentation).** No behavioural test. Acceptance is content: the outcome list enumerates every code the
   fence can emit, and a grep for the code set matches the fence's own branches. Prior art: issue 168's row set.
2. **Dead-code narrowing.** Seam: the existing pre-commit clippy gates (`-D warnings`). Acceptance is that removing the
   last caller of a function in a *narrowed* module produces a warning, while the modules that keep the suppression do
   not. Prior art: `.githooks/pre-commit` and `scripts/test-pre-commit.sh`'s behavioural cases.
3. **All-projects search.** Seam: `crates/mem-bridge/tests/wire_methods.rs`, the same fixture and assertion style as the
   existing all-projects `list` and `forget` rows. The row must drive `search` with `ScopeSelection::AllProjects` and
   assert a non-empty page from a seeded row. **Mutant it must kill:** binding the container tag unconditionally, which
   returns zero rows for this scope. Prior art: the two adjacent all-projects rows in the same file.
4. **`context` fallback.** Seam: the wrapper's node test suite, driving the `context` hook through the adapter. Rows:
   no cache and no prior prompt yields something; a withdrawn block yields nothing; a cached block is unchanged.
   **Mutants:** a fallback that fires on the withdrawn block; a fallback that returns the previous session's block.
   Prior art: the adapter wave that shipped the block-lifecycle behaviours, and the inject-status end-to-end row.
5. **Activation boundary — CLOSED 2026-11-26 as measurement; no deliverable.** The planned seam already contained the
   row. `plugin/tests/v2-plugin-adapter.test.mjs:364`
   (`test_v2_adapter_cleanup_aborts_the_subscription_and_is_idempotent`) runs exactly the sequence this section asked
   for: register → `prompt` caches `["cached before cleanup"]` → `readContext("ses-1")` returns it → `await cleanup()`
   (the teardown at `plugin/src/v2/adapter.js:173-181`) → `readContext("ses-1")` asserts `[]` with the message
   *"cleanup drops the cached block, so a late context hook injects nothing"*.
   **MEASURED, mutant killed:** deleting `contexts.clear();` (the only line removed, `adapter.js:176`) reddens the row at
   `:394` — `actual: ['cached before cleanup']`, `expected: []`, `AssertionError … cleanup drops the cached block, so a
   late context hook injects nothing` — `rc 1`; restoring leaves `git diff --stat plugin/src/v2/adapter.js` empty and the
   row green again (`pass 1 / fail 0`). So the PRD's stated risk ("staying a log-reading instead of a property") was
   already false: the property was pinned and the mutant dies on it.
   The section's **second** mutant — "a disposal that clears a map the fallback then cannot rebuild" — is §4's, cannot
   be written before §4 exists, and stays open there.

## Measurement first — what would close each section as *measurement, not a deliverable*

- **§8 prose:** closes as documentation if the fence provably cannot emit `rc 1` — i.e. the engine-gone branch and the
  port-still-bound branch are mutually exclusive. Measured evidence says they are not.
- **Dead-code:** closes as not-a-defect if every narrowed module's suppression is load-bearing for a documented reason.
  The one audited module is not.
- **All-projects search:** closes as measurement if a row already drives `search` at that scope somewhere in the tree.
  Two greps say none does.
- **`context` fallback:** closes as not-a-defect if the measured population of cache-less sessions is empty once
  re-activation is understood. The measured population is 76 events over 7 sessions, all of them re-activation
  artefacts — which is why the fix order matters, and why the *trigger* is deployment rather than compaction.
- **Activation boundary — CLOSED 2026-11-26 as measurement, by a different route than predicted.** The route written
  here was "not-a-defect if the disposal path cannot run while a session is live", and it is excluded: the measured
  plugin-load count against the session count says it can. What actually closed the section is that the row it would
  have written already existed and its mutant dies on it — so the deliverable is zero lines, and the risk the section
  described was already covered.

## Out of Scope

- **Issue 167** (re-try a local embedding model): the owner decided local embeddings and deferred the model choice.
- **Restaging the built engine** and any production cutover: owner actions.
- **Rewriting history** to remove a previously captured token: owner's call.
- **The shared search widening shape** ruled out by measurement in the search-cap work: superseded, must not return.
- **Restarting the host or the live engine** to test any of the above: forbidden by standing directive; every seam here
  is reachable without one.
- **Retro-fitting the same narrowing to every other crate's suppressions** — the change is scoped to the crate that owns
  the store, where the orphan was found.

## Further Notes

- The five findings share a failure mode this project has now caught seven times: **a check that cannot fail**. The
  mitigation that worked was always the same — name the mutant first, then write the row.
- Two of the five are documentation-level and one is a one-line prose gap; they are in this PRD because filing four
  tickets for four sentences produces four waves and one real fix.
- The measured sizes are recorded in the source issues and are not restated as pass conditions here: no count, pid or
  duration from this PRD may become a literal in a test.
