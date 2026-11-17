# 136 — the memories search is a bare full-width field

Status: done — 2026-11-17 — delivered in wave 6; verification under `## Outcome`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

Searching the memories list should feel like a control, not a strip of empty canvas.

Today the field is `flex: 1 1 18rem`, so it grows to the full content width; the Reload button and the pager count are pushed to the far edge; there is no clear affordance, and typing issues **no request at all** — the query is a two-way model that nothing watches.

Bound the field, give it a clear action that appears only when there is something to clear, and debounce typing so N keystrokes produce one request. Clearing must also produce exactly one request. The pager and Reload must not move when results change.

## Acceptance criteria

- [ ] The field's rendered width is bounded (not full-row) at 1280×800, measured in the browser.
- [ ] The clear affordance is absent when the query is empty, appears when it is not, and returns the unfiltered list in one activation.
- [ ] Typing a five-character query issues exactly **one** search request; clearing issues exactly **one**.
- [ ] The pager text and Reload button keep their measured position when the result set changes (|dx| ≤ 1 px).
- [ ] The debounce is a pure function the Node runner can execute without a DOM.
- [ ] RED on today's tree: typing issues **0** requests, so the request-count assertion fails and the position assertion is meaningless without it.

## Blocked by

None — can start immediately.

## Outcome (MEASURED, closed 2026-11-17)

Delta landed in wave 6, commit `d36f491` (review 1 APPROVED, blueprint r7 stamp `77b5028db9ab`).

- `search_field_is_bounded`: field 719.83 px on the pristine bundle → bounded inside the reserved slot; clear slot reserved via `InputSuffix`.
- `search_debounce_one_request`: 0 requests on pristine → exactly one per burst; `search_clear_affordance_is_reactive` and `search_clear_restores_list` green.
- `search_result_reflow_pinned`: pager and Reload keep position (|dx| ≤ 1 px).
- Debounce is a pure function (`search_debounce_step`) exercisable without a DOM; the dedupe mutant (`pending != fired` guard dropped) is killed at the seam — `test_search_debounce_skips_repeated_query` fails 1/73.
- Wasm suite 264 passed / 0 failed; census `191/191/232`, residue 0. Evidence: `docs/evidence/issue118-wave6/README.md`; review `.tmp/progress/20261117-1002-issue-118-wave6-search-and-stats-r7.review-1.md`.
