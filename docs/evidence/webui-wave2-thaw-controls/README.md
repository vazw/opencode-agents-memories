# Issue 118 wave 2 browser evidence (r2 run, 2026-11-05)

All figures below are **MEASURED** unless labelled otherwise, by
`web-ui/scripts/capture-evidence.cjs` against one lane in one run.

## Lane

- Engine: `/home/vaz/Code/Personal/Rust/opencode-agents-memory/target/debug/opencode-agents-memory
  --mode=standalone --storage-root /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/issue118-review-lane2
  --web-port 49392 --web-dist <worktree>/web-ui/dist`.
- UI: release build of this worktree (`cd web-ui && trunk build --release`, 2026-11-05T19:53Z).
- Chrome DevTools MCP over CDP 127.0.0.1:9222.
- Fixture: fresh store — no projects, no skills, no memories. `/api/stats` still returns its
  daily-activity rows, which is why the Stats screenshot has table content.

## Readiness gate (why these numbers differ from the previous run)

`web-ui/scripts/capture-evidence.cjs` navigates each route, then waits until
`[class*=thaw-spinner]` + `[data-testid$="-loading"]` both read 0 for **3 consecutive 100 ms samples**
before counting or screenshotting, and fails the run if `busyAtCapture != 0`. The previous run counted
and captured `/stats` inside its ~3.2 s `/api/stats` window, so it recorded the Loading arm
(`thaw-card = 0`, spinner present) instead of the settled screen. Every row below was re-derived under
the same gate in one pass; the run printed `STEADY-STATE CENSUS OK` with `busyAtCapture = 0` on all
eleven rows.

## Thaw widgets per route (MEASURED, lane 49392, steady state)

| route | thaw-button | thaw-input | thaw-card | thaw-tag | spinner | message-bar | raw `<button>` | `.state-block` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `/` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `/project-memories` | 7 | 2 | 0 | 0 | 0 | 3 | 0 | 1 |
| `/tags` | 2 | 0 | 0 | 0 | 0 | 3 | 0 | 1 |
| `/user-profile` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `/settings` | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `/export-import` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `/skill-library` | 2 | 2 | 0 | 0 | 0 | 3 | 0 | 1 |
| `/sessions` | 2 | 0 | 0 | 0 | 0 | 3 | 0 | 1 |
| `/agents` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `/stats` | 2 | 0 | 4 | 0 | 0 | 0 | 0 | 0 |
| `/skill-library`, form open (**fixture**: empty store, `skill-create` clicked) | 4 | 6 | 1 | 0 | 0 | 3 | 0 | 1 |

- `spinner` and `message-bar` are separate columns because one Thaw `Spinner` yields **4** elements
  whose class contains `thaw-spinner` and one `MessageBar` yields **3** whose class contains
  `thaw-message-bar`. The previous run's combined column hid which widget produced the number and made
  a mid-load snapshot look like a settled MessageBar row.
- `raw <button>` = every `<button>` without a `thaw` class: **0 on all ten routes**, on this fixture.
  Caveat: the blueprint sanctions `<button class="memory-tag">` wrappers for interactive tags, so raw
  buttons appear as soon as a memory carries tags — this fixture has none.
- The `/skill-library` form row is a **fixture**, not a route state: the store has no skills, so the
  settled route shows the empty `MessageBar` (2 buttons / 2 inputs / 3 message-bar) and only the opened
  form adds the `Card`, the four form inputs and the save/cancel buttons. It is not comparable to a
  populated-store row.
- `/agents` on this fixture renders its `agents-no-projects` block (count 1) instead of a roster, so it
  shows one button and no state block.

## StateBlock: `.state-block` now reaches the DOM (RED → GREEN, MEASURED)

`web-ui/scripts/test-state-block-cdp.cjs` (new) drives the real DOM.

- RED, previous release build (the `class={let _ = STATE_BLOCK_CLASS; cls}` tree):
  `FAIL StateBlock container must carry .state-block / false !== true`.
- GREEN, this build: `PASS StateBlock class` with
  `settled = {testid:"memories-empty", className:"state-block state-empty", marginTop:"12px", marginBottom:"12px", stateBlockCount:1}`.
- Side effect on `/agents`: the roster request is held open for 800 ms via CDP `Fetch`, so the Loading
  arm is deterministic — `className:"state-block agents-loading"`, `marginTop:"12px"`. Before the fix
  that container had `state-block` absent and margin `0px`. `.agents-loading` sets no margin of its own;
  `.agents-empty`/`.agents-unavailable`/`.agents-error` keep their `margin-top: 0.75rem` (later rule,
  equal specificity) and gain the shared bottom margin.

Run: `TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/test-state-block-cdp.cjs`.

## Native selects

- `select[data-testid="profile-identity-select"]` = 1 on `/user-profile` (MEASURED).
- `select[data-testid="sessions-project-select"]` = 1 on `/sessions` (MEASURED).
- `select[data-testid="agents-project-select"]` = 0 on `/agents` **with this project-less fixture**: the
  screen renders `agents-no-projects` (count 1) instead, and the select exists only in the
  `ProjectsView::Ready` arm. The element is unchanged: `<select` occurrences are 2/2 in `agents.rs`,
  1/1 in `screens.rs`, 1/1 in `core_screens.rs` (HEAD vs worktree) and `git diff` touches no `<select`
  line. A populated-roster lane measured 1 in the previous run. **Not live-verified this iteration.**

## HTTP 500 from the previous run: cause now INFERRED, not established

The positive half reproduces: the same binary on fresh roots (`.tmp/issue118-baseline-lane` port 49382,
`.tmp/issue118-lane-6` port 49383, `.tmp/issue118-review-lane2` port 49392) answers `/api/stats`,
`/api/memories`, `/api/tags` **200**.

The negative half does not: the root named in the previous report, `.tmp/issue118-test-lane-5`, no longer
exists, so the 500 cannot be re-run; and a fresh root also lacks `metadata.db` and `user-prompts.db`
until the engine writes them, yet answers 200 — which contradicts the "partial store" mechanism as
stated. Keep it labelled INFERRED. `cargo test -p mem-store --test turso_shard_rotation` was 14/14 green
on this tree, and no backend code changed in this wave (`git diff` touches no `crates/`, `src/`,
`Cargo.toml`).

## Skill form: native submission regression (RED → GREEN, MEASURED)

`web-ui/scripts/test-skill-form-cdp.cjs` drives the real form through CDP: valid name/description/
content, Save, then Cancel, then Enter in the name field, asserting one save, no submit event on
Save/Cancel, no URL change, and Enter neither navigating nor saving.

- RED (build before the fix): `AssertionError: Save must not submit form, 1 !== 0`.
- GREEN (after `button_type=ButtonType::Button` on both buttons plus a form `on:submit`
  prevent-default): `PASS Save, Cancel, Enter`.
- Live DOM after the fix: `skill-save.type = "button"`, `skill-cancel.type = "button"`.

Run: `TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/test-skill-form-cdp.cjs`.

## Reproduce

```sh
cd .tmp/worktrees/issue-118
cd web-ui && trunk build --release && cd ..
target=/home/vaz/Code/Personal/Rust/opencode-agents-memory/target/debug/opencode-agents-memory
$target --mode=standalone --storage-root ../issue118-review-lane2 --web-port 49392 --web-dist web-ui/dist
TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/capture-evidence.cjs
TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/test-state-block-cdp.cjs
TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/test-skill-form-cdp.cjs
```
