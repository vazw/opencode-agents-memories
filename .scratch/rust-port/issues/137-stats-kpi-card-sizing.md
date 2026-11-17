# 137 — the `/stats` summary cards differ in height and alignment

Status: done — 2026-11-17 — delivered in wave 6; verification under `## Outcome`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

The four summary cards on `/stats` must form a row.

Today they are content-height and centred: `PROFILE` renders six lines while the other three render three, so it sits ~70 px higher and taller and the row is ragged. The cause is Thaw's own `.thaw-card { margin: auto }` — auto block-axis margins on a grid item absorb the free space and cancel `align-self: stretch`.

Equalise the cards (top-aligned, equal height) and stop the GENERATED timestamp wrapping mid-token. Note that `white-space: nowrap` alone overflows into the card's `overflow: hidden` and clips silently, so the label must also be shortened — and that shortening is a display truncation, not a lossless format.

## Acceptance criteria

- [ ] All four cards report equal heights and equal tops within 1 px at 1280×800, measured from the browser.
- [ ] The GENERATED value renders on one line without clipping or mid-token wrapping.
- [ ] The PROFILE card keeps version, last-analyzed, all three counts and the prompt count — condensed, not dropped.
- [ ] The full instant remains available (a `title` or equivalent), and the shortening is recorded as a display truncation.
- [ ] RED on today's tree: card tops differ by ~70 px.

## Blocked by

None — can start immediately.

## Outcome (MEASURED, closed 2026-11-17)

Delta landed in wave 6, commit `d36f491`.

- `stats_cards_equal_height_top`: tops spread 70.77 px on the pristine bundle → within 1 px on the final bundle.
- `stats_generated_single_line`: was 2 lines in a 183.75 px content box → one line, no clipping; the full instant stays available as a `title`.
- PROFILE card condenses six lines to four while keeping version, last-analyzed, all three counts and the prompt count; `/stats` scrollHeight 1358 → 1335 (Δ−23).
- Evidence: `docs/evidence/issue118-wave6/README.md` §RED→GREEN, §Page heights.
