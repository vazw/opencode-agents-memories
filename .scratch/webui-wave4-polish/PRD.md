# PRD: Web UI wave 4 polish — payload-driven summaries, empty-state truth, spacing, accessible stats

Landed 2026-11-10: merged to main at `fa8d62c` (product + harness), `2daf585` (evidence), `15009f1`
(tracker update). Review 4 APPROVED; independent opsec APPROVED (0 confirmed). Owner issue:
`.scratch/rust-port/issues/118-web-ui-thaw-unused.md` (§ Wave 4 outcome). Evidence:
`docs/evidence/issue-118-wave4/`.

## Problem Statement

After the Thaw migration's waves 1–3, the web explorer still failed falsifiable polish checks that
users meet on every session:

- The Home screen painted fake numeric summary cards before data arrived and rendered each card's
  label text twice, so the numbers users see first are invented and screen readers announce labels
  twice.
- The profile picker had no `for`-bound label, so the control is unnamed for assistive tech.
- The skills screen showed the same silence whether the skill library is empty or the current filter
  matches nothing — two different problems with one misleading response.
- Export panel blocks touched (0 px gap), so actions read as one blob and mis-clicks are likely.
- Stats bars leaked `attr:aria-label` as a literal DOM attribute with a null accessible name, and
  the overflow region had no keyboard-reachable path.
- The Home heading carried UA-default margins — a regression against the shared panel rule.

## Solution

A scoped polish sweep (wave 4 of issue 118) that makes every region's first paint truthful and
accessible: summary cards render nothing until data arrives, then exact payload values including
zero; the picker label is bound; library-empty and filter-empty get distinct copy with a
clear-filters action; export blocks get measured ≥12 px separation at both viewports and both
themes; stats bars carry real accessible names and the overflow region is reachable by keyboard.
Every region shipped RED→GREEN: mutant-killing pure oracles in the wasm suite plus four
discriminating browser cases on a hash-pinned baseline dist, then 12/12 browser acceptance on the
rebuilt bundle.

## User Stories

1. As an explorer user, I want the Home summary cards to show nothing until the API answers, so that I never mistake invented numbers for real ones.
2. As an explorer user, I want zero counts to render as explicit `0` cards, so that an empty store is information, not a blank panel.
3. As a screen-reader user, I want each summary card to announce its label exactly once, so that I hear "Total memories 7", not "Total memories Total memories 7".
4. As a screen-reader user, I want the profile picker to have a `for`-bound label, so that focus lands on a named control.
5. As a skills user with no library, I want copy that says the library is empty, so that I know to add skills rather than change filters.
6. As a skills user with an over-narrow filter, I want copy that says the filter matched nothing plus a clear-filters action, so that I can recover in one step.
7. As an export-panel user, I want visible separation between export blocks, so that I click the action I meant.
8. As a screen-reader user, I want stats bars with real accessible names, so that bar state is announced instead of a literal `attr:aria-label` string.
9. As a keyboard user, I want a Tab path through the stats overflow region, so that truncated content is reachable without a mouse.
10. As a keyboard user, I want the tags link focusable with a visible ≥4.5:1 focus ring, so that I can see where I am.
11. As a maintainer, I want every region's oracle to be a pure function the wasm runner executes without a DOM, so that behavioural truth stays cheap to re-verify.
12. As a maintainer, I want browser acceptance to run against a hash-pinned served bundle with served==disk verification, so that a green run names exactly which bytes it judged.
13. As a maintainer, I want fake numerics, silent empties, and literal-attribute leaks to stay killed by named mutants, so that a regression re-opens a specific test, not a vibe.
14. As an operator, I want the wave's durable evidence to disclose what it did not fix (skill-delete erasure disclosure, acceptance-script assertion strength), so that residuals stay owned instead of absorbed.

## Implementation Decisions

- Regions and homes: Home panel + summary cards and health arms in `screens.rs`; stats bars, tags and
  export spacing in `core_screens.rs`; skills empty-state split in `skill_library.rs`; spacing and
  focus-visible rules in `public/styles.css`.
- Testid contracts stay stable and state-exclusive: `home-stats-loading` / `home-stats-ready` /
  `home-stats-error` / `home-stats-retry` / `home-summary-card`, `profile-identity-select` (native
  `<select>` per the wave-2 constraint), `skills-empty` / `skills-filtered-empty` /
  `skills-clear-filters`, the `stats-*` set including the overflow region.
- Stack frozen by ADR-0003 (leptos 0.7.x + thaw 0.4.8, CSR, wasm-only): no dependency added, no
  `Combobox` (it would silently break both native-select testids), no `DataTable`.
- Where a Thaw component spreads no attributes (0.4.8 `MessageBar`/`Badge` behaviour, wave-3 F4),
  testids ride the blueprint's wrapper-div fallback; no upstream patches.
- Class and token censuses are floors, not vibes: any removed bespoke class loses its CSS rule in the
  same step (`check-class-coverage.mjs` / `check-token-coverage.mjs` stay at missing=0 dead=0).
- Truth comes from the payload: cards render exact API values (including `0`) only in the Ready arm;
  loading is a spinner arm, never placeholder digits; error is a retry arm.

## Testing Decisions

- Two layers, both discriminators: pure oracles run by the wasm Node runner with no DOM (mutant
  targets named per test: `total=user+project`, `active=all-shards`, zero-hidden, always-copy,
  inverted-guard), and CDP browser acceptance (`web-ui/scripts/test-wave4-cdp.cjs`, 12 cases) against
  a served release build on a dedicated isolated-context page.
- RED is measured on an immutable hash-pinned baseline dist before markup changes; GREEN re-runs the
  same cases on the rebuilt bundle with served==disk SHA256 verification and a single-base
  `SHA256SUMS` manifest (`sha256sum -c` exit 0).
- Browser assertions are readiness-gated (3 clean samples) and count component testids, not nested
  Thaw DOM nodes; geometry claims are computed-style measurements (export `vGap 16.81px ≥ 12` at
  1280 & 390, light & dark; heading `18.4px`/`0px`), not screenshots alone.
- Prior art: wave-2's `test-state-block-cdp.cjs` and wave-3's readiness-gated
  `capture-evidence.cjs` lanes; same evidence-tree layout under `docs/evidence/`.

## Out of Scope

- The skill-delete confirmation's prompt-specific erasure disclosure — measured residual, needs a
  separately bounded approval; shared confirmation infrastructure untouched.
- Strengthening the acceptance script's `tags_unchanged_matrix` single-Tab assertion (actual tag
  focus was proven by the validation-local corrected probe; the shipped assertion is disclosed as
  weaker than its name).
- Any component beyond the five polish regions; the 16-row assessment table's unstarted rows remain
  owned by issue 118's wave plan.
- `DataTable`/`Dropdown`-class rebuilds — impossible on thaw 0.4.8 and not wanted here.

## Further Notes

Review chain kept for traceability: review-1 REJECTED (5 findings, incl. duplicate label + heading
regression) → debugger prescription → rework; review-2 REJECTED (unserved rebuilt dist, stale durable
evidence, residual ledger defects) → three-phase rework; review-3 REJECTED (ledger cursor shape,
missing handoff) → consolidation; **review-4 APPROVED**; independent **opsec APPROVED** (0 confirmed,
1 candidate refuted at source). The lesson that recurred three times: after the product fix lands,
the rejections are provenance and lane hygiene — serve the exact reviewed bundle, refresh the durable
evidence, keep the ledger shape legal.
