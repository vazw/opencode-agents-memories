## Run — post-change RED→GREEN (same lane, `LANE_EXPECT=post`)

Served bundle: `web-ui-259e2ecdd2554e09_bg.wasm`, sha256
`38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455` (final Step 6 tree after the two clippy
fixes, `trunk build --release`), served from its own copy under `.tmp/scratch/r9b/dist-post-5` by its own
engine on `--web-port 3803`; the pre-change dist under `.tmp/scratch/issue129-shardB/dist` was never
overwritten.

```
$ TEST_BASE_URL=http://127.0.0.1:3803 WEBUI_DIST=$PWD/.tmp/scratch/r9b/dist-post-5 \
  LANE_EXPECT=post LANE_TABLE=$PWD/.tmp/progress/r9b/dialog-post-5.md DIALOG_ONLY=1 \
  node web-ui/scripts/capture-evidence.cjs
rc=0  rows=14 mismatches=0 stopped=false
```

REWORK re-run (2026-11-06, review-1 HIGH settlement): after the lane's
`maintenance_exactly_one_cleanup_post` row was branched on `LANE_EXPECT` (pre branch restored; post
branch byte-identical to the run above), the post lane was re-run once against a fresh
`trunk build --release` whose output is byte-identical to the recorded bundle (same id
`259e2ecdd2554e09`, same wasm sha256 `38dedc58…` — the rework changed no web-ui source). Raw:
`.tmp/progress/r9b-rework/dialog-post-rework.{log,md}`.

```
$ TEST_BASE_URL=http://127.0.0.1:49392 WEBUI_DIST=$PWD/web-ui/dist \
  LANE_EXPECT=post LANE_TABLE=$PWD/.tmp/progress/r9b-rework/dialog-post-rework.md DIALOG_ONLY=1 \
  node web-ui/scripts/capture-evidence.cjs
rc=0  rows=14 mismatches=0 stopped=false
```

Every row starts with the armed-surface guard (`[data-testid="confirm-gate"],[data-testid="confirm-dialog"]`
must match exactly one element, `confirm-gate-confirm` and `confirm-gate-cancel` exactly once each). A guard
miss is recorded as `STOP` — a fixture failure, never a RED. Supplies are real: `Input.dispatchMouseEvent`
at the element centre for every click, `Input.dispatchKeyEvent` with the browser default action live for
Tab/Shift-Tab/Enter/Escape, and fetch fixtures built with `new Response`.

# DIALOG lane — bundle 259e2ecdd2554e09 (sha256 38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455)
LANE_EXPECT=post

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| dialog_role_and_name | RED | memory, session, skill rows | PASS | PASS | /project-memories:"Delete permanently? This cannot be undone." /sessions:"Delete permanently? This cannot be undone." /skill-library:"Delete permanently? This cannot be undone." |
| dialog_escape_disarms | RED | memory row, Escape | PASS | PASS | surface gone, Delete usable, no DELETE issued |
| dialog_focus_trapped_forward_and_reverse | RED | memory row, Tab and Shift-Tab | PASS | PASS | forward and reverse Tab stayed inside |
| dialog_focus_returns_to_invoker | RED | memory row, Cancel | PASS | PASS | focus returned to the same Delete node |
| confirm_removed_invoker_focus_fallback | RED | DELETE held, row removed | PASS | PASS | focus on [data-testid=memories-reload] |
| maintenance_confirm_in_dialog | RED | cleanup armed | PASS | PASS | dialogs=1 confirm=1 |
| maintenance_focus_returns | RED | cleanup armed, cancel then Escape | PASS | PASS | focus on maintenance-action (esc path: dialog present=false) |
| maintenance_inline_action_inert_while_armed | RED | cleanup armed | PASS | PASS | disabled while armed (Confirm cleanup) |
| dialog_cancel_disarms | CONTROL | memory row, Cancel | PASS | PASS | surface gone, no DELETE issued |
| dialog_mask_does_not_close | CONTROL | memory row, outside click | PASS | PASS | clicked 789.0410766601562,518.8131256103516 on DIV.thaw-dialog-surface__backdrop fade-in-transition-enter-from fade-in-transition-enter-active; surface kept |
| dialog_confirm_executes_exactly_once | CONTROL | DELETE held, Confirm twice | PASS | PASS | exactly one DELETE |
| confirm_navigation_does_not_steal_focus | CONTROL | focus moved before resolve | PASS | PASS | focus stayed on memories-reload |
| maintenance_dialog_preserves_notice | CONTROL | cleanup armed then cancelled | PASS | PASS | notice via [data-testid="maintenance-dialog-cancel"], no POST |
| maintenance_exactly_one_cleanup_post | CONTROL | every confirm path in turn | PASS | PASS | exactly one POST /api/cleanup (dialog confirm present=1, inline probe covered) |

Pre-change outcomes for the same 14 rows are in `dialog-baseline.md` (8 RED FAIL, 6 CONTROL PASS on bundle
`72f5eaea5edb020c`). The RED rows are the change; the CONTROL rows are the regression guard.

## Rendered shape (MEASURED, `.tmp/progress/r9b/probe.cjs`)

A memory row armed through a real pointer click renders:

```html
<div class="thaw-dialog-surface …" role="dialog" aria-modal="true" aria-labelledby="confirm-dialog-title">
  <div class="thaw-dialog-body">
    <div class="confirm-dialog" data-testid="confirm-dialog">
      <h2 class="thaw-dialog-title" id="confirm-dialog-title" data-testid="confirm-dialog-title">
        Delete permanently? This cannot be undone.</h2>
      <div class="thaw-dialog-actions" data-testid="confirm-dialog-actions">
        <button class="thaw-button … thaw-button--primary" data-testid="confirm-gate-confirm">Confirm delete</button>
        <button class="thaw-button … thaw-button--secondary" data-testid="confirm-gate-cancel">Cancel</button>
      </div>
    </div>
  </div>
</div>
```

So the surface carries `role="dialog"` + `aria-modal` + `aria-labelledby` (the accessible name the lane reads
is the title's text), the wrapper `data-testid="confirm-dialog"` sits inside `.thaw-dialog-body`, and the
title/actions testids land on the `h2` and the actions `div`. `attr:` on thaw 0.4.8's `DialogSurface` /
`DialogTitle` / `DialogActions` is applied to each component's rendered root (`add_any_attr`), which is why
the surface's `aria-labelledby` is present even though those components take only `class`.

The same probe records that the Delete node survives the whole gesture (`same: true`, marker intact) and that
`document.activeElement` is that button after Cancel — i.e. focus returns to the *same* node, not a rebuilt
one.

## Step 6 change

- `web-ui/src/confirm.rs` — `ConfirmGate` (inline `span[role=group]`) is gone. `ConfirmDialog` is the modal
  surface (`Dialog` + `DialogSurface` + `DialogBody` + `DialogTitle`/`DialogActions`), `mask_closeable=false`,
  `open` derived from the frozen reducer (`pending.is_some() && !running`) with the setter mapping `false`
  onto `ConfirmEvent::Cancel` and ignoring `true`. `ConfirmDeleteButton` keeps the row's Delete button
  mounted for the whole gesture and renders the dialog beside it. `track_focus_return` restores focus only
  after the gesture settles (per-row `Stage`), and `track_section_focus_return` does the same for the
  maintenance section.
- `web-ui/src/dialog_focus.rs` (new, exported from `lib.rs`) — `tab_target` (pure boundary decision),
  `trap_tab` (forward/reverse wrap over the surface's focusable descendants, empty list left alone),
  `return_focus` (first write immediately, then a bounded watch that stops the moment the user owns focus).
- `web-ui/src/{core_screens,screens,skill_library}.rs` — all four sites swapped in one compile unit; the
  memory/session/skill Delete buttons stay mounted, the maintenance inline action is `disabled` while armed
  and the final decision is the dialog's.
- `web-ui/public/styles.css` — `.confirm-gate*` and `.maintenance-cancel` retired; `.confirm-dialog` (the
  warning frame, now inside the dialog body) and `.maintenance-dialog-copy` added.
- `.githooks/pre-commit` — `--min-emitted` re-derived by measurement 178 → 175 (5 retired class tokens, 2
  added).
- `web-ui/scripts/capture-evidence.cjs` — `clickAt` now scrolls into view, hit-tests the centre and retries
  (a covered target is a named FIXTURE error); `dialog_role_and_name` computes the accessible name the way a
  screen reader does (`aria-labelledby` → referenced text, `aria-label` fallback); the
  `maintenance_exactly_one_cleanup_post` inline probe is skipped and recorded when the modal backdrop covers
  the inline action. The pre-change baseline rows are unaffected: every one of those rows failed on 0 dialogs
  or on the enabled inline action, before any of these refinements could matter.

## Frozen reducer (obligation `confirm_reducer_unchanged`)

`.tmp/progress/r9b/frozen.py` extracts each frozen item from `git show HEAD:` and from the worktree and
compares them byte for byte:

```
web-ui/src/confirm.rs:confirm_next identical=True
web-ui/src/confirm.rs:ConfirmState identical=True
web-ui/src/confirm.rs:ConfirmStep identical=True
web-ui/src/confirm.rs:ConfirmEvent identical=True
web-ui/src/confirm.rs:dispatch identical=True
web-ui/src/core_screens.rs:cleanup_next identical=True
web-ui/src/core_screens.rs:CleanupState identical=True
web-ui/src/core_screens.rs:CleanupEvent identical=True
web-ui/src/core_screens.rs:CleanupAction identical=True
web-ui/src/core_screens.rs:cleanup_disabled identical=True
web-ui/src/core_screens.rs:cleanup_action_label identical=True
web-ui/src/core_screens.rs:cleanup_status identical=True
FROZEN_ALL_IDENTICAL
```

`git diff --stat -- web-ui/tests/confirm.rs` is empty, and the cleanup table tests are green in the wasm
suite (`cargo test --target wasm32-unknown-unknown` → 20 suites, 238 tests, 0 failed).

## Focus-return scheduling — a measured deviation from the blueprint's wording

The blueprint says `return_focus` "writes on `request_animation_frame` so a re-enabled button is not
skipped". MEASURED: in the lane's background CDP tab `requestAnimationFrame` callbacks never run at all —
a probe that logged from inside a frame callback recorded `len=0` while `document.activeElement` stayed
`BODY`, so an rAF-only hand-back never wrote and three RED rows failed for that reason alone. The deferral is
still there and still the point (the first write happens immediately, and the watch keeps re-checking until
the DOM settles); the re-arm now races an animation frame against a 16 ms timer, and whichever fires first
wins (`pending` flag). Intent and observable behaviour are unchanged in a visible tab; the timer is what makes
it work when the tab is not rendering.

## Mutation proof — each mutant breaks the row it is aimed at

Each mutant is a copy of the final `web-ui/` tree with one wiring defect applied
(`.tmp/progress/r9b/prep_mutants.py`, `prep_mutants_m3b.py`), built with `trunk build --release` into its own
`CARGO_TARGET_DIR`, served by its own engine on its own storage root, and driven by the same DIALOG lane with
`LANE_EXPECT=post`. `mismatches` is the lane's own count; a `STOP` is a fixture failure.

| mutant | defect | bundle (sha256 prefix) | lane rc | row(s) that broke | recorded reason |
| --- | --- | --- | --- | --- | --- |
| m1 | `mask_closeable=false` → `true` | `web-ui-17a79faa232b8376` (`a6048163`) | 1 | `dialog_mask_does_not_close` (CONTROL) FAIL | the confirmation closed on an outside click (surface=0) |
| m2 | `on_run` fired twice per confirmed click | `web-ui-12e77d0febf8ad3f` (`92ca310d`) | 1 | `dialog_confirm_executes_exactly_once` (CONTROL) FAIL | 2 DELETE requests for one confirmation |
| m2 | (same defect, collateral) | | | `confirm_removed_invoker_focus_fallback`, `confirm_navigation_does_not_steal_focus` STOP | FIXTURE: the held DELETE never reached the fixture |
| m3 | `user_moved_focus` always false | `web-ui-edbc1b7b52a59c99` (`b18a5799`) | 0 | none | MEASURED NEGATIVE — see below |
| m3b | the fallback branch never writes focus | `web-ui-c1be340be26f6eba` (`409bc9c9`) | 1 | `confirm_removed_invoker_focus_fallback` (RED) FAIL | focus stayed on BODY, not the memories Reload fallback |
| m3b | (same defect) | | | `confirm_navigation_does_not_steal_focus` (CONTROL) FAIL | the async completion stole focus from the moved control |
| m4 | armed notice never rendered | `web-ui-f9ce023f411c8256` (`f930fff1`) | 1 | `maintenance_dialog_preserves_notice` (CONTROL) STOP, plus `maintenance_confirm_in_dialog`, `maintenance_focus_returns`, `maintenance_inline_action_inert_while_armed`, `maintenance_exactly_one_cleanup_post` STOP | FIXTURE: the cleanup arm notice never appeared |
| m5 | inline action enabled while armed | `web-ui-c56689ce198347c5` (`926003e6`) | 1 | `maintenance_inline_action_inert_while_armed` (RED) FAIL | the inline action is still enabled while armed (Confirm cleanup) |

Two measured negatives, recorded rather than papered over:

1. **m3 does not fail `confirm_navigation_does_not_steal_focus`.** The node the user moves to in that fixture
   *is* the code's declared fallback (`memories-reload`), so a watch that ignores the user's choice still ends
   on the same node and the row cannot see the difference. What the row does discriminate is the fallback
   write itself — m3b, which drops it, fails the row with "the async completion stole focus from the moved
   control". So the row proves the hand-back happens and lands where the user already was; it does not prove
   the `user_moved_focus` guard in isolation.
2. **m5 does not fail `maintenance_exactly_one_cleanup_post`.** Post-change the dialog is modal, so the
   backdrop covers the inline action; the row's inline probe is skipped and says so (`inline probe covered`).
   The inline wiring is caught by `maintenance_inline_action_inert_while_armed` instead. The exactly-once row
   still measures what it is named for: one `POST /api/cleanup` across every confirm path.

Both negatives were found by running the mutants, not by reading the code, and both are why the proof lists
`m3b` next to `m3`.

### A build trap worth recording

The first mutant builds shared one `CARGO_TARGET_DIR` and the mutant trees were copied with mtimes preserved.
Cargo's fingerprint then judged every mutant fresh and each `dist` received the *same* artifact — all five
`sha256` identical, a silent false proof. Fixed by giving each mutant its own target dir and `touch`ing its
sources before the build; the hashes above are from the corrected builds and all five (plus `m3b`) differ from
each other and from the shipped bundle `38dedc58…`.
