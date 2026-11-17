# 137 — the `/stats` summary cards differ in height and alignment

Status: ready-for-agent — will set `margin: 0` and stretch on `.stats-card` in `web-ui/public/styles.css`; will create `generated_label()` in `web-ui/src/core_screens.rs`.

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
