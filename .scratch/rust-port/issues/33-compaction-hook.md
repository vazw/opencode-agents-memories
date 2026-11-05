# 33 — W5.5 · Compaction hook: ingest before, restore after (US 1.2)

Status: done — 2026-11-04 (landed 2026-10-27 as merge `f1006f6`; residuals 67 and 68 both done)

## What to build

A **compaction hook** in the plugin: ingest the session's context *before* the host compacts, and restore the durable memory *after*. This is a **parity gap** — upstream opencode-mem ships the post-compaction half and the port does not.

**Upstream's mechanism (verified in the vendored clone, `example/opencode-mem/src/index.ts:1301`):** the `event` hook handles `event.type === "session.compacted"` → `memoryClient.searchMemoriesBySessionID(sessionID, projectTag, CONFIG.compaction.memoryLimit = 10)` (`services/client.ts:397`; the SQL is `WHERE session_id = ? OR (session_id IS NULL AND metadata LIKE ?)`) → `formatMemoriesForCompaction` (`:1425`, `## Restored Session Memory` + `### Memory N` + tags) → re-injected via `ctx.client.session.prompt({parts: [{text, synthetic: true}], noReply: true, agent, model, variant})`.

**Upstream is agent-only, and the user confirmed this as the intended behaviour (2026-10-21).** The mechanism is emergent, not an explicit check: `isChildSession` (`session.get().parentID` non-empty, `:240`) gates the *prompt-save* path (`:674`) and the injection path (`:736`), and `captureChildSessions` defaults to **false** (`config.ts:298`, `:712-715`). Since auto-capture only summarizes prompts already in `userPromptManager`, a child/subagent session never produces a tier-1 memory under its own `session_id`, so the compaction restore finds nothing and returns early (`:1317`). The `session.compacted` handler itself has **no** `isChildSession` check — agent-only behaviour is a consequence of the capture gate.

**Two upstream subtleties the port must preserve (upstream issue #236):** after compaction the host **resets the session to the stock "general-purpose" agent**, so upstream explicitly resolves the pre-compaction agent (`resolveSessionAgent`, `:76` — skipping `info.summary === true` / `info.mode === "compaction"` messages) and the model (`resolveSessionModel`) and passes both. Dropping that would restore the memory into the wrong agent.

**The port's two halves (user ruling, 2026-10-21 — "Both halves"):**

1. **Pre-compaction ingest.** Upstream has **no** pre-compaction ingest path — the pre-compaction state is captured incidentally by `chat.message` + `session.idle`. But OpenCode exposes an unused hook: `experimental.session.compacting` (`@opencode-ai/plugin/dist/index.d.ts:283`) fires **before** compaction starts and lets a plugin append `context` strings to the compaction prompt (or replace the prompt outright). The port uses it to append the session's durable memory — the tier-1 facts keyed by its `session_id` plus the episodic tier's milestone recap — so the memory is digested **into** the compaction summary and survives it.
2. **Post-compaction restore.** `event` → `session.compacted` → re-inject the tier-1 facts by `session_id`, resolving the pre-compaction agent + model explicitly (above).

**The blocker this lane inherits, and its resolution (user ruling, 2026-10-21 — "Give tier-1 facts a session_id"):** upstream's restore is keyed on tier-1 memories' `session_id`. The port has the **schema** (`memories.session_id`, added by `MEMORY_SHARD_V2_ALTER` with the `metadata.sessionID` backfill) and the **query** (`GET_MEMORIES_BY_SESSION_ID`, `crates/mem-store/src/sql.rs:344`) — but **no writer ever sets it**: the promotion path writes `session_id: None` (`crates/mem-episodic/src/promotion.rs:361`), so a naive port of upstream's restore would silently restore nothing. The fix: the **capture-created** tier-1 rows carry the producing session's id (upstream's exact mechanism — capture → `addMemory({sessionID})` → `metadata.sessionID` → `session_id`), while **promoted facts stay session-less and agent-anonymous**, preserving the 2026-10-21 rulings that durable agent attribution lives only on milestones' `contributors`. The two write paths stay distinct.

**Where the fix lands (user ruling, 2026-10-21 — "capture writes both tiers"):** the capture-time tier-1 write is **W5.3's** (issue 21), not this lane's. Upstream's auto-capture wrote tier-1 *only* because upstream had no episodic tier; the port has both, so the port's capture must write the episodic staging row **and** a tier-1 row carrying the producing session's id (upstream's `addMemory({sessionID})` → `metadata.sessionID` → `session_id` mechanism). That is what makes this lane's restore key exist at all. This lane **consumes** the write and adds none of its own.

## Acceptance criteria

- [ ] `experimental.session.compacting` is handled: the session's durable memory is appended to the compaction prompt as additional context, and the port's own injected block is not double-counted in that context.
- [ ] `session.compacted` is handled: the tier-1 facts for that `session_id` are re-injected as a synthetic no-reply message, formatted upstream-exactly (`## Restored Session Memory` / `### Memory N` + tags).
- [ ] The pre-compaction agent and model are resolved and passed explicitly, so the restore lands in the agent that was running — not in the host's stock "general-purpose" fallback (upstream issue #236).
- [ ] **Agent-only, by the capture gate:** a child/subagent session restores nothing (no tier-1 rows carry its `session_id`), matching upstream's emergent behaviour. The behaviour is pinned by a test, not assumed.
- [ ] The restore **consumes** W5.3's capture-time tier-1 write rather than owning it: capture-created tier-1 rows carry the producing session's id (that write is **W5.3's / issue 21's** acceptance criterion — it lands there because upstream's auto-capture wrote tier-1 too, and the port has both tiers); this lane adds **no** writer of `memories.session_id`, and promoted facts stay session-less and agent-anonymous.
- [ ] The port's `compaction.enabled` / `compaction.memory_limit` config knobs (currently parsed by `mem-core` and consumed by **nothing**) drive this lane, with upstream's defaults (`enabled: true`, `memory_limit: 10`).
- [ ] Fail-open: with no engine, no tier-1 rows, or a disabled config, the compaction path is a no-op and never blocks or fails the host's compaction.

## Blocked by

- 19 · W5.1 (the plugin adapter and the `event` hook surface)
- 21 · W5.3 (the capture path that writes the tier-1 `session_id` this lane's restore keys on)

## Out of scope

- Upstream's `session.idle` auto-capture and profile learning — **issues 47 (the ruling) and 48 (the
  erasure leg)**. Corrected 2026-10-25 (47.6): this line previously read "W5.3's lane", a lane that never
  existed. 47.1 ruled the profile tier read-only this release (Leg A = A2), so nothing learns yet.
- The port's own **episodic** consolidation/compaction (W4.3) — a different meaning of the word "compaction": that one ages episodic rows into milestones; this one is about the host's context window.
- Any change to the tier-1 promotion path's agent-anonymity (the 2026-10-21 ruling stands).
- `experimental.compaction.autocontinue` — noted but not required (it can suppress the host's synthetic continue turn; upstream does not use it).

## Comments

## Agent Brief

**Category:** enhancement
**Summary:** handle the host's two compaction events — append durable memory before compaction, restore
tier-1 facts after it.

**Current behavior (verified at HEAD `206cf9f`):**
No handler exists. Neither `session.compacting` nor `session.compacted` appears anywhere in the engine or the
wrapper, and the restore heading upstream uses appears nowhere either; the wrapper's event hook acts only on
`session.idle`. `compaction.enabled` / `compaction.memory_limit` are parsed and resolved by the config layer and
read by **nothing**, and the log signatures for this lane are explicitly marked dormant. The dependency this
lane was blocked on has landed: capture now writes tier-1 rows carrying the producing session's id, while
promoted facts remain session-less.

**Desired behavior:**
Before compaction, append the session's durable memory to the host's compaction prompt as additional context,
without double-counting the port's own injected block. After compaction, re-inject that session's tier-1 facts
as a **single synthetic no-reply message**, formatted exactly as upstream does (`## Restored Session Memory`,
then `### Memory N` with its tags). Resolve the pre-compaction agent and model and pass them explicitly, so the
restore lands in the agent that was running rather than the host's stock general-purpose fallback. Child and
subagent sessions restore nothing — an emergent property of the capture gate, so it must be pinned by a test,
not assumed. Both halves honour the two config knobs with upstream's defaults (`enabled: true`,
`memory_limit: 10`) and fail open: no engine, no rows, or disabled config makes the path a no-op that never
blocks the host.

**Key interfaces:**
- The wrapper's `event` hook surface — the only place a host compaction event can be observed.
- A tier-1 read keyed by `session_id`. The write side already exists; **this lane adds no writer** of
  `memories.session_id`, and it must not touch the promotion path's agent-anonymity.
- The `Compaction { enabled, memory_limit }` config type: these two fields must gain their first production
  readers.
- Upstream's restore formatting and the agent/model resolution ordering (upstream issue #236 is the reason the
  fallback is wrong).

**Acceptance criteria:**
- [ ] `experimental.session.compacting` is handled: the session's durable memory is appended to the compaction
      prompt, and the port's own injected block is not double-counted in that context.
- [ ] `session.compacted` is handled: tier-1 facts for that `session_id` are re-injected as one synthetic
      no-reply message, formatted upstream-exactly.
- [ ] The pre-compaction agent and model are resolved and passed explicitly; the restore does not land in the
      stock general-purpose fallback.
- [ ] A child/subagent session restores nothing, and a test pins that behaviour rather than assuming it.
- [ ] `compaction.enabled` and `compaction.memory_limit` drive the lane, with upstream's defaults.
- [ ] Fail-open: no engine, no tier-1 rows, or disabled config is a no-op that never blocks or fails the
      host's compaction.
- [ ] Neither half writes `memories.session_id`; the capture-time write remains the sole producer.

**Out of scope:**
- The `session.idle` auto-capture and profile learning — issues 47 (ruling) and 48 (erasure leg) own those.
- Episodic consolidation (a different meaning of "compaction"; it ages episodic rows into milestones).
- `experimental.compaction.autocontinue`.
