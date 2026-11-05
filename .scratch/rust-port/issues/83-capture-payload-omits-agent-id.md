# 83 — capture payload omits `agent_id`; every production capture dead-letters before either tier is written

Status: done — 2026-11-04 (ALREADY-FIXED — landed in 95857b7; payload.agent_id wired, r2 review-1.md APPROVED)
`.tmp/progress/opsec-audit-20261101T090956Z.md` §S1, and independently re-derived by the orchestrator
against the tree. Work will modify `plugin/src/plugin.js` (payload fields) and
`crates/mem-bridge/src/capture.rs` (field-named log helper), add the `AUTO_CAPTURE_EPISODIC_FAILED`
constant to `crates/mem-core/src/logging/signatures.rs`, and create
`plugin/tests/capture-payload.test.mjs` + `crates/mem-bridge/tests/capture_wrapper_payload.rs` and the
committed fixture `plugin/tests/fixtures/idle-capture-payload.json`.
Blueprint approved: `.tmp/blueprints/blueprint-20261101-1016-capture-agent-id-r4.md` — header
`approval: APPROVED — plan-gate 4, 2026-11-01 (M1/M2 CLOSED and measured)`, internal body hash
`a65a3b8baf3f`. That hash is the blueprint's own header field, **not** `sha256sum` of the file
(`sha256sum` gives `de6d7e032d27…`), so approval is verified from the header line and the plan-gate
record, never by re-hashing the file.)

## What is missing

The OpenCode wrapper never sends `agent_id` on any capture, and the store rejects an empty one.

MEASURED trace:

1. `plugin/src/plugin.js:1187-1194` builds the payload as `{session_id, capture_id, transcript, metadata}`
   plus `project_id` **only when `hostProjectId !== null`**. No `agent_id`, no `agent_name`, no
   `parent_session_id`. `grep -n 'agentId\|agent_id' plugin/src/*.js` returns only `tools.js:57`, an
   unrelated tool-parameter schema; the shipped `plugin/dist/` is byte-identical (`cmp`).
2. `crates/mem-bridge/src/capture.rs:405-411` — `CaptureParams` is `#[serde(default)]` with
   `agent_id: String`, so an absent field deserialises to `""` (`agent_id` at `:409`).
3. `commit_episodic` (`capture.rs:1175-1194`) passes params into `episodic_record` (`capture.rs:1577`)
   with **no default**. Contrast `crates/mem-bridge/src/vault_feeder.rs:174-178`, which *does*
   substitute `"vault-feeder"` — the codebase knows the field can arrive empty; one consumer defends,
   another does not.
4. `crates/mem-episodic/src/store.rs:339` `upsert_session` → `validation.rs:76`
   `check_id("agent_id", …)` → `"must not be empty"` (`validation.rs:47-50`).
5. `capture.rs:793-800` maps that to `Failure::of("episodic_write_failed")` → `retryable_or_dead` →
   `.dead_letter` after five attempts (`capture.rs:1044`), terminal — `SELECT_CAPTURE_INBOX_BATCH`
   only re-reads `status = 'queued'` (`crates/mem-store/src/episodic_sql.rs:227`).

Because the episodic upsert runs **first**, `commit_episodic` returns before the tier-1 write
(`capture.rs:794-812`) — so both the episodic tier and the capture memory are starved.

`validation.rs:74-76` validates `project_id`, then `session_id`, then `agent_id`, and the wrapper omits
`project_id` whenever `input.directory` is absent — so the same failure can name a different field first.

**The audit's one named falsifier is closed.** It asked whether an *installed copy* of the plugin could
differ from this repo's `plugin/`. It cannot: `~/.config/opencode/opencode.jsonc:171-175` loads this
repo's path directly (`"/home/vaz/Code/Personal/Rust/opencode-agents-memory/plugin"`, line 175, past a
commented-out duplicate at line 174) — there is no install step and therefore no second copy to drift.

## What lands

- The wrapper sends `agent_id` (resolved via `readSession` → `session.agent` →
  `resolveSessionAgent`'s fallbacks, `plugin/src/compaction.js:56-83`) and sends `project_id`
  unconditionally.
- A missing host answer **fails closed with a named event** (`capture.agent_unresolved` /
  `capture.project_unresolved`): the capture is not sent, no empty string and no sentinel — a sentinel
  would make every unattributed capture indistinguishable.
- The write failure names the offending field: a `Validation` error logs `field` plus a bounded
  `detail`, truncated with `chars().take(120)` (never a byte slice — that panics on a non-char
  boundary on the drain path).
- `AUTO_CAPTURE_EPISODIC_FAILED` joins `crates/mem-core/src/logging/signatures.rs` as a further
  non-scraped constant (precedent: `AUTO_CAPTURE_PROVIDER_FAILED`, `signatures.rs:108`; the drift guard
  hardcodes exactly nine stats cells in `crates/mem-web/tests/stats_signature_contract.rs`).
- Five artifacts the approved plan names, all of which must land with the payload change:
  - `plugin/dist/plugin.js` — **byte-identical** copy of `plugin/src/plugin.js`. Without it
    `plugin/tests/dist-sync.test.mjs` fails and nothing ships: `plugin/package.json`'s `main` and
    `exports` all point at `dist/`.
  - `plugin/tests/fixtures/idle-capture-payload.json` — the committed, single-field-normalised fixture
    (see issue 91, which owns the harness step).
  - `docs/bridge-schema.md` — the `capture` row gains the identity obligation.
  - `docs/adr/0030-wrapper-supplies-capture-identity.md` — the decision record.
  - `plugin/tests/wrapper-glue.test.mjs` — the driver gains `session.get` so `agent_id` can be resolved.

## Acceptance criteria

- [ ] A test drives the **wrapper's real payload** through the engine and asserts `EntryOutcome::Done`;
      today it returns `Dead` with `episodic_write_failed`. No test may hand-build
      `agent_id: "agent-1"` — that hand-built value is why this shipped.
- [ ] A committed, single-field-normalised fixture produced by the wrapper's own hook is consumed
      verbatim by the Rust side, with a wrapper-side equality test so the fixture cannot drift.
- [ ] An unset `agent_id` still dead-letters (the engine stays strict; the fix is the producer, not a
      default).
- [ ] A `Validation` failure logs the field name and a ≤120-byte detail; the truncation is tested with
      a multi-byte character at the boundary.
- [ ] `project_id` absent and `agent_id` absent each produce their own named `capture.*` event.
- [ ] `plugin/dist/plugin.js` remains byte-identical to `plugin/src/plugin.js`
      (`plugin/tests/dist-sync.test.mjs` green).

## Blocked by

- Issue 84 — a capture never reaches `commit_episodic` until a summary exists.
- Issue 88 — with host-first precedence the host wire is what production dials.

Neither is a substitute for this one: this fires *after* a summary exists. All three must land before
US 1.1's "captures persist" claim is true.

## Parent

Slice issue 92 (`92-slice1-capture-persists-end-to-end.md`).

## Comments

**Live corroboration (MEASURED, read-only on `~/.opencode-mem/opencode-mem.log`, as of
2026-11-01T11:00Z):** **0** `Auto-capture memory persisted`, **168** `dead_lettered`, **4** lines
carrying `"reason":"episodic_write_failed"` — entries that got a usable summary and then failed the
write. At the audit's read earlier the same day the same counters were 0 / 134 / 3; the numbers are
point-in-time, not durable, and only the `reason` field is the claim.

**Why the tests missed it:** every capture test constructs `CaptureParams` by hand with
`agent_id: "agent-1"` (`capture_attribution.rs:23`, `capture_concurrency.rs:55`,
`capture_partial_failure.rs:61`, `capture_raw_context.rs:55`, `capture_retention.rs:23`,
`capture_inject_e2e.rs:129`). None drives the wrapper's payload shape through the engine.

**Ownership vs issue 91:** 91 owns the harness by which the wrapper's payload is recorded and consumed
and the fixture/variance mechanism; this issue owns the payload fields, the fail-closed events and the
five artifacts above. An agent must not build both — see 91's "Blocked by / ownership" section.

[fact] 2026-11-04 — issue 83 r2 reviewer verdict APPROVED, verified on disk (MEASURED).
Artifact: .tmp/progress/issue-83-capture-identity-r2.review-1.md (blueprint 53121ead9062, commit 9c54880, test-only, +14/-0 in crates/mem-bridge/src/capture.rs).
- STATUS: APPROVED; all 13 gates green (re-executed): cargo test -p mem-bridge rc 0 (62 binaries), capture_wrapper_payload 4/4, --lib 17/0, --lib truncation 2/0, capture-payload.test.mjs 21/0, cmp -s plugin/src/plugin.js plugin/dist/plugin.js rc 0, dist-sync-invariant 1/1, cargo fmt --all --check rc 0, both clippy gates rc 0.
- Mutation spot-checks (2/3 cap, isolated scratch worktree): M-byte killed (rc 101), M-slice killed (panics on é at byte 120); baseline GREEN before/after; worktree == HEAD blob post-restore.
- Security gate (10 mandates): 0 violations; PASS input validation / privacy-by-design / error handling / logging hygiene; N/A elsewhere.
- Deviations verified genuine, none gate-affecting: truncate_detail gate-string substitution correct; TMPDIR must be absolute (/home/vaz/Code/Personal/Rust/opencode-agents-memory/...); /tmp tmpfs 54%.
- Related cleanup (this session): removed debris crates/mem-bridge/.tmp/ and plugin/undefined.lifecycle (flagged safe-to-delete in this review's non-blocking notes).
