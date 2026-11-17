# 133 — the memory card's "Show more" splits the sentence it expands

Status: ready-for-agent — will create `memory_body()` and `memory_toggle_label()` in `web-ui/src/core_screens.rs`, replacing the `<details>`/`<summary>` pair; deletes the `-webkit-line-clamp`/`-webkit-box`/`overflow` triple from `.memory-list .memory-content` in `web-ui/public/styles.css`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

A memory card must read continuously whether it is collapsed or expanded.

Today the body is split at 320 characters (at a word boundary, so the loss is not visible), and a `<details><summary>` element sits **between** the two halves. Collapsed, that reads fine; expanded, the reader gets half a sentence, a control, then the rest of the same sentence — with the control unstyled, because no CSS targets `details` anywhere.

Replacing it with one content element that renders either the preview or the whole body, and a control **after** it, removes the possibility structurally: there is no gap to sit in. The control should be a Thaw button like the card's other actions, labelled "Show more" and "Show less".

The card's height must not blow up: the preview is what bounds a collapsed card. Whatever guard made the clamp dead (the preview is already ≤ 320 characters) must be stated, and the clamp triple deleted only if it is genuinely dead for every use of the class.

## Acceptance criteria

- [ ] In the expanded state the body is contiguous: the rendered content equals `preview + rest`, and the toggle is the element **after** it — asserted in the DOM, not by reading code.
- [ ] Collapsed renders exactly the preview, and the toggle reads "Show more"; expanded reads "Show less" and returns to the preview.
- [ ] No interactive element sits between any two parts of the body, at 1280×800, verified in a browser lane.
- [ ] The clamp triple is gone from the stylesheet and a grep proves it; if any other component uses the class, the deletion is dropped and the guard is stated instead.
- [ ] A body long enough to overflow the old clamp (≥ 1400 characters) is used, and the lane asserts the computed style (`webkitLineClamp === 'none'`, `display !== '-webkit-box'`) so the check cannot pass on a short fixture.
- [ ] RED on today's tree: the expanded body is two halves around a `<summary>`.

## Blocked by

None — can start immediately.
