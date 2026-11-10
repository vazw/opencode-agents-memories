# Issue 118 Wave 4 — measured minimal polish r2

## Approval and scope

READ: approved blueprint `20261110-1007-issue-118-wave4-polish-r2`, header SHA256 `33e999b2a84d`, SYS-APPROVED 2026-11-10T10:13Z, orchestrator dispatch pins sole coder. Baseline HEAD `92419e3f2f5836c280bb0b472a6cb908aa1ec972` was clean.

Implementation and acceptance are complete for the narrowed r2 scope on `dist-after-r3`; final opsec approval, code-reviewer approval, commit, push and live staging are NOT yet claimed (reserved for Steps 11/12).

## Competing designs

- **A:** targeted local state/presentation additions, including real Home resources and filtered-empty feedback. Pros: missing information/feedback becomes observable and diagnostics become accessible. Cons: Home adds requests and async state; broad tabs/editor replacements add unjustified risk. Prediction: named deficits disappear on the same fixture without loss of content. Refutation: baseline already satisfies the criterion, or changes hide content/regress interaction.
- **B:** retain architecture and apply only spacing/labels. Pros: smaller diff, no new state, all Profile sections stay visible. Cons: cannot supply absent Home summaries, Skills feedback or progress semantics. Prediction: sufficient regions remain sufficient without structural rewrites. Refutation: fixture demonstrates absent feedback/semantics or narrow page overflow unfixable with spacing/labels.

Shared discriminator: identical synthetic fixture, six routes, light/dark, 1280×800 and 390×844; DOM/accessibility/interaction plus readiness-gated captures. These are competing designs, not two implementations. Neither/no-change is valid.

| Region | READ baseline report of MEASURED observations | Decision and smallest scope |
| --- | --- | --- |
| Home | Landing copy only; no health/stats requests; shell anchors work | A: local summaries and independent health/stats; navigation regression only |
| Profile | Native picker has no associated label; sections/counts 2/1/0 visible without overflow | B: explicit label only; NEITHER for tabs/race-model additions |
| Tags | Unicode wraps; no page overflow; encoded destination; contrast 4.5709:1 light / 6.4879:1 dark; solid focus outline | NEITHER: zero product diff |
| Export/Import | Export button adjacent to import label; label focuses textarea; parser/Blob correct | B: scoped spacing only; no parser/editor replacement |
| Skills | Unmatched query has no explanation; label, rejected Save retention, Save/Cancel and Enter adequate | A: filtered-empty feedback/clear only; NEITHER for editor/grid; duplicate simultaneous errors not established |
| Stats | Exact text 0/2/7/1, peak7 widths; zero finite; no progress names/regions; page width676 at viewport390; 40 shards retained | A: exact native bars, bounded named keyboard scroll region and local daily overflow; content regression only |

## Baseline provenance and acceptance archive

MEASURED by coder: every entry in original `SHA256SUMS` verified against disk before source changes; every source hash in `provenance.json` matched clean working source. Archive bytes were copied without modification under `before/`: original fixture, lane script, matrix/transitions/extra/provenance JSON, checksum file, 24 matrix PNGs and `mcp-export.png`. `before/baseline-report.md` preserves the full report. Original checksum entries retain absolute scratch paths intentionally; they describe original locations, not rewritten archive paths. Final archive-local checksums will bind retained copies.

READ: `before/provenance.json` binds original HEAD, source hashes, fixture and asset hashes; served assets were fetched and byte-matched by baseline executor. Independent syseng re-verification is recorded in `.tmp/progress/20261110-1007-issue-118-wave4-polish-r2.ops-preflight.md`. Baseline bundle was not rebuilt.

Excluded: stale `mcp-skills.png`; early failed probes with detached fixture installation, wrapper-vs-input errors, wrong error selectors/envelopes. Their outcomes are harness failures, not product defects. Accepted matrix has three consecutive ready samples per capture. Raw baseline probes are reconnaissance, not the final reusable acceptance suite.

## Limitations and residuals

Baseline did not execute successful Save, list-error recovery, Profile races/missing transitions, Stats stale-snapshot reload failure, delayed export readiness/prompt exclusion, authenticated-header behavior or actual keyboard shard traversal. Incomplete Sessions/Agents timeline payloads were fixture omissions, not product defects. Final regression lanes must provide their own evidence; no inherited pass.

MEASURED residual in `before/extra.json`: skill-delete confirmation exposes prompt-specific erasure disclosure. This wave does not modify shared confirmation infrastructure. A separately bounded approval/fix is required.

Owner disk correction: 300 GB is closeout reporting/cleanup guidance, not a mid-wave stop threshold. No permission change, inaccessible whole-tree traversal, target deletion or cleanup is authorized by this implementation. Ops preflight measured ample available space; whole-tree usage is not remeasured or claimed here.

## Test evidence (complete — dist-after-r3)

- Final browser acceptance: `W4_TEST=all` run UTC `2026-11-10T14:19:57Z` on page `F84E3B0582B22A8842B5F70B90E6CDEA` against served `dist-after-r3` (`web-ui-ca2b5df9fdce9bc9`). **12/12 PASS** across all 12 named cases; final results pinned in `.tmp/progress/issue118-w4-green-r3/results.json` and durable copy at `after/results/`.

- Final bundle hashes (verified served==disk on `127.0.0.1:49418` before browser runs, `dist-after-r3`):
  - `index.html`: `8fafebd2e8b49e313cc832b86a5f189e76fd1ef3dce54d7661f19f3788331b74`
  - `styles-590dfeed23cfe8d7.css`: `523cce1f00075bac6639a0fdeecd85a3e4992714449871493c0304ff7e292d99`
  - `web-ui-ca2b5df9fdce9bc9.js`: `bd0fe74d908dc76a1cae9ecd8f0de1b24931d158727da39688cb8279b14dc158`
  - `web-ui-ca2b5df9fdce9bc9_bg.wasm`: `5d340c0fd788904aba95e23b5ca557fd1a136b42a09677ef4e85d1339938987c`

- Final script SHA256 (`web-ui/scripts/test-wave4-cdp.cjs`): `923d0558f5a30f7d269f0db47bb9a2479cdc6932dde65b660c1085ba3c6d9515`
- Final fixture SHA256 (`web-ui/scripts/fixtures/issue-118-wave4.json`): `517b3dca8a8f8c177cc36f4c8a0d0143e65b7deeb3f81a36d7e894bd6c3af89f`

- Home pure test: `cargo test --manifest-path /home/vaz/Code/Personal/Rust/opencode-agents-memory/web-ui/Cargo.toml --target wasm32-unknown-unknown --test screens test_home_summary_uses_payload_values_including_zero` from `web-ui`: RED E0425 missing helper, then GREEN 1 passed. Input total7/user0/project3, shard active flags true,false; expected ordered 7,0,3,1. Managed log `sh_4ecchtf6` runs1–2.

- Skills pure test: analogous `--test skill_library test_skills_empty_copy_distinguishes_library_and_filter`: RED E0432 missing helper, then GREEN 1 passed. Inputs (0,0), (2,0), (2,1) distinguish library-empty, filtered-empty and visible rows. Managed log `sh_fylty2tc` runs1–2.

- **Full pure wasm suite record:** `241 passed; 0 failed; 0 ignored` across **20 `test result:` lines** and **21 `Running` sections** — log at `.tmp/scratch/issue-118-w4/wasm-suite-final.log` (shell `sh_p4e6qzst`).

- **Final checksums:** baseline `before/SHA256SUMS` **44/44 OK**; single-base regenerated manifest at `after/SHA256SUMS` — **all retained artifacts OK (exit 0)**.

- **TDD-EXEMPT:** Export visual-only spacing has geometry/screenshot acceptance, not invented parser RED. Evidence/docs/ops add no product behavior.

## Closeout disclosures (required)

- Dist-after-r2 (`web-ui-e573af5699f970a8`) browser evidence is superseded by `dist-after-r3` (`web-ui-ca2b5df9fdce9bc9`); historical `r2` runs remain in `.tmp/progress/issue118-w4-green-v2/` and `.tmp/progress/issue118-w4-green-v3/` as historical records.
- A2 intermediate script hash loss: intermediate revisions (`b24b959d`, `6077dde9`, `ada991ca`, `a03f4563`) exist only as hashes; pre-`A1/A4/A5` snapshot `b1437980` archived at `.tmp/scratch/issue-118-w4/scripts/test-wave4-cdp.b1437980.cjs`.
- A1 resolved: populated `*/milestones`/`*/sessions` fixtures exercised Ready arms; `native_select_and_route_regressions` now asserts exact single-project select value/label.
- Out-of-scope residual: skill-delete prompt-specific disclosure measured in `before/extra.json`; not fixed here; requires separate bounded approval.

- Skills pure test: analogous `--test skill_library test_skills_empty_copy_distinguishes_library_and_filter`: RED E0432 missing helper, then GREEN 1 passed. Inputs (0,0), (2,0), (2,1) distinguish library-empty, filtered-empty and visible rows. Managed log `sh_fylty2tc` runs1–2.

- Browser RED checkpoint1: `.tmp/progress/20261110-issue118-w4-browser-red-checkpoint-1.md`; four named cases sent to independent frontend executor.

- **Final browser acceptance (dist-after-r3):** `.tmp/progress/issue118-w4-green-r3/results.json` — `W4_TEST=all` run UTC `2026-11-10T14:19:57Z` on page `F84E3B0582B22A8842B5F70B90E6CDEA` against served `dist-after-r3` (`web-ui-ca2b5df9fdce9bc9`). **12/12 PASS** across all 12 named cases (Home independent resources, Profile label, Skills filtered-empty, Stats accessible counters, Skills form save/cancel/enter, Portability, Stats existing content, Tags unchanged matrix, Native select regressions, Matrix ready captures).

- **Full pure wasm suite:** `241 passed; 0 failed; 0 ignored` over `20 test result: lines / 21 Running` — record in `.tmp/scratch/issue-118-w4/wasm-suite-final.log` (shell `sh_p4e6qzst`).

- **Final artifact checksums (single-base `after/SHA256SUMS`):** `44/44 OK` for baseline (`before/SHA256SUMS`) and the regenerated single-base manifest at `docs/evidence/issue-118-wave4/after/SHA256SUMS` for dist-after-r3 evidence — all retained artifacts verified exit 0.

- **TDD-EXEMPT:** Export visual-only spacing has geometry/screenshot acceptance, not invented parser RED. Evidence/docs/ops add no product behavior.
