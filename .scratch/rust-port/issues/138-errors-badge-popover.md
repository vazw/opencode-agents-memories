# 138 — the errors column resizes the table when opened

Status: ready-for-agent — will create the errors trigger (`Thaw Badge` inside a `Thaw Button`) and its `Popover` surface in `web-ui/src/core_screens.rs`, replacing the `<details class="stats-buckets">` block; removes `.stats-buckets` rules from `web-ui/public/styles.css`; updates `web-ui/scripts/capture-evidence.cjs` if a lane touches those selectors.

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
