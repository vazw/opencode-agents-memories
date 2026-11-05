# 85 — `inject` discards the session identity it is handed; `excludeCurrentSession` is unimplementable, not merely unwired

Status: done — 2026-11-04 (landed in commit 4198a00; InjectParams exclude_session wired, inject_session_exclusion.rs and inject_chat_gates.rs pass, inject-query.test.mjs pass)

## What is missing

The wrapper stamps `session_id` on **every** `inject` call (`plugin/src/plugin.js:1133-1139`:
`injectParams` builds `{ session_id: currentSessionId() }` at `:1134`), and the engine declares it at
`crates/mem-bridge/src/inject.rs:147` — the field's own doc comment, `inject.rs:145-147`, already
concedes the defect:

```
    /// The hook's own session id. The compaction leg keys the tier-1 read on
    /// it; every other path ignores it (the field is additive and optional).
    pub session_id: Option<String>,
```

Nobody reads it outside `compaction_leg` (`inject.rs:596-607`). The ordinary chat path discards the
identity it was handed. (`inject.rs:135` is `agent_id`, a different field — an earlier revision of this
issue cited it by mistake.)

Consequence: `excludeCurrentSession` (upstream default **true**) cannot be implemented by adding a gate,
because the value it needs is thrown away before the gate could read it. Today the current session's own
capture summaries are eligible for re-injection into that same session, and with the V2 adapter
refreshing the block every turn (`plugin/src/v2/adapter.js:37-39` deletes the cached context on every
fresh prompt, unlike upstream `adapter.ts:162` which refreshes only when `refreshOnPrompt()`), the
model's reference context drifts mid-conversation toward its own outputs.

## What lands

- `assemble` carries the session identity through to the selection stage on the chat path, not just on
  the compaction leg.
- `exclude_current_session` becomes implementable and is implemented here or in issue 63 (this issue
  owns the plumbing; 63 owns the switch).
- A row that injects while the current session has a summarised capture in range, asserting that row is
  excluded.
- **Both** row classes named in AC 1 are gated. The two stores key session identity differently:
  - tier-1 capture memories carry it in JSON metadata — `metadata.sessionID`, built as a typed struct at
    `crates/mem-bridge/src/capture.rs:312-337` (the rename is at `:327-328`);
  - `agent_sessions` rows (the episodic tier the profile/journal paths read) key it on the **column**
    `session_id` (`crates/mem-episodic/src/record.rs:77`).
  A gate written against only the first class passes its test while every episodic row still leaks.

Out of scope here: the `plugin/src/v2/adapter.js` / `plugin/dist/v2/adapter.js` refresh behaviour. It is
what makes the drift visible, but it is a separate defect and is currently owned by issue 99.

## Acceptance criteria

- [ ] A chat-path `inject` with `session_id = S` never renders a memory whose `metadata.sessionID == S`
      when `exclude_current_session` is true (the default) — **and** never renders an `agent_sessions`
      row whose `session_id` column equals `S`. The row asserts both classes; one alone is not a pass.
- [ ] The compaction leg's existing use of `session_id` is unchanged — its tests stay byte-identical.
- [ ] With the flag false, the same memory **is** rendered — proving the gate is a gate, not a filter
      that always runs.
- [ ] Issue 63's premise table can drop its `exclude_current_session` row once this lands.

## Parent

Slice issue 94 (`94-slice3-switches-and-attribution.md`).

## Comments

**Why the tests missed it:** `inject_budget.rs` and siblings construct `InjectParams` directly and none
sets `session_id` for the chat path; the compaction tests do, and that is the leg that works. This is the
same shape as issues 83 and 84: a field populated by one layer, consumed on one path, silently dropped on
the other.

**Sequencing:** this must land before or with issue 63. Doing 63 first would wire a switch that cannot
be honoured.

## r3 progress (2026-11-04)

`InjectParams` now has top-level `exclude_session`; chat requests use the hook's `sessionID` and engine policy. Recent and tier-1 rows
are filtered before the cap. Tests: `inject_session_exclusion.rs`, `inject_chat_gates.rs`, `inject-query.test.mjs`.
