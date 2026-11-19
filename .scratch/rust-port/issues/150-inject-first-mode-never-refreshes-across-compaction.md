# 150 — `inject_on: "first"` never refreshes the cached block, including across compaction

Status: ready-for-human — 2026-11-19 (rule landed at `4449679`: in `first` mode every 20th prompt refreshes, cost 4 injects per 100 prompts. All 12 criteria reproduce and the reviewer built the four named mutants itself, each red on its named criterion. Owner check remains: a live `first`-mode child's filtered inject indices across prompts 0..20 must be exactly `[0, 20]`.)

*Scope note. Issue 149 owns the content (a `## Recent actions` section) and, by its 2026-11-19 amendment, the one-line
cache invalidation on compaction that 149's own measurement needs. This issue owns the policy: which refresh rule is
right, and whether `first` is the right default. It does not design that rule.*

## The gap

MEASURED 2026-11-19, run record `docs/evidence/issue-145/run-20261118T215648Z.md` (reading `no-inject-after-compaction`).
Live child `ses_e0f5602b3ffeeWae0X4cbfgm7n`:

- one `inject.requested` / `inject.received` pair for the whole session, both before its compaction boundary
  (log lines 04:57:14.708 and 04:57:15.132, local time);
- zero inject events after the boundary (04:58:00.293 local);
- a nonce written after the boundary came back `NONE`; no host DB row carries it except the prompt that asked for it.

Mechanism, each line READ at HEAD:

- `plugin/src/v2/adapter.js:63` — `const hasCache = contexts.has(event.sessionID);`
- `plugin/src/v2/adapter.js:67` — `if (!shouldRefresh({ turnIndex, hasCache })) return;` exits before the cache delete
  (`:70`) and before the `chat.message` call (`:76`). A non-refreshing prompt neither re-injects nor clears the cache.
- `plugin/src/plugin.js:1504-1508` — `shouldRefresh` is defined non-enumerable; the predicate at `:1507` is
  `!hasCache || turnIndex === 0 || bridge().engineInjectOn !== "first"`. With `engineInjectOn` equal to `"first"`, every
  prompt after turn 0 with a cache is non-refreshing, forever. `engineInjectOn` is taken from the engine's hello
  (`plugin.js:1024`).
- `crates/mem-core/src/config/schema.rs:749` — `chat_message_inject_on: InjectOn::First` is the built-in default; asserted
  at `crates/mem-core/tests/config_defaults.rs:122`. READ: `~/.opencode-mem/*.json*` carries no `inject_on` override
  (run record, grep).
- `plugin/src/v2/adapter.js:86-89` — the `context` hook pushes the cached string onto `event.system` on every model step.
- `plugin/src/v2/adapter.js:122` — `// Compaction is mid-turn: retain context until policy refresh/cleanup.` Compaction
  deletes nothing from the cache.

Net: in the default mode a child's cached block is its turn-0 block, replayed on every later step, and nothing in the
compaction path replaces it.

### Known limit — what the compacted child still receives

The child is not left with no memory after compaction. Two things still arrive, and the record must not be read as total
absence:

- the compaction leg is independent of the cache: `plugin/src/v2/adapter.js:97-113` pushes the
  `experimental.session.compacting` output onto `event.system`, and the engine's `include.compaction` render
  (`crates/mem-bridge/src/compaction.rs:66`, `render`) is what that output carries. The log records
  `Compaction memory injected: {"memory_count":1}` at 04:57:45.570 and 04:58:00.293 (REPORTED from the run record);
- the replayed turn-0 block still rides the `context` hook (`adapter.js:86-89`).

So what the child lacks is a **fresh chat block on its next request**. What it has is the turn-0 block plus whatever the
summariser kept in the compaction leg. The gap is staleness, not absence.

## Why it matters, in the owner's terms

The memory block exists so a child continues instead of re-checking. The child that most needs a fresh block is the one
that just compacted and lost detail. In the default mode it is the child guaranteed not to get one. It keeps a frozen
copy of the block from its first turn, which is the turn with the least to say about the work after it.

## Consequence of leaving it

- Any feature that rides the next prompt (149's `## Recent actions`) silently never delivers in the default mode. 149's
  amendment repairs only the compaction-triggered case.
- A long session in the default mode never sees an updated memory block, whether or not it compacts.

## The cost side — what `always` would cost

MEASURED from `~/.opencode-mem/opencode-mem.log`, three `inject.received` lines read in the tail: `handling_ms` 414, 424
and 715. The brief's 414–809 range was not re-derived across the whole log. An `always` mode is one engine inject round
trip per prompt. The log's `inject.requested` lines carry `query_chars` values from small to a 2000 cap (observed by
grep, not pinned). Whether this cost is acceptable at the session's real prompt rate is unmeasured.

## What it would touch

- `plugin/src/v2/adapter.js` — `onPrompt` (`:58-84`, gate at `:67`, delete at `:70`), the `context` hook (`:86-89`), the
  compaction hook (`:97-113`), and the comment at `:122`.
- `plugin/src/plugin.js` — `shouldRefresh` (`:1504-1508`), `engineInjectOn` (`:1024`).
- `crates/mem-core/src/config/schema.rs:749` and the pinned default `crates/mem-core/tests/config_defaults.rs:122`.
- Pinned contracts to respect: `plugin/tests/v2-plugin-adapter.test.mjs:178` ("a session with no cached injection gets
  nothing") and `:397` ("cleanup drops the cached block").

## Known limits of this issue

- It does not decide the refresh rule. Naming the candidate rules is in scope; choosing one is not.
- The compaction-leg content is what the summariser kept, and this issue does not change it.
- The `prompt` hook's behaviour for child sessions is not verified here; see 151.

## Inverted world — what would show `first` is fine and the failure is elsewhere

A post-compaction prompt that logs `inject.requested` with a non-empty block, and whose child quotes a nonce written after
the boundary, would mean the replayed block is not what the child sees on that turn and the fault is elsewhere (the
`context` hook or the host's message persistence). The current tree cannot produce that observation without a change, so
its absence is the present measured reading, not a proof of the mechanism.

## Open questions for the planner

1. Is `first` the intended default, or was it chosen for the cost? The adapter comment (`adapter.js:64-66`) calls
   staleness "the documented first-mode tradeoff". Is that an owner ruling, and if so, where is it recorded?
2. Which signal should invalidate the cache: compaction, a turn count, elapsed time, or a content change? This issue names
   the candidates; the choice is the planner's.
3. What does `always` cost over a real session, measured as the distribution of `handling_ms` across a session's prompts,
   not from three lines?
4. Does the answer differ between root sessions and child sessions? The root case works per 145's record; the child case is
   the failing one.
5. What measurement settles it: N live children per mode, nonce written after the compaction boundary, a count of
   `inject.requested` after the boundary, and the nonce's presence in the answer.
6. Does 149's amendment-line invalidation make this issue's question moot for the compaction case? It does not settle the
   non-compaction staleness question.

Relation to 149, stated once: 149 owns what the block says. This issue owns when the block is refreshed.

## Comments

> *This was generated by AI during triage.*

### Triage notes — 2026-11-19. The default is not obviously wrong; the cost is the argument.

**MEASURED from the live engine log** (`~/.opencode-mem/opencode-mem.log`, window `2026-11-18T00:06:17` →
`2026-11-19T05:34:12`, about 29.5 h, 84 `inject.received` lines with `status:"ok"`):

| quantity | min | p50 | p90 | max | mean |
|---|---|---|---|---|---|
| `handling_ms` | 0 | **774** | 914 | 2858 | 650.8 |
| `block_bytes` | 11 | **6957** | — | 8397 | — |

Read: a refreshing inject costs a **median 774 ms** and delivers a **median 6.9 KB** block. That figure is the reason
`InjectOn::First` exists, and it means "just change the default to `always`" is not a fix — it would add ~0.8 s to every
prompt in every session to re-deliver a block that usually has not changed.

**So the remaining question is narrower than the filing states.** 149's 2026-11-19 amendment already forces a refresh on
the compaction path regardless of mode, which is the case that hurts (the child that just lost detail). What 150 still
owns is the session that runs many turns **without** compacting: it keeps its turn-0 block for its whole life, and on the
cost data above a per-prompt refresh is not the answer.

**Scope for a planner, with the cost as a hard constraint:** design a refresh rule that bounds staleness without paying
774 ms per prompt. The measured envelope to design inside: per-prompt refresh = 774 ms p50 / 914 ms p90; per-compaction
refresh already lands via 149. Anything that fires more often than compaction must justify its cost against that p50.
Rejected in advance as out of scope: changing the built-in default (a product decision that the measurement argues
against), and any change to 149's block content.

**Inverted world.** A design that cannot state, in one sentence, how many extra injects per hundred prompts it adds and
what that costs at 774 ms is not a design. A design whose trigger also fires in `always` mode has misread the issue:
`shouldRefresh` (`plugin/src/plugin.js:1504-1508`) already returns true there, so the only observable change is `first`.

**This does not block issue 149.** 149 needs nothing from this file.
