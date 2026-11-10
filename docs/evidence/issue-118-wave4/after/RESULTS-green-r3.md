# Wave 4 dist-after-r3 Evidence (RESULTS-green-r3)

## Bundle identity (served on `http://127.0.0.1:49418`)

- Bundle ID: `web-ui-ca2b5df9fdce9bc9`
- Page/MCP target: `F84E3B0582B22A8842B5F70B90E6CDEA`
- UTC: `2026-11-10T14:19:57Z` (W4_TEST=all run start)
- Serve root: `.tmp/scratch/issue-118-w4/dist-after-r3` (no source/CSS/script/fixture edits after build)

## Served asset SHA256 (disk == served, verified via `curl` before browser runs)

| Asset | SHA256 |
|-------|--------|
| `index.html` | `8fafebd2e8b49e313cc832b86a5f189e76fd1ef3dce54d7661f19f3788331b74` |
| `styles-590dfeed23cfe8d7.css` | `523cce1f00075bac6639a0fdeecd85a3e4992714449871493c0304ff7e292d99` |
| `web-ui-ca2b5df9fdce9bc9.js` | `bd0fe74d908dc76a1cae9ecd8f0de1b24931d158727da39688cb8279b14dc158` |
| `web-ui-ca2b5df9fdce9bc9_bg.wasm` | `5d340c0fd788904aba95e23b5ca557fd1a136b42a09677ef4e85d1339938987c` |

## Script / fixture pins

| Pin | Value |
|-----|-------|
| `scriptSha256` (`web-ui/scripts/test-wave4-cdp.cjs`) | `923d0558f5a30f7d269f0db47bb9a2479cdc6932dde65b660c1085ba3c6d9515` |
| `fixtureSha256` (`web-ui/scripts/fixtures/issue-118-wave4.json`) | `517b3dca8a8f8c177cc36f4c8a0d0143e65b7deeb3f81a36d7e894bd6c3af89f` |

## W4_TEST=all — 12/12 PASS (all exact-case names, no invented RETRY)

| # | Case | Status |
|---|------|--------|
| 1 | `home_independent_resources` | PASS |
| 2 | `profile_picker_label` | PASS |
| 3 | `skills_filtered_empty` | PASS |
| 4 | `stats_accessible_counters_and_shards` | PASS |
| 5 | `skills_form_save_non_submit` | PASS |
| 6 | `skills_form_cancel_non_submit` | PASS |
| 7 | `skills_form_enter_prevented` | PASS |
| 8 | `portability_existing_contract` | PASS |
| 9 | `stats_existing_content` | PASS |
| 10 | `tags_unchanged_matrix` | PASS |
| 11 | `native_select_and_route_regressions` | PASS |
| 12 | `matrix_ready_captures` | PASS |

**Totals: 12/12 PASS, 0 FAIL, 0 SKIP.**

### `matrix_ready_captures` — 24 captures × 3 ready samples, overflow false

6 routes × 2 themes × 2 viewports. Ready-sample loop: every 100 ms verify terminal scenario marker and no spinner/loading testid; require 3 consecutive successes; reset on busy; 20 s timeout fails.

| Route | Theme | Width | Screenshot | Ready samples | Overflow |
|-------|-------|-------|------------|---------------|----------|
| `/` | light | 1280 | `home-light-1280.png` | 3 | false |
| `/` | light | 390 | `home-light-390.png` | 3 | false |
| `/` | dark | 1280 | `home-dark-1280.png` | 3 | false |
| `/` | dark | 390 | `home-dark-390.png` | 3 | false |
| `/user-profile` | light | 1280 | `user-profile-light-1280.png` | 3 | false |
| `/user-profile` | light | 390 | `user-profile-light-390.png` | 3 | false |
| `/user-profile` | dark | 1280 | `user-profile-dark-1280.png` | 3 | false |
| `/user-profile` | dark | 390 | `user-profile-dark-390.png` | 3 | false |
| `/tags` | light | 1280 | `tags-light-1280.png` | 3 | false |
| `/tags` | light | 390 | `tags-light-390.png` | 3 | false |
| `/tags` | dark | 1280 | `tags-dark-1280.png` | 3 | false |
| `/tags` | dark | 390 | `tags-dark-390.png` | 3 | false |
| `/export-import` | light | 1280 | `export-import-light-1280.png` | 9 | false |
| `/export-import` | light | 390 | `export-import-light-390.png` | 9 | false |
| `/export-import` | dark | 1280 | `export-import-dark-1280.png` | 9 | false |
| `/export-import` | dark | 390 | `export-import-dark-390.png` | 9 | false |
| `/skill-library` | light | 1280 | `skill-library-light-1280.png` | 3 | false |
| `/skill-library` | light | 390 | `skill-library-light-390.png` | 3 | false |
| `/skill-library` | dark | 1280 | `skill-library-dark-1280.png` | 3 | false |
| `/skill-library` | dark | 390 | `skill-library-dark-390.png` | 3 | false |
| `/stats` | light | 1280 | `stats-light-1280.png` | 3 | false |
| `/stats` | light | 390 | `stats-light-390.png` | 3 | false |
| `/stats` | dark | 1280 | `stats-dark-1280.png` | 3 | false |
| `/stats` | dark | 390 | `stats-dark-390.png` | 3 | false |

## Export geometry — dist-after-r3 (probes/export-geometry.json)

Export download/import label positively separated (vGap >= 12 CSS px at all 4 combos); page overflow false.

| Theme | Width | vGap (CSS px) | hGap (CSS px) | marginBottom | display | pageOverflow |
|-------|-------|---------------|---------------|--------------|---------|--------------|
| light | 1280 | 16.81 | -130.53 (stacking axis) | 16px | block | false |
| light | 390 | 16.81 | -130.53 (stacking axis) | 16px | block | false |
| dark | 1280 | 16.81 | -130.53 (stacking axis) | 16px | block | false |
| dark | 390 | 16.81 | -130.53 (stacking axis) | 16px | block | false |

## Console / network — dist-after-r3 (probes/console-network.json)

6 routes, 3 ready samples each, served bundle `dist-after-r3 web-ui-ca2b5df9fdce9bc9`:

| Metric | Count |
|--------|-------|
| Exceptions | 0 |
| Console errors/warnings | 0 |
| Failed requests | 0 |
| Bad responses | 0 |

## Pure wasm suite (discrete evidence)

`cargo test --target wasm32-unknown-unknown` — exit 0; `.tmp/scratch/issue-118-w4/wasm-suite-final.log`:

- **241 passed** across all suites, **0 failed**, **0 ignored**
- **20 `test result:` lines** across all test modules
- **21 `Running` sections** (header count)

## Disclosures (dist-after-r3 closeout)

- **Dist-after-r2 invalidation:** `dist-after-r2` (`web-ui-e573af5699f970a8`) artifacts previously archived under `after/` are superseded. Source/CSS/script edits landed before `dist-after-r3` was built (build 14:04–14:11Z); `dist-after-r2` browser evidence is invalidated. Historical `r2` runs remain in `.tmp/progress/issue118-w4-green-v2/` and `.tmp/progress/issue118-w4-green-v3/`.
- **A2 intermediate script hash loss:** Earlier script revisions (`b24b959d`, `6077dde9`, `ada991ca`, `a03f4563`) exist only as hashes on historical records. The pre-`A1/A4/A5` revision `b1437980` is archived at `.tmp/scratch/issue-118-w4/scripts/test-wave4-cdp.b1437980.cjs`. Current script hash `923d0558` pins the `A1/A4/A5` populated-payload + exact-select-assertions build.
- **A1 resolution:** `*/milestones` now returns one row with non-empty `agentId` (`fixture-agent`), exercising `AgentsView::Roster`; `*/sessions` returns one `SessionRow` exercising `SessionState::Ready`; `sessionCount:1, milestoneCount:1` keeps `project_option_label` consistent. Ready-arm regression lanes now run against populated fixtures on served `dist-after-r3`.
- **Skill-delete prompt disclosure (residual):** MEASURED in `before/extra.json` — skill-delete confirmation exposes prompt-specific erasure disclosure. This wave does NOT modify shared confirmation infrastructure; separately bounded approval/fix required (not fixed here).
- **Out-of-scope residuals:** successful Save write path, list-error recovery, Profile races/missing transitions, Stats stale-snapshot reload failure, delayed export readiness/prompt exclusion beyond existing geometry evidence, authenticated-header behavior beyond synthetic marker, actual keyboard shard traversal beyond bounded local region lane, live staging, commit/push (unmarked until Step 11/12).
- Owner disk correction: 300 GB is closeout reporting/cleanup guidance, not a mid-wave stop threshold. No permission change, inaccessible traversal, target deletion, or cleanup authorized by this closeout.
