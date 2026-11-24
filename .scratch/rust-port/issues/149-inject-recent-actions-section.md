# 149 — engine inject block: a `## Recent actions` section (the M1 floor)

Status: done and verified live — 2026-11-24T18:29 local (fix `8f34546`; engine `33b5c73e1a094498` running as pid
`14629`; both post-restart injections carry `## Recent actions` — see "Live verification" at the end. The earlier
"owner render capture" closure condition was superseded by a MEASURED starvation defect, see "Measured 2026-11-24".)

*(Spun out of issue 146 after a grilling session. 146 asked only for a background-shell section; the grilling found the
shell line and the recent-action line are the same data and the same mechanism, so they are one section. `plugin/`,
`crates/mem-bridge/` and `crates/mem-episodic/` all change into it.)*

## Goal, in the owner's words

A child agent, after compaction, must continue from its last action instead of re-checking or re-running what it already
did. It must not forget what it just did, nor which background shell it left running.

## Decisions of record

Each was forced by a measurement, not by taste. The measurement is named with each.

1. **Source of truth: idle-time capture into the engine store.** The plugin wires `chat.message`, `context` and `event`
   (`session.idle`, `session.compacted`) and **no tool hook** — the only `tool.execute` in the tree is the plugin's own
   memory tool. So live shell truth is not reachable from the wrapper. What is already reachable: `parseIdleTranscript`
   parses `toolCalls` (`{name, input}`) and `textResponses`, and `fireCapture` sends them as
   `metadata.autoCaptureContext`. Rejected: a live query at inject time (needs a hook that does not exist) and reading
   the child's own session messages at inject time (fails for exactly the child M1 exists for).

2. **Scope: restore on the next prompt. No new trigger.** After compaction the owner's follow-up turn *is* a
   `chat.message`, so the block regenerates there and the persisted actions ride it. The never-prompted case is out of
   scope. Note for readers: this is **not** M1-as-previously-deferred — that M1 is the `context`-cache fallback in
   `plugin/src/v2/adapter.js:96-101` (`!contexts.has(id) && !attempted.has(id)`), pinned by
   `plugin/tests/v2-plugin-adapter.test.mjs:178`. This issue does not touch that fallback.

3. **A bounded last-actions list is the floor; the summariser's extension is enrichment on top.** The engine already
   stores `files_touched` / `completed_work` / `decisions` / `open_threads` in `SessionMetadata`, and `recent_line`
   (`crates/mem-bridge/src/inject.rs:1237`) renders only `summary` and `open_threads`. But those fields exist only when
   the summariser ran: `Auto-capture skipped: reason "provider_skip"` was measured on a child that read fourteen files,
   and `capture.rs:1795` strips `autoCaptureContext` before persisting. Extension-only would render nothing exactly when
   the provider declines. So: persist a bounded `Vec<String>` of recent actions, and render the extension on top.

4. **The shell line reports the last shell action, never liveness.** The engine cannot see the host's process table, and
   capture happens at `session.idle`. A line reading `bash (background): dev` is a statement about what the agent did; a
   line reading "running shell: dev" would be a guess encoded as fact, and a dead shell told as running is worse than no
   line. A true liveness claim is recorded as a known limit, not promised.

5. **New `## Recent actions` heading, current session only.** Heading assertions in the tree use `contains`
   (`crates/mem-bridge/tests/capture_inject_e2e.rs:312`, `capture_tier1.rs:380`, `inject_budget.rs:387,497,509`), so a
   fifth heading costs no test churn. Folding the actions into `recent_line` would repeat five action lines on every row
   of `## Recent sessions` and bury the agent's own position in other agents' history.

6. **First in the greedy budget's tier order, ahead of `## Project facts`.** The assembler (`inject.rs:396` onward) runs
   `greedy(candidates, budget_bytes, tier1_cap)` over `facts → summaries → recent → older` and sets `truncated`. Actions
   are the most perishable content in the block — worthless one turn late — while facts survive to the next turn, so
   under pressure facts lose a turn and actions lose everything.

7. **On `skip`, write a deterministic row built from `toolCalls` — no provider call.** `CaptureSummaryOutput::Skip`
   currently writes **neither** tier, so the floor would vanish for exactly the children it targets. The skip contract's
   intent is "do not spend a provider call summarising trivia", not "forget the session happened". No model output means
   no hallucination risk.

8. **Acceptance demands provenance.** The child must quote the section line it learned from, verbatim, and the quote
   must match the rendered section. Grounds: a `TRACE-4410` marker planted before a real three-compaction run survived
   inside the compaction summaries (`seq 326`, `421`), so a plain correct answer proves nothing about the block.
   MEASURED 2026-11-19.

9. **Issue 146 closes as covered** when this lands. The shell need is "tell a resumed agent which shell it left running";
   the action line meets it without a liveness claim. Liveness stays recorded as a limit on 146.

## Left to the blueprint (not decided here)

- Whether a skip-path deterministic row carries an embedding, given it would otherwise enter `## Captured session
  summaries` and compete with curated rows for budget.
- The marker that keeps deterministic rows from displacing curated ones.
- Exact field shape and cap for the actions list; one line per action, and how a shell tool call is labelled.
- Budget interplay: the section is an ordinary candidate in the greedy pass (decision 6), so `truncated` reports its
  eviction. No reserved floor.

## Acceptance

The post-restart runbook `docs/evidence/issue-145/README.md` (post-issue-148 rewrite) reads this directly:
`stale` is the predicted reading **before** this lands; after it lands, a child that quotes the actions section line is
`recent-action-delivered`, and `stale` becomes a real failure. The inverted world of decision 6 is a `truncated` block
that still carries the section — which would mean the tier order was not applied.

Unit-level: the section renders when the summariser skipped, and does not render for a session with no captured row.

## Amendment 2026-11-19 — Decision 2's premise is falsified by measurement; one `plugin/` change is now forced

Measured during the issue-145 baseline run (`docs/evidence/issue-145/run-20261118T215648Z.md`, reading
`no-inject-after-compaction`). Live child `ses_e0f5602b3ffeeWae0X4cbfgm7n`: **one** `inject.requested`/
`inject.received` pair for the whole session, both before its compaction boundary; **zero** after it. A nonce
written after the boundary came back `NONE`, and no row in the host DB carries it except my own prompt.

Mechanism, each line READ at HEAD:

- `plugin/src/v2/adapter.js:67` — `if (!shouldRefresh({ turnIndex, hasCache })) return;` returns *before* the
  cache delete at `:70` and before `chat.message` at `:76`.
- `plugin/src/plugin.js:1507` — `!hasCache || turnIndex === 0 || bridge().engineInjectOn !== "first"`.
- `crates/mem-core/src/config/schema.rs:749` — `chat_message_inject_on: InjectOn::First` is the built-in
  default, asserted at `crates/mem-core/tests/config_defaults.rs:122`; no override in `~/.opencode-mem/*.json*`.

So in `first` mode a session caches its turn-0 block and the `context` hook (`adapter.js:86-89`) replays that
same block on every later turn. Compaction deletes nothing — `adapter.js:122` says "retain context until policy
refresh/cleanup". There is no next-prompt refresh for the actions to ride: **as designed, this section would be
captured, stored, and never delivered to a compacted child.**

**Decision 2 stands as intent and gains a precondition.** Restore still happens on the next prompt and still
needs no new trigger — but only if the cache is invalidated when the session compacts, so that prompt is a
refreshing one. Required change, now in scope for this work:

- In the `compaction` hook, where `event?.sessionID` is already read (`plugin/src/v2/adapter.js:97-113`, `:102`),
  delete that session's cached block. One line.
- Pinned contracts are unaffected: `plugin/tests/v2-plugin-adapter.test.mjs:178` ("a session with no cached
  injection gets nothing") and `:397` ("cleanup drops the cached block") both describe an empty cache, which is
  what invalidation produces.
- New criterion: a session that compacts and then takes another prompt logs a second `inject.requested` pair
  carrying the actions section. It fails today, because it logs none.
- Not done: M1's never-prompted case. An empty cache still means the `context` hook injects nothing until a
  prompt arrives — that is the pinned contract and this change does not relax it.

**Second correction, same cause:** the blueprint r1 claim "zero `plugin/` change" is no longer true, and
`plugin/dist/` byte-identity now applies to `adapter.js` too. The reviewer should treat that as part of the
scope rather than as an untracked edit.

**Inverted world for this amendment:** if a post-compaction turn still logs no `inject.requested` after the
invalidation lands, the child is seeing the `context`-hook replay rather than a refresh, and the section belongs
on the compaction leg (the `experimental.session.compacting` output pushed onto `event.system` at
`adapter.js:99-106`) instead of the prompt path.

---

## Measured 2026-11-24 — the section is starved, not missing

Read after the host restart at `13:50:17` with `OPENCODE_MEM_DEBUG_INJECT` live, so the block's own heading list is now
logged on every injection. This is the reading the issue was waiting on, and it is negative.

**The log.** Across every flag-on `inject.received` in `~/.opencode-mem/opencode-mem.log`, the distinct `sections`
arrays seen are:

```
sections=["Recent sessions"]
sections=["Captured session summaries","Recent sessions"]
sections=["Project facts","Captured session summaries","Recent sessions"]
sections=["Captured session summaries"]
```

Three headings total, and `Recent actions` appears in none of them. The inject builder declares five
(`crates/mem-bridge/src/inject.rs:87-91`: `Project facts`, `Captured session summaries`, `Recent sessions`,
`Related older sessions`, `Recent actions`), so two of the five never render.

**Why the heading is dropped.** `action_candidates` (`:1198`) maps `row.metadata.recent_actions` into candidates, so an
empty list means no heading. The row it reads is chosen at `:492-501`: the requesting session's own row, but only from
`recent_raw` — the newest `2 × recent_count` project rows — and only when it clears the age cutoff at `:497`. The
`ponytail:` note at `:490` already names the upgrade: "a dedicated session-keyed read would lift the row-window ceiling".

**The data is there.** Read-only on `~/.opencode-mem/data/episodic.db` (`immutable=1`):

```
agent_sessions rows:                          1369
rows with non-empty recentActions:             115   (83 of them at the 8-line cap)
this session's own row:                          1   recentActions = 5
rows newer than that row:                      482
```

So the capture path is alive and this session has five recorded actions — and it sits 482 rows outside the window,
because every gate, coder and reviewer subagent writes a newer row. The absence is starvation, not an empty-data
omission and not a missing implementation.

**Second ceiling, same wave.** The age cutoff at `:497` is independent of the window. Fixing the window alone could
still leave this row dropped, so both are settled together, and the wave reports which one bit.

## The fix

A session-keyed read by `(project_id, session_id)` instead of a scan of `recent_raw`, so a session's own actions are
found regardless of how many rows its subagents wrote. `idx_agent_sessions_session_id` exists, so the read is indexed;
the window was a micro-optimisation guarding against a read the schema already serves. The age cutoff is decided in the
same wave, with its behaviour stated rather than inherited.

Deployment note for whoever plans it: this is `crates/mem-bridge`, so it needs an engine rebuild and
`plugin/scripts/stage-engine.sh`, then a host restart. It cannot ride a plugin-only reload — unlike issues 152, 151 and
155, which are `plugin/src/v2/adapter.js` changes. Do not batch it into a plugin-only restart and then read its absence
as a failure.

**Inverted world.** If the session-keyed read lands and a session with recorded actions still logs no `Recent actions`
heading, the omission is downstream of row selection — check the `max_memories` exemption at `:684`, the row-level
`seen` dedupe, and the `included.is_empty()` early return at `:692` — and this issue is reopened on that finding rather
than on the window.

**What this unblocks.** Issue 156 (recent-action lines losing the tool name) can only be observed when the section
renders, so 156 shares this blocker and closes after it.

## Landed 2026-11-24T09:43Z — the section is no longer starved
`Recent actions` reads the requesting session's own row by `(project_id, session_id)`; the `2 x recent_count` window and the age cutoff no longer apply to it. The eviction and age-ceiling tests were replaced by `test_actions_render_for_own_row_beyond_the_project_window` and `test_actions_render_for_an_old_session_resumed_days_later`; the anti-vacuity test is unchanged.
Engine staged sha256 33b5c73e1a094498. Owner: rerun `SETUP` after the host restart and require a `sections` reading carrying `Recent actions`.

## Review outcome — APPROVED (round 2)

`review-1` was `REJECTED` on two surviving mutants, not on the code: the comparator (`max_by`→`min_by`) and the
dropped `deleted_at IS NULL` predicate both lived through the whole suite because every fixture used the correct
implementation's happy path — one carrying row, distinct ticks, freshly inserted rows, one project, no tombstones.
Bounded contexts `inject.rs` / `episodic_sql.rs` were measured correct; the defect was oracle coverage.

`debug-1` widened the sweep and found five survivors of that one class (M1, M2 tie-break, M3 `updated_at`, M5
`flat_map`, M12 project predicate, M13 soft delete, M18 reversed tie-break). Prescription was test-only: five tests
added, production sources untouched. It also measured that the review's own proposed remedy — merging the
two-carrying case into `test_actions_come_from_the_newest_row_that_has_them` — would have traded one survivor for two,
by deleting the only fixture where the newest row carries no actions.

Two mutants remain accepted without a test, stated rather than silent: the read-side `MAX_RECENT_ACTIONS` clamp is
unreachable (validation rejects a ninth action, capture truncates at eight), and the `ORDER BY` in the session-keyed
read is equivalent for its only consumer, which is order-independent over a unique comparator.

`review-2` re-ran the mutants on its own throwaway worktree: all seven RED as assertion panics naming the right
tests, M4/M8 controls still red, gates green, production hunks byte-identical to wave 1.

**One step remains, and it is the owner's:** restart the host so the staged engine (`33b5c73e1a094498`) loads, then
read one `inject.received` line for `sections` containing `Recent actions`. That reading also unblocks issue 156,
whose tool-name defect can only be observed once the section renders.

## Live verification — the section renders (2026-11-24T18:29 local)

Owner restarted at `18:29:15`. Host `14558`, engine `14629`, and `/proc/14629/exe` hashes to `33b5c73e1a094498` — the staged
binary that carries this fix.

The reading the issue was blocked on, from `~/.opencode-mem/opencode-mem.log`:

```
line 918  18:29:24  session ses_e9d5dd698f (this session, the witness)
          block_prefix: "<memory_context>\n…<project_agent_session_history>## Recent actions\nshell:…"
line 920  18:29:34  session ses_e8839ef3ff
          block_prefix: "…<project_agent_session_history>## Recent actions\nshell:…"
```

Both post-restart injections begin their history body with `## Recent actions`, and the lines under it are `shell:` actions.
The witness session is the one measured starved before the fix — its own row carried five actions behind 482 newer rows —
so this is the fix working on the exact input that defeated the windowed read.

Five readings in total carry `Recent actions` as their first heading. Three of them (`14:09`, `14:37`, `15:22`, session
`ses_df3bff192f`) predate the restart and came from the old windowed engine: that session is short-lived, so its own row
was inside the window. They do not weaken the starvation finding; they show the section always worked for a session that
never spawned subagents, which is exactly what the defect description said.

**One caveat, filed as issue 161:** the `sections` array on those same lines does *not* list `Recent actions`. The block
glues the first heading to the open tag (`inject.rs:1080` pushes `HISTORY_BLOCK_OPEN` then the heading with no newline),
and the probe's scanner is line-anchored, so it drops the first heading in all 30 flag-on readings. The block is the
authority here, not the array; read `block_prefix`.

**Issue 156 is now unblocked** — the tool-name question can be asked of a rendered section.
