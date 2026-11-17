# Issue 118 wave 5 — export truncation, memory-card reading flow, reachable tags, `/agents` label

Blueprint: `.tmp/blueprints/blueprint-20261117-0825-issue-118-wave5-export-and-a11y-r6.md`
(body `856e70b5f06f`, SYS-APPROVED 2026-11-17T08:45Z, Plan Gate round 6).

Four owner-reported defects, `web-ui/` only. **No `crates/**` change, no dependency change, no request
read by the product, and the served bundle at `plugin/engine/linux-x64/web-ui/dist` was never replaced**
(§8). Every figure below is a pasted command output unless labelled READ or INFERRED.

| defect | what it was | what it is |
| --- | --- | --- |
| **F1** | the export walk stopped at page 25 (5 000 of the owner's 6 227 memories) and the preview printed the truncated count as truth | the walk follows the server's `total_pages` up to the named ceiling `EXPORT_MAX_PAGES = 250`; truncation is **derived from counts**; the preview says `memories: N of M — export limited` |
| **G1** | the card's `<details>`/`<summary>` sat **between** the two halves of one body, so an expanded card read mid-sentence → "Show more" → rest of the sentence | one content div renders `memory_body(…)` (preview alone, or the whole body) and the toggle is its **next element sibling**; the clamp triple is gone |
| **G2** | `+N more` was an inert `<span>`; the hidden tags were unreachable by mouse | a Thaw `Button` (class `memory-tag-more`, `data-testid="memory-tag-more"`, `aria-expanded="false"`, today's `aria-label` intact) reveals the remainder through the **same** `tag_chip` constructor, one-way |
| **F2** | `/agents` project picker had an id but no accessible name (empty name in the a11y tree) | `<label for="agents-project-select">Project</label>`, `for`/`id` bound to one const; native `<select>` kept |

## 1. RED → GREEN, per defect

### F1a — behavioural RED (the cap), then GREEN

```
$ cd web-ui && cargo test --offline --target wasm32-unknown-unknown --test export_import test_export_walk_reaches_the_store_total
failures:

    test_export_walk_reaches_the_store_total

test result: FAILED. 0 passed; 1 failed; 0 ignored; 16 filtered out; finished in 0.02s
```
The distinguishing input is `should_continue(25, 200, 200, 32)`: a **full** page 25 with seven pages to
come. It returned `false` (the `page >= 25` clause) and the test wants `true`. After the fix:

```
$ cd web-ui && cargo test --offline --target wasm32-unknown-unknown --test export_import
test result: ok. 19 passed; 0 failed; 0 ignored; 0 filtered out; finished in 0.03s
```

### F1b — compile RED (`E0061`/`E0432`), then GREEN

```
error[E0061]: this function takes 2 arguments but 3 arguments were supplied
...
Some errors have detailed explanations: E0061, E0432.
error: could not compile `web-ui` (test "export_import") due to 12 previous errors
```
After the derivation (`export_truncated`) and the third preview argument:

```
$ cd web-ui && cargo test --offline --target wasm32-unknown-unknown --test export_import
test result: ok. 25 passed; 0 failed; 0 ignored; 0 filtered out; finished in 0.03s
$ grep -c '#\[wasm_bindgen_test\]' web-ui/tests/export_import.rs
25
```
The six new tests also ran **by name**, each `test result: ok. 1 passed; 0 failed; … 24 filtered out`:
`test_export_preview_discloses_a_short_walk`, `test_export_preview_states_no_limit_when_complete`,
`test_export_preview_survives_a_store_that_grew_mid_walk`, `test_export_preview_reports_an_unknown_total`,
`test_export_truncation_is_derived_from_the_counts`, `test_export_document_row_count_equals_the_preview_count`.

### G1 — compile RED, then GREEN

```
error[E0432]: unresolved imports `web_ui::core_screens::memory_body`, `web_ui::core_screens::memory_toggle_label`
```
```
$ … --test core_screens test_memory_body_is_contiguous_when_expanded
test result: ok. 1 passed; 0 failed; 0 ignored; 61 filtered out
$ … --test core_screens test_memory_toggle_label_swaps
test result: ok. 1 passed; 0 failed; 0 ignored; 61 filtered out
```
The 416-character fixture cuts at index 318 (`preview` 318 / `rest` 98), so the strict-shorter assertion has
98 characters of margin. `as_deref()` is used, never `Some(rest.expect(..).as_str())` (E0716).

### G2 — compile RED, then GREEN

```
error[E0432]: unresolved imports `web_ui::core_screens::hidden_tags`, `web_ui::core_screens::memory_tag_more_label`
```
```
$ … --test core_screens test_hidden_tags_complete_the_visible_prefix
test result: ok. 1 passed; 0 failed; 0 ignored; 63 filtered out
$ … --test core_screens test_memory_tag_more_label_absent_at_zero
test result: ok. 1 passed; 0 failed; 0 ignored; 63 filtered out
```
Five tags → visible 4 + hidden 1 with `visible ++ hidden == tags`; `memory_tag_more_label(0) == None` (no
`+0 more` control on a four-tag card).

### F2 — compile RED, then GREEN (const drift guard, not the F2 proof)

```
error[E0432]: unresolved imports … AGENTS_PROJECT_SELECT_ID, AGENTS_PROJECT_SELECT_LABEL,
          AGENTS_PROJECT_SELECT_LABEL_CLASS … due to 1 previous error
```
```
$ … --test agents test_project_select_id_is_the_labelled_testid
test result: ok. 1 passed; 0 failed; 0 ignored; 30 filtered out
```
The behavioural proof is the CDP case `agents_select_has_accessible_name` (§6, phase 1 fails today with
`labels.length === 0`).

### Browser RED — phase 1 on `dist-before` (`W5_TEST=all`), 0/5 PASS, exit 1

```
FAIL export_walks_to_store_total: the walk issued 25 requests and the preview reads "memories: 5000"; expected 32 requests and "memories: 6227"
FAIL export_discloses_a_short_walk: page 3 failed after 3 requests: preview reads "memories: 400" and blob truncated=false; expected "memories: 400 of 6227 — export limited" and truncated=true
FAIL memory_card_toggle_after_content: the expanded card carries 0 toggle(s) and the content's next element sibling is a DETAILS; expected one toggle after the content
FAIL memory_card_tags_reachable: the +1 more control is a SPAN, not a BUTTON (hidden tags are unreachable)
FAIL agents_select_has_accessible_name: the picker's labels.length === 0 (label null); expected one label reading "Project"
```
Readiness rule honoured: every case waits only on selectors that exist in both bundles
(`memory-content`, `memory-tags`, `export-download:not([disabled])`, `agents-project-select`), so the pre run
fails on an assertion that **names the defect**, never on a timeout.

## 2. Mutants (each applied, the owning test run, then reverted with `cp` + `cmp`)

| # | mutant (wrong code) | where | command | observed |
| --- | --- | --- | --- | --- |
| M1 | restore `if page >= 25 { return false; }` | `export_import.rs` `should_continue` | `cargo test … --test export_import test_export_walk_reaches_the_store_total` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 24 filtered out` — the walk stops at 5 000 of 6 227 |
| M2 | the clamp **present** (`display:-webkit-box; -webkit-line-clamp:12; -webkit-box-orient:vertical; overflow:hidden`) | `styles.css` `.memory-list .memory-content` | `trunk build --dist …/dist-mutant`; `W5_TEST=memory_card_toggle_after_content` | `FAIL …: webkitLineClamp is "12", expected "none" (clamp still applied)` (exit 1). The product state is the clamp **dropped**: phase 2 measures `webkitLineClamp: "none"`, `display: block`. |
| M3 | expanded = `rest` alone (the old split) | `core_screens.rs` `memory_body` | `cargo test … --test core_screens test_memory_body_is_contiguous_when_expanded` | `FAILED. 0 passed; 1 failed` — `assertion left == right failed: expanded is the whole body, in one piece` |
| M4 | collapsed = the whole body | `core_screens.rs` `memory_body` | same | `FAILED. 0 passed; 1 failed` — `assertion left == right failed: collapsed is the preview` |
| M5 | the preview formats `count + 1` | `export_import.rs` `export_preview_text` | `cargo test … --test export_import test_export_preview_states_no_limit_when_complete` | `FAILED … left: "memories: 6228" right: "memories: 6227"` |
| — | drop the slot / drop the `> button` override / reintroduce a scroll term | **wave 6's mutants, not this wave's code** | `grep -n 'memories-search-clear-slot\|> button' web-ui/public/styles.css` → no output; `grep -rn 'scroll_y\|scrollY' web-ui/src/` → no output | wave 5 has no reserved input slot, no suffix override and no scroll term to mutate; the three belong to wave 6's G3/G5 table and are named here so a later reader does not look for them in this wave |

Every revert was proven with `cp` + `cmp`: `MUTANT1 restored cmp OK`, `MUTANT3 restored cmp OK`,
`MUTANT4 restored cmp OK`, `MUTANT5 restored cmp OK`, and the CSS restore `RESTORE cmp OK` (source
`sha256 3682d4a920978267b555f3706e1ab1410c2f54af4de56359416b90cc6b352a10`).

The view-call-site mutant (**preview fed `all.len()` instead of the exportable count**) is invisible to a
pure wasm test by construction: it is owned by the CDP equality
`preview === "memories: ${blob.memories.length}"` in phase 2, where the synthetic store leaks one prompt
row (fetched 6 228 ≠ exportable 6 227).

## 3. Greps the wave gates on

| grep | HEAD (before) | now (after) |
| --- | --- | --- |
| `grep -c '<details>' web-ui/src/core_screens.rs` | `1` | **`0`** |
| `grep -c '<details class="stats-buckets">' …` (positive control) | `1` | **`1`** — the Stats disclosure is wave 6's, not this wave's |
| `grep -c '<summary' …` | `2` | **`1`** |
| `grep -n 'webkit-line-clamp\|-webkit-box' web-ui/public/styles.css` | `838`, `839`, `840` | **no output** |
| `grep -n 'page >= 25' web-ui/src/export_import.rs` | `221`, `386` (and `page > 25` at `392`) | **no output** |
| `grep -n 'let (shown, hidden)' web-ui/src/core_screens.rs` | (the shadowing form) | **no output** |
| `grep -c "laneRun('pill_[a-z_]*', 'RED'" web-ui/scripts/capture-evidence.cjs` | **`4`** (`:610`, `:632`, `:639`, `:656`) | **`1`** (`pill_overflow_survives`) |

The clamp grep runs **twice**: in Step 2 after the deletion, and again in §6 phase 3 **after** the mutant
build restored `styles.css` (`post-restore-exit=1`, i.e. no output). `git status` cannot see a leftover
clamp while `styles.css` is legitimately modified — that second run is the only proof.

## 4. Class census (the tool is the authority)

Method: `git archive HEAD web-ui/src web-ui/public/styles.css web-ui/scripts/check-class-coverage.mjs` into
`.tmp/scratch/w5-census/web-ui/` (same relative layout), then the real extractor; each post-step line is the
real extractor on the working tree.

| tree state | verbatim output |
| --- | --- |
| HEAD (staged copy) | `sites=228 literal=204 const=19 brace=5 residue=0` / `emitted=187 hooks=187 missing=0 dead=0 unresolved=0` |
| after Step 2 (G1: `.memory-toggle`) | `sites=229 literal=204 const=20 brace=5 residue=0` / `emitted=188 hooks=188 missing=0 dead=0 unresolved=0` |
| after Step 3 (G2: `tag_chip`, `memory-tag-more` on a Button) | `sites=229 literal=203 const=21 brace=5 residue=0` / `emitted=188 hooks=188 missing=0 dead=0 unresolved=0` |
| after Step 6 (F2: `.agents-project-select-label`) | `sites=230 literal=203 const=22 brace=5 residue=0` / `emitted=189 hooks=189 missing=0 dead=0 unresolved=0` |

**Ladder: 187 → 188 → 188 → 189.** The pre-commit floor stays `--min-emitted=175`
(`.githooks/pre-commit:69`); it is not raised here.

## 5. Gates (Step 7)

| gate | exit | note |
| --- | --- | --- |
| `cargo fmt --all` then `cargo fmt --all --check` (root + `web-ui`) | 0 | format pass **first**, check after — several fences are not in rustfmt shape |
| `web-ui`: `cargo clippy --offline --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used` | 0 | |
| `web-ui`: `cargo clippy --offline --workspace --all-targets -- -D warnings` | 0 | the r5 `clippy::obfuscated_if_else` trap is avoided by the `if/else` label |
| root: `cargo clippy --offline --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used` | 0 | |
| root: `cargo clippy --offline --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation` | 0 | |
| `cd web-ui && cargo test --offline --target wasm32-unknown-unknown --no-fail-fast` | 0 | **255 passed, 0 failed, 20 `test result:` lines** (core_screens **64**, export_import **25**, agents **31**; other binaries unchanged) |

The wave-4 baseline was 241 (READ from `docs/evidence/issue-118-wave4/README.md`); this wave adds
4 + 9 + 1 = 14 tests. **`cargo test --workspace` was never run** (concurrent environment).

## 6. Browser matrix (Step 9) — `web-ui/scripts/test-wave5-cdp.cjs`

Script `sha256 cac29a5754d9085e2007d458c5081bc24624929695453899984d075bfb503379`. Synthetic REST fixture
installed before the wasm boots; **no engine and no store involved** (`serve-dist.mjs` answers no `/api/*`,
so the script answers it itself). Synthetic `/api/memories`: **6 227 memory rows + one leaked prompt row**,
`total: 6227`, `totalPages: 32`. The leak is a deliberate synthetic exception (see §9(c)): a live server
filters prompts before `timeline.len()`.

**Phase 1 — `dist-before`, `W5_TEST=all` → 0/5 PASS, exit 1** (the five named failures in §1).

**Phase 2 — `dist-after`, `W5_TEST=all` → 5/5 PASS, exit 0**

```
PASS export_walks_to_store_total
PASS export_discloses_a_short_walk
PASS memory_card_toggle_after_content
PASS memory_card_tags_reachable
PASS agents_select_has_accessible_name
```
Computed details (`results.json`):

| case | measured |
| --- | --- |
| `export_walks_to_store_total` | `requests: 32`, `preview: "memories: 6227"`, `rows: 6227`, `truncated: false` — **preview == blob row count asserted** |
| `export_discloses_a_short_walk` | `requests: 3`, preview `"memories: 400 of 6227 — export limited"`, `rows: 400`, `truncated: true` |
| `memory_card_toggle_after_content` | `previewChars: 316`, `expandedChars: 1550`, `webkitLineClamp: "none"`, `display: "block"`, `ariaExpanded: ["false","true","false"]` |
| `memory_card_tags_reachable` | pills `4 → 5`, last chip `"Filter by tag: fifth"`, click issued a `tags=fifth` request |
| `agents_select_has_accessible_name` | `tag: SELECT`, `labels: 1`, `label: "Project"`, AX `role: combobox`, AX `name: "Project"` |

**Phase 3 — the clamp mutant** (the one assertion phase 1 cannot reach, because pre-change the toggle does
not exist): the clamp triple re-added to `.memory-list .memory-content`, `trunk build --dist …/dist-mutant`
(`styles-fac5a7fabfd25252.css`, sha256 `703741763c5fdd47489fd5cf54d93a28d73975611983b999428c68cdbb472120`),
served, one case:

```
FAIL memory_card_toggle_after_content: webkitLineClamp is "12", expected "none" (clamp still applied)
EXIT=1
```
Then `styles.css` restored (`RESTORE cmp OK`), `dist-mutant` discarded (deleted, never served again, never
staged), and the clamp grep re-run **on the restored source** → no output.

## 7. Regressions (Step 10)

**Wave 4 lane, `dist-after`, `W4_TEST=all` → 12/12 PASS, exit 0**, including
`portability_existing_contract` (it reads the export document and stays green).

**Pill lane, both orientations** (`PILL_CONTRACT_ONLY=1`), one row FAILs pre and passes post:

| row | kind | `LANE_EXPECT=pre` on `dist-before` | `LANE_EXPECT=post` on `dist-after` |
| --- | --- | --- | --- |
| `pill_is_thaw_button` | CONTROL | PASS (`card=5 filter=1 all thaw-button`) | PASS (`card=6 filter=1 all thaw-button`) |
| `pill_no_chip_class` | CONTROL | PASS | PASS |
| `pill_no_chip_selector` | CONTROL | PASS | PASS |
| `pill_has_stable_testid` | CONTROL | PASS (`memory-tag-pill=5 memory-filter-pill=1`) | PASS (`memory-tag-pill=5 memory-filter-pill=1`) |
| `pill_accessible_name_preserved` | CONTROL | PASS | PASS |
| `pill_click_filters_as_before` | CONTROL | PASS | PASS |
| `pill_duplicate_filter_idempotent` | CONTROL | PASS | PASS |
| `pill_remove_filter_as_before` | CONTROL | PASS | PASS |
| `pill_overflow_survives` | **RED** | **FAIL** — `the overflow control is a SPAN, not a BUTTON` | PASS — `pills=4->6 more=BUTTON then removed, last="Filter by tag: zeta"` |
| `pill_keyboard_activation` | CONTROL | PASS | PASS |
| `pill_type_button_no_submit` | CONTROL | PASS | PASS |

```
LANE PILL_CONTRACT bundle=1f33b675e5625a35 expect=pre  rows=11 mismatches=0 stopped=false   (exit 0)
LANE PILL_CONTRACT bundle=4740b35ed77c1f0f expect=post rows=11 mismatches=0 stopped=false   (exit 0)
```
`card=5` vs `card=6` is the new more-control: it is a `button` inside `[data-testid="memory-tags"]`, counted
by `pill_is_thaw_button` and excluded by `pill_has_stable_testid` (which counts **filter** pills). The "4"
in the pre-activation contract is counted on `memory-tag-pill`, never on `button`.

## 8. Bundles and the served-before manifest

Built to scratch, never in place (`.tmp/scratch/issue-118-w5/`):

| bundle | index.html | styles | wasm | js |
| --- | --- | --- | --- | --- |
| `dist-before` (HEAD) | `af261124…` | `523cce1f…` (`styles-590dfeed…`) | `2bba72ce…` (`web-ui-1f33b675…_bg.wasm`) | `2f64ae0b…` |
| `dist-after` (this wave) | `c008d086…` | `3682d4a9…` (`styles-fba1e3de…`) | `03ef5e46…` (`web-ui-4740b35e…_bg.wasm`) | `2f64ae0b…` (unchanged) |
| `dist-mutant` (phase 3, discarded) | `31053ca8…` | `70374176…` (`styles-fac5a7fa…`) | `03ef5e46…` | `2f64ae0b…` |

`served-before.sha256` (every file of `plugin/engine/linux-x64/web-ui/dist`, sha256 of the manifest
`463eb1e61a9c7c062b1e3d1588da0ce42da8eb635c5a675afcac1996321587d7`) verified **5/5 OK** immediately before
the build and again after the browser work:

```
./.trunk-lock: OK
./index.html: OK
./styles-590dfeed23cfe8d7.css: OK
./web-ui-1f33b675e5625a35_bg.wasm: OK
./web-ui-c4cff17b4bcc4e1d.js: OK
```

## 9. Disclosures

**(a) No docHeight is measured, and why.** This wave claims **no** docHeight figure, and such a pair could
not discriminate a clamp change here: `--app-measure` is never defined (`grep -n 'app-measure'
web-ui/public/styles.css` → `681`, `799`, `838`, every one a read with a `72ch` fallback); a ≤320-character
preview is a few lines against a 12-line clamp; `min-height: 8em` is retained and pins the box; and the pill
lane's fixture bodies are 37 and 33 characters (READ from `PILL_SPEC`). The only instrument that could
produce such a figure is the census loop (`web-ui/scripts/capture-evidence.cjs:94` is the `docHeight`
expression, `:1296` the per-route loop) against a **disposable engine**, because `serve-dist.mjs` answers no
`/api/*`; it was deliberately not run because it cannot discriminate. The 3322 px figure
(`docs/evidence/webui-wave3-worst-screens/README.md:96`) is a **different fixture's** context only (15
memories at page size 8).

**(b) The `<summary` count is a sequence, not a constant:** **2** today (`core_screens.rs:493` and `:1932` at
HEAD) → **1** after wave 5 (measured above; the survivor is the Stats-bucket `<summary>`) → **0** after
wave 6, which deletes it by design (`.tmp/blueprints/blueprint-20261117-0758-issue-118-wave6-search-and-stats-r4.md`,
body `4ea01372b838`, **`:27`** and **`:330`**). Wave 5's gate asserting `1` is therefore **historical after
wave 6** and must never be "repaired" back to `1` by restoring a `<summary>`. *Citation note: this wave's
blueprint cites that file as `:388`; r4 has 330 lines and `:388` is wave-6 **r3**'s last line. The content —
wave 5's `<summary` gate becomes historical — is at r4 `:27` and `:330`.*

**(c) Two synthetic payloads deliberately show a count a live server cannot produce.** The retained wave-4
fixture (`web-ui/scripts/fixtures/issue-118-wave4.json`, sha256 `517b3dca…`, **not corrected**) holds 2 items
— one memory + one prompt — with `total: 2`, so the preview on that lane now reads
`memories: 1 of 2 — export limited`: truthful for that payload, synthetic as a store total. A live server
filters prompts before `timeline.len()` (`crates/mem-web/src/api/memories.rs:289,298-304,383`), so it never
reports a `total` counting prompts. The same exception, other direction, is why the wave-5 export case leaks
one prompt row: fetched 6 228 ≠ exportable 6 227 is the only way a browser can observe the view's call site.

**(d) Five lane kinds changed, and why.** `pill_overflow_survives` `CONTROL → RED` (its agreement inverts:
the span becomes a Button). `pill_is_thaw_button`, `pill_no_chip_class`, `pill_no_chip_selector` and
`pill_has_stable_testid` `RED → CONTROL`: the plan gate measured all four **PASS on pristine HEAD**
(`LANE_EXPECT=pre` → `rows=11 mismatches=4`, exit 1), i.e. their own waves had landed; a RED row that passes
pre-change is a non-discriminating assertion by the lane's own rule (`capture-evidence.cjs:254-258`). After
the change the pre lane is `rows=11 mismatches=0` with exactly one row FAILing
(`pill_overflow_survives`). `pill_has_stable_testid`'s count basis moved to
`[data-testid="memory-tags"] button:not([data-testid="memory-tag-more"])` — the more-control is a control,
not a filter pill. `cardPills` is unchanged, so `pill_type_button_no_submit` still covers the new button's
`type="button"`.

**(e) The superseded-walk write guard and the live region.** The walk now publishes `memories` and `total`
**inside** the existing generation guard, so a superseded reload cannot overwrite a newer walk's rows while
the newer one's count stands. `export-preview` gained `role="status"`; its text changes at most twice.

**(f) The ceiling is INFERRED.** `EXPORT_MAX_PAGES = 250` (50 000 rows at page size 200) is ≈8× the derived
32-page store (6 227 / 200 = 31.1 → 32 pages) and exists only so a very large store cannot wedge the tab.
It is not a correctness rule: when it binds, the preview and the file both say so. A future wave raises it
with a measurement, never by deleting the disclosure.

**(g) The count basis was already tied to the document before this wave.** At HEAD the preview read
`exportable_memories(&memories.get()).len()` (`export_import.rs:501`) and `export_document` read the same
function (`:116`) — that tie is **not** the fix. What wave 5 changes is (i) how many rows reach that
function —
the walk runs to the server's `total_pages` instead of stopping at 5 000, (ii) the disclosure text beside
the number when the file falls short, and (iii) the document's `truncated` flag, now **derived from the two
counts** instead of set by a branch. On the owner's store the file goes from **5 000 rows to the walk-time
`total`** — 6 227 at the baseline measurement, i.e. **+1 227 rows, +24.5 %** (derived: `6227-5000`,
`1227/5000`; the store grew 6 226 → 6 229 during the baseline walk, so the row count is not a constant).

**(h) `plugin/engine/**` is untracked** (`git ls-files plugin/engine | wc -l` → `0`; ignored by
`plugin/.gitignore:2`). No `git status`, `git diff` or `git checkout` can prove the served bundle survived;
§8's `served-before.sha256` manifest is the only evidence. The live engine (pid 20227,
`--storage-root /home/opencode/.opencode-mem/data`) was read-only for this wave — in fact not addressed at
all: every browser case was served by `serve-dist.mjs` with an in-page fetch interceptor.

## 10. Rollback record (Step 12)

- This wave **wrote** `.tmp/scratch/issue-118-w5/dist-before` and `dist-after`, and **wrote and discarded**
  `dist-mutant` there (phase 3's probe: never served again, never staged, deleted).
  `.tmp/scratch/issue-118-w5/{phase1-before,phase2-after,phase3-mutant,wave4-after,pill-pre.md,pill-post.md,*.sha256,*.baseline}`
  hold the run artefacts.
- The served bundle `plugin/engine/linux-x64/web-ui/dist` was **never replaced** — proved by the (h)
  manifest, since that path is untracked. The live engine's store was never mutated.
- **Rollback** is "point the lane at the previous dist and stop the disposable server": no `rm -rf` of any
  tracked path, no in-place replacement, no migration. The source rollback is a revert of the three
  `web-ui/src` files plus `styles.css` and `capture-evidence.cjs`; `dist-before` is the HEAD build, so the
  previous behaviour is reproducible from it.
- Tracked changes are exactly the files in the blueprint's [FILE STRUCTURE] (`web-ui/src/{export_import,
  core_screens,agents}.rs`, `web-ui/public/styles.css`, the three `web-ui/tests/*`, `capture-evidence.cjs`,
  the new `web-ui/scripts/test-wave5-cdp.cjs`, this README). Nothing was committed.

**Measurement note (deviation, disclosed):** both scratch dists carried trunk's 0-byte `.trunk-lock` build
residue, which `serve-dist.mjs` answers with the SPA shell for an extensionless path — so wave-4's
`matrix_ready_captures` compared that lock file against `index.html` and failed its served-vs-disk check.
The lock was deleted from the two **scratch** dists (no bundle asset changed; all four asset hashes are the
ones in §8) and the lane re-ran 12/12. No assertion was weakened: every bundle asset is still byte-checked.
