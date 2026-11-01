# 91 — Slice 0 (prefactor): extend the existing stub-engine harness so a wrapper→engine payload defect cannot ship green

Status: ready-for-agent — 2026-11-01 (work will extend `plugin/tests/wrapper-glue.test.mjs`'s driver
harness, commit `plugin/tests/fixtures/idle-capture-payload.json`, create
`plugin/tests/capture-payload.test.mjs` and `crates/mem-bridge/tests/capture_wrapper_payload.rs`, and
re-copy `plugin/dist/plugin.js`. No production behaviour changes.)

## What to build

A test seam that crosses the boundary where this project's defects keep surviving — **not** from
scratch. `plugin/tests/wrapper-glue.test.mjs` already stages a temp package, installs a node stub engine,
calls `plugin.server()`, fires the hooks and logs the wire NDJSON: `runGlueTest` at `:234` writes
`DRIVER_SOURCE` (`:132-180`) to disk, spawns it, and the tests read back `{"method","params"}` lines
(`:439-446`). The idle case already exercises the real hook end to end
(`test_wrapper_captures_session_idle_with_project_id`, `:449-480`).

So two pieces are genuinely missing, and only two:

1. **A required-field assertion.** The harness records the payload and asserts specific values
   (`project_id == input.directory` at `:446`, `autoCaptureContext` contents at `:467-475`) but never
   asserts that the fields the **engine requires** are present. That is why an omitted `agent_id`
   dead-letters every capture in production (issue 83) with a green suite.
2. **A committed fixture plus its consumer.** Today every capture test constructs the request by hand —
   `agent_id: "agent-1"`, a `project_id` the wrapper may not send, a transcript invented at the call
   site — so the wrapper's own shape never reaches the engine.

The fixture and how a Rust test resolves it:

- path: `plugin/tests/fixtures/idle-capture-payload.json` (created by this issue, consumed verbatim);
- mechanism: `include_str!` with a path relative to the **test file's own directory**, e.g. in
  `crates/mem-bridge/tests/capture_wrapper_payload.rs`:
  `const WRAPPER_PAYLOAD: &str = include_str!("../../../plugin/tests/fixtures/idle-capture-payload.json");`
  There is **no in-tree precedent** for reaching into `plugin/` from a Rust test — `tests/fixtures/` is
  the existing Rust-side convention, and `plugin/tests/fixtures/recall-tool.json` is wrapper-side only.
  The path is the whole mechanism, so the test must state it: `include_str!` is compile-time, so a moved
  fixture is a build error rather than a silent skip.

The fixture may be normalised in exactly one field (a generated id), and that normalisation must be
stated in the file itself (a sibling `"$comment"` key or a wrapper-side header), so a reader can tell a
fixture from a recording.

## Acceptance criteria

- [ ] A test drives the real hook and records the emitted wire payload; no capture payload in the repo's
      tests is hand-constructed for this path.
- [ ] The committed fixture is consumed verbatim by a Rust test through `include_str!`, and that test
      fails **today**, naming the required field the wrapper omits (`agent_id`), not a generic
      deserialisation error.
- [ ] An equality test on the wrapper side fails if the fixture and a freshly recorded payload diverge.
- [ ] Production paths are untouched — no behaviour change, no new dependency, no schema or wire change.
      Only the harness, the fixture and the new test files change.
- [ ] The harness can express a negative case (a payload missing one required field) without editing the
      fixture, so later slices can reuse it.
- [ ] `plugin/dist/plugin.js` is regenerated after any `plugin/src/plugin.js` edit — the host loads
      `dist` (`plugin/package.json` `main`/`exports`), and `plugin/tests/dist-sync.test.mjs` asserts
      byte-identity. For the harness-only change in (1) no `src` edit is expected, so this step should be
      a no-op; if it is not, the change was not harness-only.

## Blocked by

None — can start immediately.

## Ownership split with issue 83 (read this before starting)

Issue 83's approved blueprint (`.tmp/blueprints/blueprint-20261101-1016-capture-agent-id-r4.md`,
`approval: APPROVED — plan-gate 4`) names **the same three artifacts** this issue names — the fixture,
the wrapper-side test and the Rust consumer test. Two agents must not build both. Split:

- **This issue owns the harness mechanics:** the fixture and its stated single-field normalisation, the
  wrapper-side recorder/equality test, the Rust consumer via `include_str!`, the required-field assertion
  and the negative-case mechanism.
- **Issue 83 owns production behaviour plus its documents:** the payload fields in
  `plugin/src/plugin.js`, the fail-closed `capture.*_unresolved` events, the named `Validation` failure,
  the `AUTO_CAPTURE_EPISODIC_FAILED` constant, the `plugin/dist/plugin.js` copy,
  `docs/bridge-schema.md`'s capture row, `docs/adr/0030-wrapper-supplies-capture-identity.md`, and the
  `session.get` driver change in `wrapper-glue.test.mjs` that lets `agent_id` be resolved at all.

Sequencing: if this issue lands first, 83 consumes its fixture and does not re-write those three files.
If 83's wave lands first (its blueprint is already approved and its implementation is in flight), this
issue's remaining scope shrinks to the two pieces nothing else has built — the required-field assertion
(1) and the negative-case mechanism — and the duplicate rows are **not** written.

## Evidence

Issue 83 (the omitted `agent_id`), issue 84 (the dropped schema), issue 87 (the globals race): all three
survived because no test crossed this seam. Audit: `.tmp/progress/opsec-audit-20261101T090956Z.md` §S1.
