# Issue 118 wave 6 — search row shape, stats card parity, errors badge + popover

Blueprint `20261117-1002-issue-118-wave6-search-and-stats-r7` (stamp `sha256=77b5028db9ab`,
SYS-APPROVED 2026-11-17T10:26Z). Executed on wave 5's landed commit `70fde7e`, clean tree.
**Bundle id: `web-ui-a0b6f95350a233c6`** (trunk `wasm-release`, served bytes sha-verified against
disk by every CDP run's provenance block). Nothing is committed; landing happens after review.

## Measured base (Step 1, before any wave-6 edit)

```
base: emitted=189 hooks=189 sites=230 tests=255 summary=1 commit=70fde7e8bedb3e4d1f8e91b9dc979d4708bb7f34
```

Measured with the blueprint's scoped block, exit 0. Matches the rebase facts exactly, so the
r7 predictions held as written: expectations `E+2/H+2/S+2`, `T+9`.

## RED → GREEN

**Pristine RED sweep (all 22 rows' CDP half).** The wave-6 lanes ran against the wave-5
`70fde7e` bundle (`.tmp/scratch/issue118-w6/dist-pristine`): **13/13 FAIL, exit 1**
(`.tmp/scratch/issue118-w6/red-pristine/results.json`). Failure signatures matched the
blueprint's predictions exactly:

| lane | pristine failure | blueprint's prediction |
| --- | --- | --- |
| `search_field_is_bounded` | field **719.83** > 360 | 719.83 in a 957 px scrollbar-bearing panel |
| `search_debounce_one_request` | 0 search requests | today 0 |
| `search_clear_affordance_is_reactive` | `0 -> 0 -> 0` | today `0 -> 0 -> 0` |
| `search_clear_slot_is_reserved` (M1) | no clear button at the typed precondition | today red at the button-count precondition |
| `search_clear_restores_list` | missing `[data-testid=memories-search-clear]` | (same precondition) |
| `search_result_reflow_pinned` | 0 search requests | today fails the count (0) |
| `stats_cards_equal_height_top` | tops spread **70.77** | measured 70.76 on the r7 fixture |
| `stats_generated_single_line` (M2) | 2 lines, content box **183.75** px | pinned 183.75 px content box |
| `errors_badge_is_thaw_badge` | no trigger | today 0 |
| `errors_popover_no_reflow` | missing trigger | (same precondition) |
| `errors_popover_shows_buckets` | missing trigger | (same precondition) |
| `errors_popover_keyboard_and_escape` | no `.stats-errors-surface` | today red at its precondition |
| `errors_popover_placement_scrolled` | missing trigger | (same precondition) |

**Wasm oracles.** RED = compile failure at the seam (`E0432` unresolved imports of the not-yet-written
functions — the blueprint's honest RED for a pure slice), then GREEN per group:

- Step 2 (G3, T+5): `69 passed` after `SearchDebounce`/`search_debounce_step` landed.
- Step 4 (G4, T+3): `72 passed` after `generated_label`/`profile_counts_line` landed.
- Step 6 (G5, T+1): `73 passed` in `core_screens` after `buckets_total` landed.

The plan gate's round-7 mutant sweep (9/9 killed, 22/22 oracle rows red on pristine HEAD) is the
design-time mutant validation; the CDP pristine sweep above re-demonstrates the wrong-behaviour
side of every browser row on the landed tree, and the discriminator inputs are in the test table
(running-total convention: burst 1 vs 5; dedupe 1 vs 2; submit 2 vs 1; trim `rust` vs `"  rust  "`;
clear 2 vs 1; `[("a",3),("b",4)]` → 7 vs 3).

**GREEN (final dist).** One authoritative sweep of all 13 lanes against the final bundle:
**13/13 PASS, exit 0** (`.tmp/scratch/issue118-w6/green-final/results.json`).

## Census ladder (Step 8)

```
base  (70fde7e):        sites=230 literal=203 const=22 brace=5  emitted=189 hooks=189
wave 6 (final tree):    sites=232 literal=205 const=22 brace=5  emitted=191 hooks=191 missing=0 dead=0
```

- emitted: −1 `stats-buckets`, +1 `stats-errors-trigger`, +1 `stats-errors-surface`,
  +1 `memories-search-clear-slot` → **+2** (191).
- sites: +1 (clear-slot span), +1 (errors cell: +2 trigger/surface class sites, −1 deleted
  `<details>` site) → **+2** (232). `residue=0`.
- Gate: `node scripts/check-class-coverage.mjs --self-test --min-emitted=191` → exit 0.
- `check-token-coverage.mjs --self-test` → `SELFTEST 2/2`, exit 0.
- `<summary` in `web-ui/src/core_screens.rs`: **0** (wave 5's landed assertion of 1 is historical
  by design — the last one, the stats bucket, was deleted this wave).

## Page heights (Step 9, 1280×800, readiness-gated, wave-6 fixture)

| route | post-wave-5 (`70fde7e` bundle) | post-wave-6 | wave-6 delta |
| --- | --- | --- | --- |
| `/project-memories` (15 memories, page size 8) | 4891 | 4891 | **0** — G3 moved nothing |
| `/stats` (8 daily rows + 30 shards) | 1358 | **1335** | **−23** — PROFILE condenses six lines to four |

(4891 vs the wave-3 baseline 3322: different fixture content, same fixture shape — re-derived per
the blueprint, only the wave-6 delta is ruled on. 1335 matches the blueprint's pinned placement
fixture scrollHeight exactly.)

## Resting row height of the errors cell (Step 11)

`<details><summary>` → Badge-in-zeroed-Button, popover closed:

```
row: 44.1875 -> 44.1875 (Δ0)   errors cell: DETAILS 20px -> BUTTON.stats-errors-trigger 20px (Δ0)
```

The swap itself is height-neutral at rest; activation reflow is gated by
`errors_popover_no_reflow` (PASS: table scrollHeight and row height both Δ0 across activation).

## Gates (Step 10) — all exit 0

| gate | result |
| --- | --- |
| `cd web-ui && cargo build --offline --target wasm32-unknown-unknown` | exit 0 |
| `cd web-ui && cargo test --offline --target wasm32-unknown-unknown` | **264 passed** (255 + 9, `T+9` exactly) |
| web-ui panic-family: `cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used` | exit 0 |
| web-ui strict: `cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation` | exit 0 (the let-chain and the `title=` local binding pass the gate that owns `collapsible_if`/E0521) |
| root panic-family + root strict (AGENTS.md verbatim) | exit 0, exit 0 |
| `cd web-ui && cargo fmt --all --check` (new in r7) | exit 0 (two round-6-style reflows caught and applied: import order, call wrapping) |
| census `--min-emitted=191` | exit 0, `emitted=191 hooks=191 sites=232 … residue=0` |
| token coverage `--self-test` | exit 0, `SELFTEST 2/2` |
| `cd web-ui && trunk build --dist ../.tmp/scratch/issue118-w6/dist` | exit 0 |

## Step 12 — stale numerals (in scope per task delta)

- `.githooks/pre-commit` `--min-emitted=175` → **191** (both the gate line and its reproduce hint),
  `bash -n` clean.
- `docs/agents/testing.md`: command floor `187` → **191**; census triple updated to the measured
  `sites=232 literal=205 const=22 brace=5 residue=0` / `emitted=191 hooks=191` with the wave-6
  arithmetic appended (wave-2 history kept); ratchet prose now reads 191.

## Staging (Step 11)

`cp -R .tmp/scratch/issue118-w6/dist .tmp/scratch/issue118-w6/stage/dist` — scratch staging only;
`plugin/engine/linux-x64/web-ui/dist` untouched; no `rm -rf` anywhere; engine pid 20227 untouched.

## Deviations

1. **CDP keystroke pacing is Node-local, not `Runtime.evaluate` roundtrips.** First G3 GREEN run
   fired 4 debounce requests: CDP roundtrips stretched the "50 ms apart" keystrokes past the 300 ms
   window, so each `Elapsed` legitimately fired. `sleep()` now uses `setTimeout` in Node; the
   burst lane then reads exactly 1. Recorded as a lane-construction rule for future waves.
2. **`set_timeout_with_handle` returns `Result<TimeoutHandle, JsValue>`** in the pinned leptos_dom,
   not the bare `TimeoutHandle` the blueprint's sketch assumed. Handled with
   `if let Ok(handle)` — never `unwrap`/`expect` (panic gate). The timer is simply not armed in
   the failure branch (no window to schedule on).
3. **`InputSuffix` slots into `Input`, not `Field`.** The blueprint's markup sketch reads as a
   `Field` child; thaw 0.4.8 puts `input_suffix: Option<InputSuffix>` on `Input`'s props
   (`E0599` on first build). Slot moved inside `<Input>` — same reservation semantics.
4. **Reflow lane fixture widened.** The blueprint's `/api/search` answer was unspecified; 8 items
   toggled the scrollbar (15-row listing → 8-row search) and moved Reload by exactly the 15 px
   scrollbar width. The wave-6 fixture answers search with 15 items / `total: 42`, so the
   scrollbar state holds constant and the lane measures the margin-`auto` geometry only.
5. **Headless renderer wedge.** One lane run regressed to 0 requests / empty range; the browser
   session was corrupt (a prior run's pages persisted). Restarting Chrome with a fresh profile
   cleared it; every recorded number above is from a clean session.

## Artifacts

- Scratch dist + staging: `.tmp/scratch/issue118-w6/{dist,stage/dist,dist-pristine}`
- RED sweep: `.tmp/scratch/issue118-w6/red-pristine/results.json`
- GREEN sweep: `.tmp/scratch/issue118-w6/green-final/results.json`
- Per-group greens: `.tmp/scratch/issue118-w6/{green-g3,green-g4,green-g5}/results.json`
- Lane script: `web-ui/scripts/test-wave6-cdp.cjs` (fixture
  `web-ui/scripts/fixtures/issue-118-wave6.json`)
