# Issue 118 wave 3 browser evidence — the three worst screens (r1 run, 2026-11-06)

All figures below are **MEASURED** unless labelled otherwise, by
`web-ui/scripts/capture-evidence.cjs` (plus the step-6 docHeight lane script)
against one lane in one run. Blueprint:
`.tmp/blueprints/blueprint-20261106-issue-118-webui-wave3-worst-screens-r2.md`
(stamp `sha256=36ff0e12949c`, SYS-APPROVED 2026-11-06T00:55Z).

## Lane

- Engine: `target/release/opencode-agents-memory --mode=standalone
  --storage-root /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/worktrees/issue-118-w3/.tmp/issue118-w3-lane
  --web-port 3777 --web-dist web-ui/dist` (worktree `issue-118-w3`).
- **The storage root MUST be an absolute path.** MEASURED: the same release
  binary on the same source answers `/api/tags` 200 with an absolute root and
  500 with a relative one (`metadata.db` never materialises). Wave 2's lane
  used absolute paths too; the failure mode is pre-existing engine behaviour,
  not a wave-3 regression, and is documented here so the next lane does not
  re-derive it.
- Fixture: fresh store + 13 project memories POSTed through the REST API
  (container tag `opencode_project_aaaaaaaaaaaaaaa1`, ~210-char content each)
  so the Memories cards render at true height for the docHeight measurement.
- Chrome: the headful chromium on CDP 127.0.0.1:9222 (the MCP browser), window
  resized via `Browser.setWindowBounds` to 1427×987 outer = **1279×800 CSS
  viewport** (the display's fractional DPR 1.3383 makes an exact 1280×800
  inner viewport unattainable by integer window sizing; 1279 vs 1280 cannot
  move the ruling below — the decision gap is ~1250 px wide).

## The discriminating docHeight measurement (step 6's ruling)

`document.documentElement.scrollHeight` at the 1279×800 CSS viewport,
readiness-gated (no spinner, no `*-loading` testid), release build:

| page size | rows rendered | docHeight (px) | ≤ 2500? |
| --- | --- | --- | --- |
| 10 (draft) | 10 | **3835** | no |
| 8 (else-branch) | 8 | **3128** | no |

**Ruling: `MEMORY_PAGE_SIZE = 8`.** The drafted 10 failed the bar (3835 > 2500),
the blueprint's "else 8 and re-measure" branch was taken, and the re-measure is
recorded (3128 > 2500). The blueprint authorizes no third value, so 8 stands;
the residual height comes from the 320-char previews keeping each card ~340 px
tall — that is the documented remaining state, not something a fourth run can
ratchet without a blueprint amendment.

Re-run (from the worktree, engine up, lane browser tab open):

```sh
TOKEN=$(cat .tmp/issue118-w3-lane/.auth-token)
LANE_TOKEN=$TOKEN node .tmp/issue118-w3-lane/docheight4.mjs
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

## Steady-state census (MEASURED, lane 3777, readiness-gated, one pass)

| route | thaw-button | thaw-input | thaw-card | thaw-tag | spinner | message-bar | raw `<button>` | `.state-block` | memories-pager | docHeight |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `/` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 758 |
| `/project-memories` (empty store before fixture) | 7 | 2 | 0 | 0 | 0 | 3 | 0 | 1 | **1 (Ready arm)** | 758 |
| `/tags` | 2 | 0 | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 758 |
| `/user-profile` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 758 |
| `/settings` | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 758 |
| `/export-import` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 758 |
| `/skill-library` | 2 | 2 | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 758 |
| `/sessions` | 2 | 0 | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 758 |
| `/agents` | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 758 |
| `/stats` | 2 | 0 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 1097 |
| `/skill-library` form open (fixture) | 4 | 6 | 1 | 0 | 0 | 3 | 0 | 1 | 0 | 758 |

`STEADY-STATE CENSUS OK` printed with `busyAtCapture = 0` on all rows. The
empty-store Memories row shows the pager **inside** the Ready arm (wave 3:
the pager does not render on the Empty/Loading/Error arms — the fixture's
pre-population screenshots would otherwise show a pager over "no memories").

The screenshot set (`*-r2.png`) covers all ten routes plus the skill-form
fixture on the same pass. The wave-3-specific arm captures (populated Memories
Ready + post-shrink behaviour, Sessions roster-failure + empty-timeline arms)
require a populated/failed lane per arm and are listed under "Known limits".

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

- The per-arm Sessions captures (roster `Unavailable`/`Failed` arms) need a
  lane whose `/api/projects` 501s or fails; the empty-store lane renders
  `sessions-no-projects` (arm verified live in wave 2's `/agents` row, and the
  arm mapping is table-tested in the wasm suite).
- The shrink-clamp behaviour (`page_after_shrink`) is pure-function tested
  (60-test core_screens suite) + the server's `div_ceil` comparator is pinned
  upstream; a live post-shrink capture would need a delete-driven lane.
