# 150 — `inject_on: "first"` never refreshes the cached block, including across compaction

Status: needs-triage — 2026-11-19 (filed by architect from a measured delivery gap)

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
