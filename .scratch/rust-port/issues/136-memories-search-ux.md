# 136 — the memories search is a bare full-width field

Status: ready-for-agent — will create `search_debounce_step()` (a pure state machine) in `web-ui/src/core_screens.rs`, bound the field with `.memories-search-field`, and add the clear affordance; CSS in `web-ui/public/styles.css`.

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
