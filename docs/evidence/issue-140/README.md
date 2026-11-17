# Issue 140 — Thaw adoption, applied to its only screen (`/stats`)

Blueprint `issue140-thaw-adoption-implementation-r7` (`sha256=f3fc10caaf9b`,
SYS-APPROVED 2026-11-17T19:40Z), executed at HEAD `fb80d18` on 2026-11-18 (UTC 2026-11-17T19:41–19:56).
No commit was made during the wave; the transport is `.tmp/scratch/issue140/u1.patch` plus the staged
bundles `stage/u1-dist` and `stage/u2-dist`.

## AC1 as amended 2026-11-18

Quoted from `.scratch/rust-port/issues/140-thaw-adoption-implementation.md:21`:

> The six `replace` kinds are no longer authored in `web-ui/src`, and every remaining raw element there
> is `keep-native` or `no-equivalent`. Operative evidence is the walk (`/stats` contains no raw
> `table|thead|tbody|tr|td|progress`; `th` stays 21) plus the census reporting exactly
> `kinds=25 unclassified=0 stale=13 replace=6 worklist=6 violations=0 drift=13 harness=0 exit=3`,
> recorded in `docs/evidence/issue-140/README.md`.

## Census (`thaw-adoption-census.mjs --src=web-ui/src --ruling=… --probe=…`)

```
census kinds=25 unclassified=0 stale=13 replace=6 worklist=6 violations=0 drift=13 harness=0 exit=3
exit=3
```

`exit=3` is by design: the six `replace` rows in `ruling.md` still read stale against the current HEAD,
and the six worklist findings are now *overstated* (nothing left to do). `replace=6` and `worklist=6` are
ruling-side invariants; this issue consumes `ruling.md` rather than regenerating it.

## Walk — `/stats` authored tags (`thaw-census-browser.cjs --walk`)

| run | authored tags on `/stats` |
| --- | --- |
| baseline (HEAD `fb80d18`) | `table 2, thead 2, tbody 2, tr 40, td 250, th 21, progress 32` |
| after U1+U2 | `div 15, h1 1, h2 1, h3 6, main 1, nav 1, p 221, path 11, section 4, span 37, th 21` |

No `table|thead|tbody|tr|td|progress` remains; `th` stays 21 (all native, including the eight
`scope="row"` cells — `ruling.md:44`).

## Lane — baseline vs after (18 cases: 13 pre-existing + 5 issue-140)

Seam: `test-wave6-cdp.cjs` on the served dist, `127.0.0.1:49418`.

| case | baseline (pristine bundle) | after (U1+U2) |
| --- | --- | --- |
| 13 pre-existing wave-6 cases | PASS 13/13 | PASS 13/13 |
| `stats_table_geometry` | PASS (`auto`/`auto`), pairs 907/907, 892/892, 892/892 | PASS (`auto`/`auto`), pairs 907/907, 892/892, 892/892 |
| `stats_tables_are_thaw_tables` | **FAIL** `daily root lacks thaw-table` | PASS: both roots `TABLE` + `thaw-table`, cells 40/210, `tbody tr` 8/30, `th` 21, header-cell wrappers 0 |
| `stats_progress_contracts` | **FAIL** `four progress bars in the 2026-11-08 row, got 0` (a native `<progress>` has no `role`) | PASS: labels `2026-11-08 {Captures,Skipped,Profile,Compaction}`, `aria-valuenow` 4,0,2,0, `aria-valuemax` 11 on all four, height 12 (`0.75 × 16`), width 128, inner ratio 36.36 %/0 %/18.18 %/0 % |
| `native_select_contracts` | PASS — `/agents` 1 option, `/sessions` 1, `/user-profile` 3 | PASS — same three |
| `stats_renders_clean` | PASS `{n:1, stats:[200], rows:8, pageErrors:0}` | PASS `{n:1, stats:[200], rows:8, pageErrors:0}` |

The two baseline failures are the RED record for the swap; the adopted tree is the only state in which
both new contracts hold. Baseline: `.tmp/scratch/issue140/baseline/results.json`; after:
`.tmp/scratch/issue140/u2/results.json`.

## `table-layout` — the geometry oracle and its mutant

`stats_table_geometry` asserts `getComputedStyle(root).tableLayout === 'auto'` for `.stats-daily` and
`.stats-shards`, and *returns* the per-root scroll pairs instead of asserting them (under `fixed` the
columns clip rather than widen, so a scroll diff cannot red the mutant and would red the wanted state).

| root | baseline `scrollWidth/clientWidth` | baseline `tableLayout` | after `scrollWidth/clientWidth` | after `tableLayout` |
| --- | --- | --- | --- | --- |
| `.stats-daily` | 907 / 907 | `auto` | 907 / 907 | `auto` |
| `.stats-shards` | 892 / 892 | `auto` | 892 / 892 | `auto` |
| `.stats-shards-region` | 892 / 892 | — (a `div`) | 892 / 892 | — |

Both roots are unchanged by the swap, so the dropped 2 px comparison hid no overflow. Mutant, run rather
than described (Step 3a): with the mapping compiled and `table-layout: auto` **absent**, both roots
computed `fixed` and the case exited 1 —

```
FAIL stats_table_geometry: tableLayout {"daily":"fixed","shards":"fixed"}
geometry_exit=1
```

Adding the one declaration to `.stats-daily, .stats-shards` turns that into `geometry_exit=0`. Nothing
else declares `table-layout` on either class, so the flip is the declaration's, not a tie-break.

## The shared fixture was extended — and why

`web-ui/scripts/fixtures/issue-118-wave6.json` now carries one `projects` row and a second `roster` row.
As shipped (`projects: []`, one-row roster) the app mounts **zero** `<select>` on all three routes the
`native_select_contracts` case visits: `projects_view` returns `ProjectsView::NoProjects`
(`agents.rs:573-578`), the `/sessions` picker sits behind the same `Ready` arm (`screens.rs:530-531`,
`:735-737`), and a one-row roster makes `profile_identity_control` return `IdentityControl::Hidden`
(`core_screens.rs:1540-1541`). With the extension all three resolve (`SELECT` with 1 / 1 / 3 options) and
the thirteen pre-existing cases still pass 13/13 — measured: the lane's cases visit only
`/project-memories` and `/stats`, so `projects`, `roster` and the identity-control state cannot move them.
Fixture sha256 `64667e72d1f66bca044704b72973dd52973c4c0e74c7c0f04c40ce41005cca5e`.
`u1.patch` carries the fixture on purpose: the U2 rollback reverts the same four paths with
`git checkout --`, so a patch without it would restore `projects: []` and leave a state where
`native_select_contracts` reds.

## Accepted cosmetic deltas (not fought with CSS)

- `.thaw-table-cell { height: 44px }` (`thaw-0.4.8/src/table/table.css:101`) — rows are taller than the
  native reading. Not a contract.
- `.thaw-table { background-color: var(--colorSubtleBackground) }`
  (`thaw-0.4.8/src/table/table.css:7`) — the tables now paint a surface tint.

## Provenance — the after bundle (`.tmp/scratch/issue140/u2/results.json`, utc `2026-11-17T19:50:08.152Z`)

| file | sha256 |
| --- | --- |
| `index.html` | `cd3b0d23488d546ef15d058a629c7be7ea72b48fbd2b4306da25c65d93c3e107` |
| `styles-dcdac4570cd82bb2.css` | `53292b59663219533dded473d650347da93afdcf7c9a04f76e978478ce2d7343` |
| `web-ui-2bb1e7f022a44e9d_bg.wasm` | `6fea7f2f6372f5ec9ef6f75fb76870d4522ce4771649e8c4f021d29ef9cf725e` |
| `web-ui-a55d4d337bfbc403.js` | `001e2e0aacd4c3887f7eba80b05d26c2cd48338984151c3f79b65776a7e98a22` |

The lane verifies that the served bytes equal these disk bytes before any case runs.
`scriptSha256 a546e07822d77e87de16cca687a7fffe2d144bf08e84526cca52e63f2cf3a4c5`,
`fixtureSha256 64667e72d1f66bca…`. No probe re-run: `--probe` would rewrite `probe-table-progress.md`
at the current HEAD and dissolve the stale finding.

## Gates run (all from the repo root; cargo in subshells)

| gate | result |
| --- | --- |
| `(cd web-ui && cargo fmt --all --check)` | exit 0 |
| `(cd web-ui && cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used)` | exit 0 |
| `(cd web-ui && cargo clippy --workspace --all-targets -- -D warnings)` | exit 0 |
| `(cd web-ui && cargo test --target wasm32-unknown-unknown)` | exit 0 — **21 test binaries** (2 unittests + 19 `tests/*.rs`), `core_screens` 73 passed, 264 passed / 0 failed, doc-tests 0 |
| `check-class-coverage.mjs --self-test --min-emitted=191` | `SELFTEST 9/9`, `emitted=191 hooks=191 missing=0 dead=0 unresolved=0` |
| `check-token-coverage.mjs --self-test` | `SELFTEST 2/2`, `missing=0 dead=0` |
| `thaw-adoption-census.mjs --self-test` | `SELFTEST 54/54` |
| `thaw-census-browser.cjs --self-test` | `SELFTEST 15/15` |
| `thaw-census-browser.cjs --walk --dist=…/dist --out=…/routes-after.json` | exit 0, `walked 10 routes` |
| `thaw-adoption-census.mjs --src=web-ui/src --ruling=… --probe=…` | exit 3 by design (line above) |
| U2 mapping `(cd web-ui && cargo check --target wasm32-unknown-unknown)` | exit 0 — the `date` hoist is present; without it the crate reds `E0521` at the `<TableRow>` `view!` |

The `.stats-progress` class and its rule both survive (`public/styles.css:1174`, fact 2): deleting either
alone reds the class census with `missing=1` / `dead=1`, and deleting both reds the ratchet at
`emitted=190`.

## Rollback (per unit; never a wave revert, no commits)

U2 wrong → back to U1 (source and bundle together):

```sh
find .tmp/scratch/issue140/dist -mindepth 1 -delete      # else U2's hashed bundles sit beside U1's
git checkout -- web-ui/src/core_screens.rs web-ui/public/styles.css \
  web-ui/scripts/test-wave6-cdp.cjs web-ui/scripts/fixtures/issue-118-wave6.json
git apply .tmp/scratch/issue140/u1.patch
cp -R .tmp/scratch/issue140/stage/u1-dist/. .tmp/scratch/issue140/dist/
```

U1 wrong → back to HEAD:

```sh
find .tmp/scratch/issue140/dist -mindepth 1 -delete
git checkout -- web-ui/src/core_screens.rs web-ui/public/styles.css \
  web-ui/scripts/test-wave6-cdp.cjs web-ui/scripts/fixtures/issue-118-wave6.json
cp -R .tmp/scratch/issue140/dist-baseline/. .tmp/scratch/issue140/dist/
```

## Build hygiene — one deviation, recorded

The first `dist-baseline` build shipped a **swapped** wasm: `web-ui/target` had been written by the
gate-round `verify-ui` scratch copy, which shared `CARGO_TARGET_DIR=web-ui/target`, so cargo reported the
crate fresh and trunk served that artifact. Detected by the lane itself (a pristine tree passed
`stats_tables_are_thaw_tables` and redded geometry with `fixed`). Fixed by touching the crate roots
(`touch web-ui/src/{lib,main}.rs`) and rebuilding — `trunk build` exit 0, 31.46 s. The contaminated run was
discarded; the baseline recorded above is the rebuilt one (`utc 2026-11-17T19:44:48.582Z`), whose geometry
pairs and 13/13 pre-existing results match the blueprint's rehearsal. Anyone re-running the plan should
give this wave its own `CARGO_TARGET_DIR` or touch the crate roots first.
