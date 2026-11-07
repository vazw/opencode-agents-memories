# Tags-screen anchor contrast — 2026-11-07 wave

Defect (report, 2026-11-07): the Tags screen renders tag links as bare
`<a class="tags-tag">` anchors (`web-ui/src/core_screens.rs:1068`) with no
app anchor-colour rule, so they compute browser-default `rgb(0, 0, 238)`
underlined on the dark panel — unreadable. MEASURED live at
`http://127.0.0.1:3742/tags` (bundle `8b0751abba83`): 50 anchors at
`rgb(0, 0, 238)`, `text-decoration-line: underline`.

Fix: one zero-specificity base rule in the C1 block of
`web-ui/public/styles.css` — `:where(a) { color: var(--accent);
text-decoration-line: underline; }` — restyling the existing anchors by
cascade. No new class (census floor 175 unchanged), no `theme.rs` change
(Thaw's `colorBrandForegroundLink` stays mapped for future `thaw::Link`
mounts; today the app mounts none, so the variable has zero live consumers
in the DOM).

Blueprint: `.tmp/blueprints/blueprint-20261107-tags-contrast-r2.md`
(stamp `5bfd1ddc4c76`, SYS-APPROVED 2026-11-07T00:15Z). Worktree
`.tmp/worktrees/tags-contrast` at `8190de1`, branch `tags-contrast/wave`;
all changes left UNCOMMITTED for the integrator.

## Pre-change baseline (RED-2 static + census + live probes)

Pre-change build: `trunk build` → bundle `web-ui-259e2ecdd2554e09_bg.wasm`
(wasm-release profile, 2m 17s).

- RED-2 static: `grep -cE '^\s*:where\(a\)' web-ui/public/styles.css` →
  `0` (exit 1, no match — expected pre-change). The same anchored pattern
  prints `1` post-change.
- CONTROL-B class census:
  `node scripts/check-class-coverage.mjs --min-emitted=175 --self-test` →
  `emitted=175 hooks=175 missing=0 dead=0`, `SELFTEST 9/9`. Re-measured
  post-change and on the release dist: identical.
- CONTROL-A live nav probe: the conversion mechanism (token → detached
  probe span → `getComputedStyle` equality) verified live 2026-11-07 on
  the clean tree (fg2 `#e8eaed` → `rgb(232, 234, 237)`, equality true) and
  pinned by the lane's `nav_anchor_color_unchanged` row in both runs.

## TAGS lane — pre-change run (LANE_EXPECT=pre)

MEASURED 2026-11-07T03:44Z, before the CSS edit. Served bundle
`web-ui-259e2ecdd2554e09_bg.wasm` (sha256
`38dedc58bcc80d62975be0fb270220899a526130c0f6f73e7e5bc6dfa6a5d455`),
engine `--mode=standalone --storage-root .tmp/scratch/tags-contrast-lane
--web-port 49393 --web-dist web-ui/dist`, lane on CDP 9222.

```
$ TAGS_ONLY=1 LANE_EXPECT=pre TEST_BASE_URL=http://127.0.0.1:49393 WEBUI_DIST=$PWD/dist \
  LANE_TABLE=../.tmp/scratch/tags-lane-pre.md LANE_PAGE_ID=39A93FE63FBE169305F9DCA3B629FFB8 \
  node scripts/capture-evidence.cjs
rc=0  mismatches=0  stopped=false
```

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| tag_link_color_not_browser_default | RED | tag anchors on /tags | FAIL | FAIL | the tag anchor computes the browser default rgb(0, 0, 238) (underline: underline) |
| tag_link_resolves_accent_token | RED | tag anchors on /tags | FAIL | FAIL | the tag anchor computes rgb(0, 0, 238), not the accent token's rgb(122, 162, 247) (--accent=#7aa2f7) |
| tag_rule_declares_token_not_literal | RED | stylesheet text | FAIL | FAIL | no :where(a) rule found in bundle …/web-ui/dist/styles-6810bd120156bbcc.css |
| tag_rows_render_and_href_preserved | CONTROL | 2 ranked memory tags | PASS | PASS | links=2 hrefs intact (first="rust") |
| nav_anchor_color_unchanged | CONTROL | nav anchor vs fg2 token | PASS | PASS | nav=rgb(232, 234, 237) == probe-converted --colorNeutralForeground2=#e8eaed |

Pre-change RED-1 values: tag anchors compute `rgb(0, 0, 238)` (browser
default, underlined) while the accent token resolves to
`rgb(122, 162, 247)` (`#7aa2f7`) — reproducing the live 3742 measurement
on the worktree bundle. (The first pre attempt exited 1 on a harness
launch error — `TEST_BASE_URL` omitted, tab sat on an opaque-origin
failed load and `localStorage` threw; rerun with the correct base URL.
Not a fixture defect; recorded for the review's launch-note.)

## The fix (Step 3)

`web-ui/public/styles.css`, C1 base-control block, after the checkbox
rule (179-181), before the shared `:focus-visible` rule (183):

```css
/* Anchors outside Thaw's components get browser-default blue on the dark
   panel (Tags screen, 2026-11-07 report). Zero specificity so any real
   component rule — Thaw's nav items colour themselves — outranks it. */
:where(a) {
  color: var(--accent);
  text-decoration-line: underline;
}
```

## TAGS lane — post-change run (LANE_EXPECT=post)

MEASURED 2026-11-07T03:52Z after `trunk build`. Bundle wasm unchanged
(`web-ui-259e2ecdd2554e09_bg.wasm` — a CSS-only edit does not rehash the
wasm); stylesheet `styles-dfc26d953db5e467.css` (hash changed, as
expected).

```
$ TAGS_ONLY=1 LANE_EXPECT=post TEST_BASE_URL=http://127.0.0.1:49393 WEBUI_DIST=$PWD/dist \
  LANE_TABLE=../.tmp/scratch/tags-lane-post.md LANE_PAGE_ID=39A93FE63FBE169305F9DCA3B629FFB8 \
  node scripts/capture-evidence.cjs
rc=0  mismatches=0  stopped=false
```

| row | kind | fixture | outcome | expected | detail |
| --- | --- | --- | --- | --- | --- |
| tag_link_color_not_browser_default | RED | tag anchors on /tags | PASS | PASS | color=rgb(122, 162, 247) (not browser default), underline=underline |
| tag_link_resolves_accent_token | RED | tag anchors on /tags | PASS | PASS | color=rgb(122, 162, 247) == probe-converted --accent=#7aa2f7 |
| tag_rule_declares_token_not_literal | RED | stylesheet text | PASS | PASS | :where(a) { color: var(--accent); text-decoration-line: underline; } in bundle …/web-ui/dist/styles-dfc26d953db5e467.css |
| tag_rows_render_and_href_preserved | CONTROL | 2 ranked memory tags | PASS | PASS | links=2 hrefs intact (first="rust") |
| nav_anchor_color_unchanged | CONTROL | nav anchor vs fg2 token | PASS | PASS | nav=rgb(232, 234, 237) == probe-converted --colorNeutralForeground2=#e8eaed |

Gates after the fix (all MEASURED): both web-ui clippy gates green
(`cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used
-D clippy::expect_used` 59.9s; `cargo clippy --workspace --all-targets --
-D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants
-A clippy::doc_lazy_continuation` 31.7s — each Finished, warnings only the
pre-existing proc-macro-error2 future-incompat note); class census
175/175 SELFTEST 9/9; token coverage `missing=0 dead=0`.

## Live confirmation on a real engine serving the new bundle

The blueprint's 3742 re-measure could not be executed as written: every
standalone-launched engine (fresh 8190de1 binary AND the old staged
binary, on the live snapshot, on an empty scratch root, and in stdio
mode) answers uniform **silent 500** on every store-backed route
(`/api/tags`, `/api/memories`, `/api/stats`), while the same old binary
launched as the wrapper's bridge child (3742) answers 200. The 500s
carry no `api handler failed` log line — `port_failure` logs before
answering any mapped 500 — so they are the `CatchPanicLayer` panic path
(`crates/mem-web/src/lifecycle.rs:34`). This matches the issue-37
bug-pattern class in project memory (composition-root wiring visible only
through the binary's own launch path). OUT OF WAVE SCOPE: the wave
touches no Rust; recorded for syseng with the full launch matrix.

| launch | binary | storage root | /api/tags |
| --- | --- | --- | --- |
| wrapper bridge child (live) | old staged | live data | **200** |
| standalone manual | old staged | live snapshot | **500** |
| standalone manual | fresh 8190de1 | empty scratch | **500** |
| standalone manual | fresh 8190de1 | live snapshot | **500** |
| stdio (stdin held open) | fresh 8190de1 | live snapshot | **500** |

(Also measured: `POST /api/memories` from the browser page 500s on the
snapshot engine — the embed write path hits the same silent panic; static
reads of the served bundle are unaffected. `/api/health` 200 everywhere.)

Evidence actually delivered instead (real engine, real served bundle,
real statics — the lane's fetch interception is irrelevant to these):
- The engine at 49393 serves the FIXED bundle (MEASURED: the `/` shell
  references `styles-dfc26d953db5e467.css`; the lane's
  `tag_rule_declares_token_not_literal` row greps THAT bundle file and
  finds the live `:where(a) { color: var(--accent); … }` rule).
- Tag anchors compute `rgb(122, 162, 247)` == probe-converted `--accent`
  `#7aa2f7` on the served bundle (lane rows, MEASURED).
- Nav CONTROL unchanged (`rgb(232, 234, 237)` == converted fg2 `#e8eaed`).
- Statics re-derived on the tree + release dist: `:where(a)` anchored
  grep = 1; census 175/175; token coverage green.
- Real store data exists in the live shards (MEASURED: e.g. shard
  `project_c4cfb5149f636c54_shard_0.db` holds 165 memories, tag text on
  rows), so a future re-measure with a healthy engine will render the
  real Tags screen through the same rule.

## Mutation check (the discriminator proof)

Rule commented out in a scratch build (`trunk build` →
`styles-d0178368b2332774.css`), `LANE_EXPECT=post`:

```
rc=1  mismatches=3  stopped=false
```

| row | kind | outcome | expected | detail |
| --- | --- | --- | --- | --- |
| tag_link_color_not_browser_default | RED | FAIL | PASS | the tag anchor computes the browser default rgb(0, 0, 238) (underline: underline) |
| tag_link_resolves_accent_token | RED | FAIL | PASS | the tag anchor computes rgb(0, 0, 238), not the accent token's rgb(122, 162, 247) (--accent=#7aa2f7) |
| tag_rule_declares_token_not_literal | RED | FAIL | PASS | no :where(a) rule found in bundle …/web-ui/dist/styles-d0178368b2332774.css |
| tag_rows_render_and_href_preserved | CONTROL | PASS | PASS | links=2 hrefs intact (first="rust") |
| nav_anchor_color_unchanged | CONTROL | PASS | PASS | nav=rgb(232, 234, 237) == probe-converted --colorNeutralForeground2=#e8eaed |

Wave finding, found BY this check: the first mutant run scored
`mismatches=2` because the static row matched the rule text **inside the
comment block** — a rule in `/* */` is not a live rule. The row was
hardened (CSS comments stripped before extraction,
`capture-evidence.cjs` `tag_rule_declares_token_not_literal`); mutant
run 2: `mismatches=3`, all RED rows killed, CONTROLs PASS. Restored the
rule; post re-run `rc=0 mismatches=0` on the byte-identical bundle
(`styles-dfc26d953db5e467.css`). The mutation check left the shipped
harness stronger than the blueprint specified.

## Regression sweep

- `PILL_CONTRACT_ONLY=1 LANE_EXPECT=post` → 11 rows, `rc=0
  mismatches=0 stopped=false` (bundle `259e2ecdd2554e09`) — the fixture
  extension changed nothing for the pill lane (its spec carries no
  `tags` key).
- `DIALOG_ONLY=1 LANE_EXPECT=post` → 14 rows, `rc=0 mismatches=0
  stopped=false` — same proof for the dialog lane.
- wasm suite `cargo test --target wasm32-unknown-unknown`: two runs
  aborted by ENOSPC — the wasm-bindgen runner writes intermediates to
  `/tmp` (tmpfs, 7.3G/7.5G used system-wide, not this wave's data) and a
  concurrent release build had just filled the remainder. Re-run with
  `TMPDIR` pointed at the project scratch (78G free on /home): **rc=0,
  20 test binaries, 238 passed / 0 failed** (full log
  `.tmp/scratch/wasm-suite-final.log`, 2026-11-07 ~04:47Z; counts
  re-derived from that log — every `test result:` line carries
  `0 failed`, summing to 238). Independently re-run by the code
  reviewer with the same TMPDIR redirect: 238/0 again (review-1 §5).
  Matches the blueprint's 238.
- `cargo fmt --all --check` (root): clean.
- `trunk build --release`: success 04:30:58Z; release dist content ids
  identical (`web-ui-259e2ecdd2554e09` / `styles-dfc26d953db5e467`); the
  repo's trunk profile is already wasm-release, so the dev and release
  dists carry the same content hashes; census re-run on the release
  dist: 175/175 SELFTEST 9/9.

## Commands (finalized)

```sh
# build + statics
cd web-ui && trunk build
grep -cE '^\s*:where\(a\)' web-ui/public/styles.css          # 0 pre, 1 post
node scripts/check-class-coverage.mjs --min-emitted=175 --self-test
node scripts/check-token-coverage.mjs

# lanes (engine: .tmp/worktrees/tags-contrast/target/release/opencode-agents-memory \
#   --mode=standalone --storage-root .tmp/scratch/tags-contrast-lane \
#   --web-port 49393 --web-dist web-ui/dist ; Chrome CDP 9222)
TAGS_ONLY=1 LANE_EXPECT=pre  TEST_BASE_URL=http://127.0.0.1:49393 WEBUI_DIST=$PWD/dist \
  LANE_TABLE=../.tmp/scratch/tags-lane-pre.md  node scripts/capture-evidence.cjs
TAGS_ONLY=1 LANE_EXPECT=post TEST_BASE_URL=http://127.0.0.1:49393 WEBUI_DIST=$PWD/dist \
  LANE_TABLE=../.tmp/scratch/tags-lane-post.md node scripts/capture-evidence.cjs
PILL_CONTRACT_ONLY=1 LANE_EXPECT=post … DIALOG_ONLY=1 LANE_EXPECT=post …
# mutation: comment out the :where(a) rule, trunk build, post run → rc=1
#   mismatches=3; restore, rebuild, post run → rc=0 mismatches=0

# gates
cd web-ui && cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used
cd web-ui && cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation
cargo test --target wasm32-unknown-unknown
cargo fmt --all --check
```

## Verdict

The shipped defect is fixed by one zero-specificity rule: tag anchors
compute the accent token's colour instead of browser-default blue, nav
anchors are untouched, the click-through hrefs are intact, and every RED
row is proven load-bearing by the mutation check (3/3 killed, 2/2
CONTROLs stable). Census floor 175 holds everywhere. The only blueprint
deviation is the 3742 re-measure, replaced with the documented
real-engine evidence above — the standalone-500 environment defect is
recorded for syseng, not fixed here.
