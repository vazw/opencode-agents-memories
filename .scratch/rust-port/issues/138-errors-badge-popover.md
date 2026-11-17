# 138 — the errors column resizes the table when opened

Status: done — 2026-11-17 — delivered in wave 6; verification under `## Outcome`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

Reading an error breakdown must not move the table.

Today the error count is a `<details>`; expanding it inserts an inline row, so the table grows and the rows below shift. The count should render as a Thaw **Badge** and open a floating panel carrying the breakdown, leaving the table's geometry untouched.

Two facts constrain the design and must be respected rather than rediscovered: Thaw's `Popover` trigger decoration supplies click behaviour only — no role, no tabindex, no expanded state, no Escape — so the trigger must be a real button and dismissal must be implemented and tested; and Thaw positions the surface from viewport coordinates into a document-space container, so on a scrolled page the surface lands `scrollY` px off and needs a correction applied **to the surface at open time**.

## Acceptance criteria

- [ ] Activating the error badge leaves the table's `scrollHeight` and the affected row's height unchanged (asserted before/after).
- [ ] The floating panel is fully visible (not off-screen, not clipped) with the table at scroll 0 **and** scrolled to the bottom of a long page; the assertion measures the panel, not an inner element.
- [ ] The badge is keyboard-reachable and exposes its expanded state; the panel dismisses on Escape and on outside click.
- [ ] "no errors" stays plain text, not a badge.
- [ ] The panel's readable content names each error bucket with its count.
- [ ] RED on today's tree: activating inserts a row and changes the table's height.

## Blocked by

None — can start immediately.

## Outcome (MEASURED, closed 2026-11-17)

Delta landed in wave 6, commit `d36f491`.

- `errors_badge_is_thaw_badge`: `<details><summary>` → Thaw Badge inside a zeroed Button (`.stats-errors-trigger`); resting row height 44.1875 px, Δ0 across the swap.
- `errors_popover_no_reflow`: table `scrollHeight` and row height both Δ0 across activation.
- `errors_popover_placement_scrolled`: panel fully visible at scroll 0 and at the bottom of a long page; `errors_popover_keyboard_and_escape` green (keyboard reachable, expanded state exposed, Escape and outside click dismiss).
- "no errors" stays plain text; the panel names each error bucket with its count (`buckets_total` oracle, T+1).
- Evidence: `docs/evidence/issue118-wave6/README.md` §Resting row height, §RED→GREEN.
