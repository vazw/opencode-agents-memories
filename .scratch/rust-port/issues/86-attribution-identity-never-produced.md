# 86 — attribution identity is never produced: `parent_session_id` unset, child captures ungated, `inject.agent_id` unread

Status: ready-for-agent — 2026-11-01 (premise MEASURED by the `opsec` audit §S5 + §S6 and re-derived
against the tree. Work will modify `plugin/src/plugin.js` (payload + `isChildSession` use on the capture
path), `crates/mem-bridge/src/inject.rs` (agent filter) and `crates/mem-bridge/src/capture.rs`
(`resolve_parent_row_id` consumers), plus new rows in `crates/mem-bridge/tests/capture_attribution.rs`.
Related: issues 16, 33, 63.)

## What is missing

Three parts of US 1.3's attribution story are inert, and one of them is actively wrong.

**Part 1 — `parent_session_id` is never sent; no producer ever sets the field.** `plugin/src/` contains
zero occurrences. The only producer in the tree is a test helper: `fn child(...)` at
`crates/mem-bridge/tests/capture_attribution.rs:33`, whose body sets
`parent_session_id: Some(parent_wire.to_string())` at `:35` (`..params(...)`).

Be precise about what *is* live downstream — an earlier revision of this issue overstated it. The field
is read: `agent_sessions.parent_session_id` is a real column with `ON DELETE SET NULL`
(`crates/mem-episodic/src/record.rs:77-80`), `resolve_parent_row_id` (`capture.rs:1228-1258`, doc from
`:1222`) resolves a wire parent id to a row id whenever the field is present, and
`MARK_CHILDREN_ORPHANED` runs on every soft delete (`crates/mem-episodic/src/store.rs:577,800`;
`consolidation.rs:530`; `purge.rs:186`). The accurate claim is narrower and sufficient: **no producer
ever sets the field**, so in production the whole path never receives an input, and the orphan-flag
machinery has nothing to mark.

**Part 2 — the child gate exists in the wrapper but not on this path.** `plugin/src/plugin.js:998` and
`:1074` *do* call `isChildSession(session)` — on the two **compaction** legs only. `chat.message` and the
idle capture never do. Upstream gates both (`example/opencode-mem/src/index.ts:674-676`, `:735-740`).
Result: subagent/`task` sessions are captured into the parent's project journal and injected into their
own context, in the default configuration.

**Part 3 — `inject.agent_id` is advertised and unread.** `docs/bridge-schema.md:99` lists it;
`inject.rs:135` declares it; nothing reads it (`params.agent_id` never appears outside the declaration;
the other `agent_id` references render `row.agent_id`). So "recall its own prior work or other agents'
work" is not expressible on the chat path.

## What lands

- The wrapper sends `parent_session_id` for child sessions and applies `isChildSession` on the capture
  path (default `captureChildSessions = false`, per upstream `config.ts:312-318`).
- `inject` honours `agent_id` when supplied, or the parameter is removed from the wire and the schema —
  either is acceptable; advertising an unread field is not.
- A child capture is either not written, or written with a resolvable parent. Never written as a
  top-level row that pretends to be the human's work.
- **Cost of the gate, stated up front:** the idle path makes no `session.get` call today — it calls
  `client.session.messages` only (`plugin/src/plugin.js:1153-1175`). Applying `isChildSession` there
  costs **one extra host round-trip per idle event**. `readSession(client, sessionID)`
  (`plugin/src/compaction.js:31-40`) fails open: it returns `null` when `session.get` is not a function,
  when the call throws, or when the response is not an object. The capture path must therefore treat
  `null` as "child status unknown" explicitly — inherit the parent gate's choice (the compaction legs
  read `null` as *not* a child and proceed), or fail closed with a named event. Silence is not an option,
  because `null` also covers a host that has no `session.get` at all.

## Acceptance criteria

- [ ] A child session's capture is skipped by default; with `capture_child_sessions = true` it is
      written **with** a non-empty `parent_session_id` that resolves.
- [ ] No row can exist with `parent_session_id` set to a value `resolve_parent_row_id` cannot resolve
      into a visible parent. The `NotFound { entity: "parent_session" }` path
      (`capture.rs:1246-1250`) must be a named, tested dead-letter at the **capture** level: no such test
      exists today (`crates/mem-episodic/tests/orphan_linkage.rs` tests the store, not the capture
      service), so the ordering risk is currently unpinned rather than handled.
- [ ] `inject` with `agent_id` set either filters by it (tested) or the key is gone from
      `docs/bridge-schema.md`.
- [ ] The `recent_line` render (`inject.rs:999`, used at `:378`/`:413`) has no leading `": "` after
      issue 83 lands.
- [ ] The `null` return of `readSession` is handled by name: the chosen reading (not-a-child, or a named
      fail-closed event) is asserted by a row.

## Blocked by

- Issue 83 — a parent row must exist before a child row's `parent_session_id` can resolve to it.

## Parent

Slice issue 94 (`94-slice3-switches-and-attribution.md`).

## Comments

**Interaction with issue 63:** 63 lists `capture_child_sessions` among the six resolved-but-unread keys
and records that issue 33 treats the child gate as an *emergent property* of capture. This issue supplies
the missing producer half; 63 supplies the switch. Do not close either alone.

**Risk recorded:** deferring the parent id was considered during blueprint work because
`resolve_parent_row_id` returning `NotFound` introduces a new dead-letter mode whose reachability depends
on parent/child drain ordering. That risk is real but does not justify leaving child rows unattributed —
the deferral must be an explicit, tested state, not silence.
