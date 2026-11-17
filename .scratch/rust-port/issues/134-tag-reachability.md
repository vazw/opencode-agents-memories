# 134 — the memory card's `+N more` tags cannot be reached

Status: done — 2026-11-17 — delivered in wave 5; verification under `## Outcome`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

Every tag on a memory must be reachable and usable, not merely announced.

Today the first four tags render as filter buttons and the remainder collapse into a `<span>` carrying only an `aria-label` — a sighted mouse user cannot see or click the fifth tag. The card that exposed this showed `rust blueprint doc-correction mem-store` followed by a `+1 more` chip that does nothing.

The overflow label should become a real control that reveals the remaining tags, and the revealed tags must behave identically to the visible four: same testid, same accessible name, same filter action. One activation from collapsed must be enough.

Note the evidence script currently pins the defect: one lane asserts the overflow label is **not** a button and another counts buttons on the card. Both move in the same change, or the script keeps certifying the bug.

## Acceptance criteria

- [ ] From the collapsed state, one activation reveals every tag in the record; each revealed tag filters by its own tag, exercised end to end.
- [ ] The revealed tags carry `data-testid="memory-tag-pill"` and the same accessible name shape as the visible four.
- [ ] The overflow control keeps an accessible name naming the hidden tags, and exposes its expanded state.
- [ ] `capture-evidence.cjs` no longer asserts that the overflow label is not a button; its button-count lane is narrowed so the new control does not make it fail for the wrong reason.
- [ ] The class census moves only as declared, with `dead=0` and no new unresolved hook.
- [ ] RED on today's tree: the overflow label is a `<span>` with no handler, and activating it does nothing.

## Blocked by

- 133 — same component and same file; land 133 first to avoid conflicting edits.

## Outcome (MEASURED, closed 2026-11-17)

Delta landed in wave 5, commit `70fde7e`.

- `memory_card_tags_reachable`: pills `4 → 5` after one activation, last chip accessible name `"Filter by tag: fifth"`, the click issued a `tags=fifth` request end to end.
- The overflow control is now a Button (`memory-tag-more`), not a span, and keeps an expanded state.
- `capture-evidence.cjs` pill RED lanes narrowed from 4 to 1; the census moved `187 → 188` with `dead=0`, `unresolved=0`.
- Evidence: `docs/evidence/issue-118-wave5/README.md` §3 greps, §4 census, §6.
