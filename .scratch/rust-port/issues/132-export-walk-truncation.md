# 132 — the browser export silently truncates at 5000 memories

Status: done — 2026-11-17 — delivered in wave 5; verification under `## Outcome`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

A download from the Export panel must contain every exportable memory in the store, and the number the panel prints must be the number of rows in the file it produces.

Today the walk stops at a fixed `page >= 25` (× 200 rows = 5000) while the store holds more, and the preview prints the truncated count as if it were the total — so on this store the panel reads `memories: 5000` against a store of 6227 and the download silently ships 5000 rows. The discontinuation is invisible: no warning, no marker, and the count itself looks authoritative.

The continuation decision should come from the server's own page count (already returned with each page), bounded by a named work ceiling rather than a magic 25. When the ceiling or a mid-walk failure does shorten the walk, the preview must say so — `memories: {n} of {total} — export limited` — and when the store's total cannot be known, say that too. The number printed and the number written must come from one function, so they cannot disagree.

## Acceptance criteria

- [ ] The walk reaches the store's total whenever the ceiling permits it: on a store of 6227 rows the preview reads `memories: 6227` and the document holds 6227 rows.
- [ ] A truncated walk (ceiling hit, or a page failing mid-walk) prints `memories: {n} of {total} — export limited`; an unknown total prints the unknown-total wording, not a bare count.
- [ ] The preview's number and the JSON document's row count are produced by the same function; a test asserts they are equal for a complete walk and for each truncation shape.
- [ ] The ceiling terminates for any `total_pages` (including an absurd one) and a short page still ends the walk.
- [ ] The store's total is taken from the list response with `includePrompts=false`, which counts exactly the exportable rows; no new server route is added.
- [ ] RED on today's tree: `should_continue(25, 200, 200, 32)` is `false`, and the preview reads `memories: 5000` on a 6227-row store.

## Blocked by

None — can start immediately.

## Outcome (MEASURED, closed 2026-11-17)

Delta landed in wave 5, commit `70fde7e` (review 2 APPROVED; review 1 had rejected one vacuous oracle, S1).

- `export_walks_to_store_total`: `requests: 32`, `preview: "memories: 6227"`, `rows: 6227`, `truncated: false`; preview and blob row count come from one function and a test asserts equality.
- `export_discloses_a_short_walk`: `requests: 3`, preview `"memories: 400 of 6227 — export limited"`, `rows: 400`, `truncated: true`.
- Ceiling terminates for any `total_pages`; mutant M1 (restore `page >= 25`) fails `test_export_walk_reaches_the_store_total`; M5 (`count + 1`) fails `test_export_preview_states_no_limit_when_complete`.
- S1 kill input now pinned: `!export_truncated(6229, Some(6227))` (mutant `<`→`!=` survived 25/25 before it).
- Total read from the list response with `includePrompts=false`; no new server route.
- Evidence: `docs/evidence/issue-118-wave5/README.md` §1 F1a/F1b, §2 M1/M5, §6; review `.tmp/progress/20261117-0825-issue-118-wave5-export-and-a11y-r6.review-2.md`.
