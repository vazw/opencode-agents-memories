# Issue 118 wave 3 browser evidence — the three worst screens (r1 run, 2026-11-06; r3 rework re-capture, 2026-11-06)

All figures below are **MEASURED** unless labelled otherwise, by
`web-ui/scripts/capture-evidence.cjs` (plus the step-6 docHeight lane script)
against one lane in one run. Blueprint:
`.tmp/blueprints/blueprint-20261106-issue-118-webui-wave3-worst-screens-r2.md`
(stamp `sha256=36ff0e12949c`, SYS-APPROVED 2026-11-06T00:55Z).

**Bundle provenance (r3 rework):** every figure and `*-r2.png` screenshot below
was re-captured 2026-11-06T04:50–04:53Z on the **HEAD bundle
`web-ui-72f5eaea5edb020c`** — `trunk build --release` from the r3 rework HEAD
(review-1 Finding 1's 4-site tracked-read fix), served by a disposable engine.
The r1 pass had captured the older `web-ui-4302a4d03e9237ed` bundle (built
mid-step-7, before the last source edits) and declared no bundle id at all —
the wave-2 empty-memories census row below was carried un-refreshed from that
older capture. The bundle id is now declared next to each capture set so
stale-bundle drift is legible in the filenames (ADR-0003 content-hash scheme).

## Lane

- Engine: `target/release/opencode-agents-memory --mode=standalone
  --storage-root /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/worktrees/issue-118-w3/.tmp/scratch/issue118-w3-r3/store
  --web-port 3795 --web-dist .tmp/scratch/issue118-w3-r3/dist` (worktree
  `issue-118-w3`, r3 rework; the r1 lane on 3777 stayed untouched).
- **The storage root MUST be an absolute path.** MEASURED: the same release
  binary on the same source answers `/api/tags` 200 with an absolute root and
  500 with a relative one (`metadata.db` never materialises). Wave 2's lane
  used absolute paths too; the failure mode is pre-existing engine behaviour,
  not a wave-3 regression, and is documented here so the next lane does not
  re-derive it.
- Fixture: fresh store + 15 project memories POSTed through the REST API
  (container tag `opencode_project_aaaaaaaaaaaaaaa1`, ~210-char content each)
  so the Memories cards render at true height for the docHeight measurement
  and page 2 of the pager holds rows (15 = 8 + 7).
- Chrome: the headful chromium on CDP 127.0.0.1:9222 (the MCP browser), window
  sized so the CSS viewport reads **exactly 1280×800** (the script records the
  live `innerWidth x innerHeight` with every row and the docHeight lane refuses
  to measure outside the band; this window's chrome needs a 1300×907 outer
  window — the display's fractional DPR makes integer sizing viewport-dependent,
  so measure the inner viewport, never assume it).

## The discriminating docHeight measurement (step 6's ruling, re-derived on the HEAD bundle)

`document.documentElement.scrollHeight` at the exact 1280×800 CSS viewport,
readiness-gated (no spinner, no `*-loading` testid), release build
`web-ui-72f5eaea5edb020c`, populated lane (15 memories, page size 8):

| page size | rows rendered | docHeight (px) | ≤ 2500? |
| --- | --- | --- | --- |
| 10 (draft) | — | **3835** (r1 lane bundle) | no |
| 8 (else-branch) | 8 | **3322** (HEAD bundle, MEASURED 2026-11-06T04:53Z) | no |

**Ruling: `MEMORY_PAGE_SIZE = 8`.** The drafted 10 failed the bar (3835 > 2500
on the r1 bundle), the blueprint's "else 8 and re-measure" branch was taken,
and the re-measure is recorded. The r1 8-row number (3128 on the stale bundle)
moved to **3322** on the HEAD bundle — the ruling is unchanged (3322 > 2500,
and the reviewer's independent 3130 @ 8 on a current build already showed the
stale-bundle number was not the decision's basis); the residual height comes
from the 320-char previews keeping each card ~380 px tall — that is the
documented remaining state, not something a fourth run can ratchet without a
blueprint amendment. The 10-row row is marked r1-lane rather than re-measured:
10 is no longer a reachable UI state (`MEMORY_PAGE_SIZE = 8` is compiled in),
and the ruling it decided is not in question.

Re-run (from the worktree, engine up, lane browser tab open):

```sh
node .tmp/scratch/issue118-w3-r3/docheight-r3.mjs
```

## Field label association (step 5's hard check, MEASURED)

`capture-evidence.cjs` now ends with the `memories-field-label-association`
fixture on `/project-memories` (hard check, no fallback — a failure stops the
run):

```json
{"searchRootPresent":true,"searchControlName":"q",
 "searchControlId":"3d4da667-…","labelFor":"3d4da667-…","labelForResolves":true}
```

- The search input's `id` is the thaw `Field` uuid (the explicit
  `id="memories-search"` is DROPPED per blueprint r2), `name="q"` survives, and
  `label[for]` resolves to that id — the association the a11y acceptance needs.
- The mutant this kills is the r1 shape (explicit id kept): the control keeps
  its id but `label[for]` stays pinned to the Field uuid, failing the
  `labelForResolves` clause while everything else reads green.
- The `data-testid="memories-search"` lives on the thaw Input's root `<span>`,
  the control is the inner `input` — the check resolves both.

## Steady-state census (MEASURED, engine 3795, bundle `web-ui-72f5eaea5edb020c`, readiness-gated, one pass)

| route | thaw-button | thaw-input | thaw-card | thaw-tag | spinner | message-bar | raw `<button>` | `.state-block` | memories-pager | docHeight |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `/` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 800 |
| `/project-memories` (populated, page 1 of 2) | 24 | 2 | 40 | 16 | 0 | 0 | 8 | 0 | 1 (Ready arm) | 3322 |
| `/tags` | 2 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 800 |
| `/user-profile` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 800 |
| `/settings` | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 800 |
| `/export-import` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 800 |
| `/skill-library` | 2 | 2 | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 800 |
| `/sessions` (empty store) | 1 | 0 | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 800 |
| `/agents` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 800 |
| `/stats` | 2 | 0 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 1202 |
| `/skill-library` form open (fixture) | 4 | 6 | 1 | 0 | 0 | 3 | 0 | 1 | 0 | 800 |

`STEADY-STATE CENSUS OK` printed with `busyAtCapture = 0` on all rows.

**r1 provenance correction (review-1 Finding 2).** The r1 README recorded the
empty-store Memories row as `state-block: 1` + `memories-pager: 1`, glossed
"shows the pager **inside** the Ready arm" (the r1 file's §90-92). That shape
is the wave-2 DOM (the pager rendered outside the state match at c835288);
wave-3 moved the pager inside the Loaded arm (`core_screens.rs:829-833`), so on
wave-3 code an empty/filtered store renders `memories-empty` with
`memories-pager: 0` — the pair the r1 row recorded cannot occur. It was a
pre-step-6 wave-2 capture carried into this README un-refreshed, alongside
screenshots taken on the stale `web-ui-4302a4d03e9237ed` bundle (the r1 pass
declared no bundle id, which is why the staleness went unnoticed). The rows
above are the wave-3 truth on the HEAD bundle: the populated page shows
`memories-pager: 1` **in the Ready arm** together with 8 memory rows and 0
state-blocks — the pager-over-a-state-block pairing is gone with the wave-2
DOM. The `/sessions` row is the real-engine empty store:
`sessions-no-projects` with the timeline layer suppressed — the populated-roster
arms are fabricated by the lane (next section) because no REST write path
exists for sessions.

The screenshot set (`*-r2.png`, re-captured on bundle `web-ui-72f5eaea5edb020c`)
covers all ten routes plus the skill-form fixture on the same pass. The
Sessions arm captures live in the lane transcript below (hard-checked
transitions); the r1 "Known limits" item about per-arm Sessions captures is
closed by the Sessions arms lane.

## The Sessions arms lane (r3 rework, review-1 Finding 1's permanent guard)

The census pass can never reach the roster-Ready arm on a real store (no REST
write path for sessions exists), so the lane fabricates the wire answers with a
page-level fetch override installed before the wasm boots and hard-checks every
arm transition. One pass on the HEAD bundle (2026-11-06T04:52Z, MEASURED):

```json
{"midLoad":{"loading":1,"rows":0},"rows":2,"loading":0,"picker":1,"orphanedBadges":1,"fixture":"sessions-arms-loading-to-rows"}
{"rosterUnavailable":1,"picker":0,"timelineLayers":0,"timelineFetches":0,"fixture":"sessions-arms-roster-unavailable"}
{"rosterError":1,"picker":0,"timelineFetches":0,"fixture":"sessions-arms-roster-error"}
{"sessionError":1,"picker":1,"loading":0,"fixture":"sessions-arms-timeline-error"}
{"sessionsEmpty":1,"picker":1,"fixture":"sessions-arms-timeline-empty"}
```

- `loading-to-rows` is THE discriminator against the frozen timeline layer
  (review-1 Finding 1): mid-load `sessions-loading:1` with the picker up, then
  rows render and the spinner clears once the 700 ms-delayed timeline fetch
  settles. RED on the committed r1 bundle (`web-ui-4302a4d03e9237ed`:
  `rows=0, loading=1` at t+3 s — the pinned spinner), GREEN here.
- The roster failure arms (501 → `sessions-roster-unavailable`, 500 →
  `sessions-roster-error`) prove the picker and the whole timeline layer are
  suppressed, and `timelineFetches: 0` — no project, no timeline request.
- The timeline failure arm proves `session-error` renders **with the picker
  intact** (`picker: 1`), the empty arm proves `sessions-empty` with the picker.
- Run just this lane with `SESSIONS_ARMS_ONLY=1`; the Field association check
  and the full pass stay the default.

## Class census (pre-commit floor)

```sh
node web-ui/scripts/check-class-coverage.mjs --min-emitted=179 --self-test
# sites=217 literal=193 const=19 brace=5 residue=0
# emitted=179 hooks=179 missing=0 dead=0 unresolved=0
```

Baseline 179 (wave 2 HEAD) → wave 3 end **179**: `+settings-by-type-cloud`,
`+settings-by-type-none`, `+memories-search-field`, `+memory-card-foot` (kept,
now on the thaw `CardFooter`), `+session-select-label`; −`maintenance-armed`,
−`memory-card-head`, −`memories-action-error`, −`orphaned-badge` (Badge swap
landed). The floor stays **179**, written into `.githooks/pre-commit` in this
step.

## F4 attr-forwarding findings (READ, thaw 0.4.8 source)

- `MessageBar` (message_bar.rs): root `<div>` carries `class_list!` + `role`
  ONLY — `attr:data-testid` has nowhere to land. Wrapper-div fallback used for
  both `maintenance-armed` and `memories-action-error` (no class, no census
  impact; the CDP census counts `thaw-message-bar` = 3 elements per bar, so
  the bars ARE in the DOM).
- `Badge` (badge.rs): root is `class_list!` only — same fallback for
  `orphaned-badge`.
- `Button.loading` is a real prop (button/mod.rs:43): the maintenance action
  button shows thaw's own spinner while `running`.
- `CardHeader`/`CardFooter` slots: the `card_header_description` slot needs the
  leptos `slot` keyword (`<CardHeaderDescription slot>`); the timestamp
  renders in the header's description line.

## Known limits

- The roster-failure and timeline arm captures are no longer a limit: the
  Sessions arms lane fabricates them per pass (section above). What the lane
  does NOT fabricate is the shrink-clamp's live refetch — still listed below.
- The shrink-clamp behaviour (`page_after_shrink`) is pure-function tested
  (60-test core_screens suite) + the server's `div_ceil` comparator is pinned
  upstream; a live post-shrink capture would need a delete-driven lane.
