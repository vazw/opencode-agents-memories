# 34 — W5.1 follow-up · Wrapper sends `project_id = input.directory`

Status: done — 2026-10-21 (landed in W5.3, commit a96a2bc; dist byte-identical; ACs verified 2026-10-21 by grep + cmp — see comment)

**Scheduling:** carried by W5.3 · Step 8, behind its Step 1 fence — do NOT open a separate wave
against this issue. It is listed here so the follow-up is NAMED and traceable, not so it can be
scheduled twice.

## What to build

W5.3 needs ONE `project_id` derivation, shared identically by capture, recapture,
inject and consolidation. The host supplies the project directory in the plugin's
top-level `input.directory`, but the W5.1 wrapper currently discards it
(`void input`, `plugin/src/plugin.js:544-547`), so the bridge receives only
`{session_id, capture_id, transcript}` (`plugin/src/plugin.js:587-591`). The fix
has one intent: capture the host `input.directory` once, as an immutable
`project_id`, and pass it on every `capture` and `inject` call — never from the
transcript or event payload, never a per-hook override, never an empty string.

This is the NAMED, USER-AUTHORIZED (2026-10-21) sequenced cross-lane change to a
file W5.1 owns. It is gated behind W5.1's landing and is carried by W5.3's Step 8
behind its Step 1 fence; it is recorded in W5.3's ADR-0008 as D-W5.3-7 and named
in the issue-21 close-out block. It must never land as a silent edit to a
W5.1-owned file.

## Acceptance criteria

- [x] The wrapper captures the top-level `input.directory` once as `project_id` and sends it on every `capture` and `inject` call. — `plugin/src/plugin.js:555-558` binds `hostProjectId` once (the declaration at `:555` plus its ternary); `grep -n 'hostProjectId'` returns exactly three lines, `:555`, `:626`, `:681`.
- [x] No per-hook, transcript, or event-payload override of `project_id` exists. — no override: `:555` is the only assignment; both sends (`:626`, `:681`) read that binding.
- [x] Absent-param behaviour matches the bridge contract: a missing or blank `project_id` on `capture` → `-32602` before any enqueue; on `inject` → an empty block with no shard query (never `-32602`, never an empty tag). — both halves tested: `crates/mem-bridge/tests/errors.rs:187 test_capture_missing_project_id_returns_invalid_params_without_enqueue` and `errors.rs:329 test_missing_project_scope_writes_nothing`.
- [x] `plugin/dist/plugin.js` is byte-identical to `plugin/src/plugin.js` (`dist-sync.test.mjs` enforces this). — `cmp` exit 0 (2026-10-21); `plugin/tests/dist-sync.test.mjs` enforces it.

## Blocked by

- 19 · W5.1

## Comments

- 2026-10-21 (W6.5-adjacent verification, no code change): `a96a2bc` (W5.3) carries the fixed wrapper. The `project_id` derivation is one immutable binding (`plugin/src/plugin.js:555-558`), consumed by capture (`:626`) and inject (`:681`), with no per-hook path; the two absent-parameter halves are covered by `crates/mem-bridge/tests/errors.rs:187` and `:329`; `plugin/dist/plugin.js` is byte-identical to source (`cmp` exit 0) and `plugin/tests/dist-sync.test.mjs` keeps it that way. All four ACs ticked above.
