# Issue 139 — Thaw adoption inventory

A measurement, not a migration. `web-ui/src` was scanned for every raw HTML
element kind it authors; each kind is ruled once in `ruling.md`; a browser probe
measured whether Thaw forwards `attr:` to the `Table` and `ProgressBar` roots;
and a per-route walk recorded what each of the ten routes actually renders.

Everything here was measured at HEAD `d58a94f`. Nothing under `web-ui/src`,
`web-ui/public`, `.githooks` or `docs/agents` changed.

## Artifacts

| file | what it is |
| --- | --- |
| `ruling.md` | the ruling: 31 kinds, one class each, plus the replace worklist |
| `probe-table-progress.md` | probe report: HEAD, bundle sha256, raw facts, three verdicts |
| `routes.json` | raw per-route walk output (this README's tables are derived from it) |
| `README.md` | this file |

Scripts: `web-ui/scripts/thaw-adoption-census.mjs` (census, `--self-test`,
`SELFTEST 54/54`) and `web-ui/scripts/thaw-census-browser.cjs` (browser probe
and walk, `--self-test`, `SELFTEST 15/15`). Fixture:
`web-ui/scripts/fixtures/issue-139-census.json`, one entry per
`(method, template)` pair from `paths.rs`.

## Source scan

31 kinds, 272 openers, 20 files. 25 kinds are fixed; 6 are probe-dependent
(`table`, `thead`, `tbody`, `tr`, `td`, `progress`).

Replace occurrences: 20, all in `core_screens.rs` (`table` 2, `thead` 2,
`tbody` 2, `tr` 4, `td` 9, `progress` 1). Every other kind is `no-equivalent`
(20) or `keep-native` (5 rows over four component contracts: `label`,
`select`/`option`, `input`, `textarea`).

## Probe verdicts

HEAD `d58a94f`, bundle sha256 `1332238b5efa` (recorded by the runner, not
recomputed by the census).

| verdict | value | measured fact |
| --- | --- | --- |
| `probe-table` | `landed` | `[data-testid=probe-table]` exists, `tagName === 'TABLE'`, classList has `thaw-table`, `.thaw-table-cell` count 1 |
| `probe-progress` | `landed` | `[data-testid=probe-progress]` exists, `tagName === 'DIV'`, `role === 'progressbar'` |
| `probe-progress-label` | `landed` | its `aria-label === 'probe'` — `attr:aria-label` reaches the root |

The landed branch sets `table` = `replace` with `class,data-testid`, the
`thead`/`tbody`/`tr`/`td` cluster = `replace` with `class`, and `progress` =
`replace` with `aria-label,class,value,max`. The other branch was never
measured, so it is not asserted: had the table probe dropped, the table cluster
would be `keep-native`/`no-equivalent`; had the label dropped, `progress` would
be `no-equivalent`.

## Per-route walk (AC1)

Every route settled, every route was measured, and no route recorded an api
miss. Counts are authored tags only: a Thaw-owned element (its own classList has
a `thaw-` token) is not authored, and its children still are. `thawInternal`
lists out-of-universe tags the walk saw inside a Thaw subtree — here only `path`
(the SVG child of a Thaw icon) — and they are never counted or gated.
`unclassedUnderThaw` is the token list the ruling must be read against, not a
second copy of the elements: every authored element inside a Thaw subtree,
listed once, never summed.

| route | status | counts | thawInternal | unclassedUnderThaw |
| --- | --- | --- | --- | --- |
| / | measured | div 14, dd 4, dl 4, dt 4, p 2, h1 1, h2 1, main 1, nav 1, section 1, span 1 | path | dd, div, dl, dt, h1, h2, main, nav, p, path, section, span |
| /project-memories | measured | div 45, span 27, li 15, h1 1, h2 1, main 1, nav 1, section 1, ul 1 | path | div, h1, h2, li, main, nav, path, section, span, ul |
| /tags | measured | div 14, section 4, h3 3, span 3, code 2, li 2, ul 2, a 1, h1 1, h2 1, main 1, nav 1 | path | a, code, div, h1, h2, h3, li, main, nav, path, section, span, ul |
| /user-profile | measured | div 14, span 10, section 4, h3 3, li 3, option 3, ul 3, code 1, h1 1, h2 1, label 1, main 1, nav 1, p 1, select 1 | path | code, div, h1, h2, h3, label, li, main, nav, option, p, path, section, select, span, ul |
| /settings | measured | div 14, dd 4, dt 4, section 2, span 2, dl 1, h1 1, h2 1, h3 1, main 1, nav 1, p 1 | path | dd, div, dl, dt, h1, h2, h3, main, nav, p, path, section, span |
| /export-import | measured | div 15, h1 1, h2 1, label 1, main 1, nav 1, section 1, span 1, textarea 1 | path | div, h1, h2, label, main, nav, path, section, span, textarea |
| /skill-library | measured | div 13, span 8, code 1, h1 1, h2 1, li 1, main 1, nav 1, section 1, ul 1 | path | code, div, h1, h2, li, main, nav, path, section, span, ul |
| /sessions | measured | div 14, span 7, h1 1, h2 1, label 1, li 1, main 1, nav 1, option 1, p 1, section 1, select 1, ul 1 | path | div, h1, h2, label, li, main, nav, option, p, path, section, select, span, ul |
| /agents | measured | div 14, span 5, h1 1, h2 1, label 1, li 1, main 1, nav 1, option 1, p 1, section 1, select 1, ul 1 | path | div, h1, h2, label, li, main, nav, option, p, path, section, select, span, ul |
| /stats | measured | td 250, p 221, tr 40, span 37, progress 32, th 21, div 15, h3 6, section 4, table 2, tbody 2, thead 2, h1 1, h2 1, main 1, nav 1 | path | div, h1, h2, h3, main, nav, p, path, progress, section, span, table, tbody, td, th, thead, tr |

`/stats` is the reason the probe branch matters: 250 `td`, 40 `tr`, 32
`progress` and 21 `th` authored elements on one route, rendered from the
fixture's 8 daily rows and 30 shard rows; the source holds 20 replace
occurrences in one file (the daily table and the shards table).

**Not rendered at HEAD.** The `Planned` arm (`screens.rs`
`ScreenState::Planned`, the `planned-notice` section) is not reached by any
route at HEAD — every route is `Ready` or `Live` — so its markup is absent from
the counts above. It is stated here as "not rendered", not as zeros: its kinds
(`section`, `h2`, `p`, `strong`, `ul`, `li`, `code`) are all already counted on
routes that do render them.

## Predicted vs measured

| prediction | measurement |
| --- | --- |
| `attr:data-testid` reaches the `TABLE` and `DIV` roots | MEASURED: both `landed`; the table root carries `thaw-table` and the cell, the progress root is `div[role=progressbar]` |
| `attr:aria-label` reaches the `ProgressBar` root | MEASURED: `aria-label === 'probe'` on the `div[role=progressbar]` |
| All ten routes settle and render their panels | MEASURED: 10/10 `measured`, 0 api misses, 0 `NOT MEASURED` |
| `/` and `/settings` measure only because `/api/health` is in the fixture | both routes measured with `/api/health` answered by the fixture; the counterfactual (health absent) was not measured, so it is not claimed |
| The `(method, template)` pair count is derived by the runner | MEASURED: 37 pairs over 32 distinct templates; `_PATHS` (31) ∪ `/api/health` = 32, empty symmetric difference |

No prediction missed. The walk reports `apiMisses: []` for every route.

## Decisions

**D1 — is design-system consistency alone "strictly better"?** No for a single
kind, and the ruling says so: 25 of 31 kinds stay as they are, on semantics
(`ul`/`li`/`dl`/`dt`/`dd`/`section`/landmarks/headings/`th`) or on native
contracts (`label`, `select`, `option`, `input`, `textarea`). The one cluster
that moves is the table, and it moves because the probe landed and
`.thaw-table-cell` is present — a Thaw table keeps its row/cell structure,
which is the only thing that makes the swap more than a class change.

**D2 — should `progress` be replaced?** The ruling takes the landed branch:
`ProgressBar` roots in `div[role=progressbar]` and forwards the accessible name.
The cost is explicit and belongs to 140: a UA-styled `<progress>` becomes a
percent-width bar; `.stats-progress` (`public/styles.css:1174`) and its doc
comment (`:1056`) must be deleted in the same change or `check-class-coverage`
reports `dead>0`; and `peak` (`daily_peak`, `core_screens.rs:1969`) can be 0,
where Thaw's width is `value/max*100` (NaN) while native `<progress max=0>` is
indeterminate. 140 must handle `peak = 0` explicitly. This remains an owner
call for 140's landing, not a measurement 139 can settle.

## Limits

- Nothing runs the census automatically. With the pre-commit hook cut, "cannot
  silently grow back" is enforced only when someone runs
  `thaw-adoption-census.mjs`. 139's AC5 is about the script's exit code, not
  about enforcement. Reinstating a hook needs an AC that names it; none does at
  139.
- Per-route counts are a single browser run at HEAD, not a gate. A later change
  can move them without failing anything.
- The bundle sha256 in the probe report is recorded by the runner and is not
  recomputed by the census. The census checks that the line is present and
  well-formed, not that it matches the dist on disk.
- An unclassed element a Thaw component renders internally (no `thaw-` token,
  for example the `svg` of a Thaw icon) is counted as authored, because the walk
  cannot tell it from markup we wrote. The walk prints each such element's
  `localName` and parent class in `unclassedUnderThaw`, so the README can rule
  it. Until ruled, it sits outside the gate unless its tag is in the source
  scan.
- An `x <p` with a space before `<` and none after is markup-shaped and counts
  as an opener. `rustfmt` writes `x < p` (space both sides), which is not an
  opener, so the real tree is unaffected. Measured: the real tree yields 272
  openers before and after the `)`, `]`, `'`, `}` exclusion, all in-universe.
- Once the ruling is committed, the probe report's `HEAD:` becomes stale and a
  rerun of the census exits 3 (`probe stale`). That is expected drift, not a
  regression. Re-run the probe at the new HEAD to refresh the report.
- Thaw-internal tags absent from the source scan are reported in `thawInternal`
  and not gated. A Thaw-internal tag that the source later starts to use is then
  gated by the ordinary rule.
- A ruling row's `basis` citation is parsed and then dropped. `parseRuling` in
  `web-ui/scripts/thaw-adoption-census.mjs` validates a row's shape, class,
  `attrs-allowed`, duplicates and markers; `checkPolicy` never reads `row.basis`.
  So a `basis` cell is a comment, not a contract: a cite pointing at the wrong
  element is indistinguishable from a correct one to every artifact here. The
  review-1 defect on the `input` row (`READ skill_library.rs:695`, which is the
  `<textarea`) got through exactly that way, and AC4's "name the three testids"
  is likewise prose no check inspects. The guard is deferred: this ruling is a
  dated artifact that issue 140 consumes rather than regenerates, so a checker
  whose lifetime is one issue would be scaffolding. Upgrade trigger: any future
  edit or regeneration of `docs/evidence/thaw-adoption-census/ruling.md`. What to
  add, in `checkPolicy`: (1) a `basis mismatch` finding — for a `basis` matching
  `READ <file>:<line>`, scan that line and require its opener tag to equal the
  row's kind (skipping directory cites and Thaw files outside `--src`); (2) a
  `missing contract` finding for the three select testids, asserted against each
  `keep-native` `select`/`option` row's reason cell. Both are exit 1 and belong
  to the violation class, not to the two exit-3 producers.

## Closure

139 closes as measurement if the probe report holds a landed or dropped verdict
for table, progress and label, with HEAD equal to the current HEAD and a bundle
sha256 line present (recorded, not recomputed); the ruling's replace set equals
the Step 5 branch; the census exits 0 with unclassified=0 stale=0 violations=0;
and routes.json holds a measured or NOT MEASURED status for every route with its
reason. If the probe or the walk harness fails, 139 stays open and nothing lands
except the probe report.

Inverted world: this probe landed, so 140 has a worklist of six kinds / 20
occurrences and a ruling to follow. Had it dropped, the replace set would be
empty or `progress`-only and 140 would collapse toward zero.

Closure runs, at HEAD `d58a94f`:

```
node web-ui/scripts/thaw-adoption-census.mjs --self-test
SELFTEST 54/54                                                      (exit 0)
node web-ui/scripts/thaw-census-browser.cjs --self-test
SELFTEST 15/15                                                      (exit 0)
node web-ui/scripts/thaw-adoption-census.mjs --src=web-ui/src \
  --ruling=docs/evidence/thaw-adoption-census/ruling.md \
  --probe=docs/evidence/thaw-adoption-census/probe-table-progress.md
census kinds=31 unclassified=0 stale=0 replace=6 worklist=6 violations=0 drift=0 harness=0 exit=0
node web-ui/scripts/thaw-adoption-census.mjs --src=web-ui/src \
  --routes=docs/evidence/thaw-adoption-census/routes.json
census kinds=31 unclassified=0 stale=0 replace=0 worklist=0 violations=0 drift=0 harness=0 exit=0
```
