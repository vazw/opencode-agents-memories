# 100 — `docs/bridge-schema.md:98` advertises `agent_id?`/`project_id?` as optional on `capture`, but both dead-letter when empty

Status: ready-for-agent — 2026-11-01. Work will edit the `capture` row of `docs/bridge-schema.md` and add
a row to `crates/mem-bridge/tests/capture_wrapper_payload.rs` (created by issues 91/83) or a sibling if
that file lands later. Documentation plus one test row; no wire change.

## What to build

The published schema and the enforcing code disagree about which `capture` params are optional, and the
disagreement is invisible: the doc's reader sends a "valid" capture, the engine rejects it after five
attempts, and the only trace is `episodic_write_failed`.

MEASURED:

- `docs/bridge-schema.md:98` (the `capture` row): `session_id` (req), `capture_id` (req), `project_id?`,
  `agent_id?`, `agent_name?`, `parent_session_id?`, `transcript?`, `summary?`, `metadata?`.
- The engine requires all three identity fields. `CaptureParams` declares `project_id: String` and
  `agent_id: String` (non-`Option`) under `#[serde(default)]`
  (`crates/mem-bridge/src/capture.rs:405-411`), so an absent field becomes `""` and then:
  - empty `project_id` → scope resolution fails first, at `capture.rs:762-768`
    (`Failure::of("scope_unresolved")`);
  - empty `agent_id` → `validate` refuses at `crates/mem-episodic/src/validation.rs:76`, via
    `check_id("agent_id", …)` → `"must not be empty"` (`:47-50`).
  Either way the entry retries five times and dead-letters (`capture.rs:1044`) — terminal, because
  `SELECT_CAPTURE_INBOX_BATCH` only re-reads `status = 'queued'`
  (`crates/mem-store/src/episodic_sql.rs:227`).
- The wrapper is the producer that omits them (issue 83). The doc is the reason a reader would not know.

This is **not** a re-open of issue 76 or 73. Those repaired the prose bullets beneath the table (`:98-103`
as they read then); this is the table **row's** optionality marks, untouched by either.

## Acceptance criteria

- [ ] The `capture` row marks `project_id` and `agent_id` as required (or marks them optional and carries
      the producer obligation in one sentence), and says what happens when they are empty: a named
      dead-letter after five attempts, not a `-32602` — a wire rejection and a terminal write failure are
      different promises and the doc must not blur them.
- [ ] One test row asserts the documented contract in both directions: the full documented request set is
      accepted, and a request with an empty `agent_id` reaches the same named dead-letter the doc
      describes.
- [ ] `docs/bridge-schema.md` and `docs/adr/0030-wrapper-supplies-capture-identity.md` (issue 83) use the
      same wording for the obligation — two docs disagreeing is how this row went stale.

## Blocked by

None for the doc edit. The test row in (2) is cheapest once issue 91's harness exists; until then it can
be written against a hand-built `CaptureParams` **only** if the row says so, because hand-built params are
exactly what hid the defect (issue 83).

## Evidence

`docs/bridge-schema.md:98`; `crates/mem-bridge/src/capture.rs:405-411`, `:762-768`, `:1044`;
`crates/mem-episodic/src/validation.rs:47-50`, `:74-76`; `crates/mem-store/src/episodic_sql.rs:227`;
issues 73 and 76 (the adjacent, already-closed doc repairs). Found by the 2026-11-01 issue-set audit as a
finding with no owning issue.
