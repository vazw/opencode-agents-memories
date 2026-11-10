# Issue 118 Wave 4 — unchanged-source baseline measurement

## Scope and ruling

Measurement only. No tracked source/config edits, dependencies, live runtime staging, real storage reads/writes, cleanup or permission changes. First blocked report preserved. Owner correction: 300 GB is a closeout cleanup target, not a mid-wave stop threshold. Complete earlier accounting is READ from `.tmp/drafts/issue-118-wave4-disk-accounting-2.md`: 138,029,211,648 bytes, exit 0. No inaccessible whole-tree traversal repeated.

These are per-region recommendations for revised design, NOT implementation approval:

| Region | Discriminator | MEASURED result / smallest surviving scope |
|---|---|---|
| Home | **A** for real summaries; existing navigation regression-only | All four theme/viewport captures contain only landing copy; no stats/health requests and no summaries. Existing shell already provides actual anchors to memories, profile, skills and stats. Do not claim missing navigation or independent-failure bug: independent resources do not currently exist. |
| Profile | **B** for explicit picker label; **NEITHER** for mandatory tabs | Populated native SELECT has 3 options, id `profile-identity-select`, zero associated labels. All three sections and exact counts 2/1/0 are visible without page overflow. No measured evidence that tabs improve this fixture; preserve concurrent content unless a new discriminator justifies hiding it. |
| Tags | **NEITHER**, no-change/regression-only | Scope headings Project/User/Memory tags already distinguish content. Long Unicode/space-bearing tag wraps in narrow screenshots; no document horizontal overflow. Encoded destination retained. Link contrast 4.5709:1 light, 6.4879:1 dark. Focused link has theme-colored solid outline. No evidence requiring replacement of Card/Table/Badge or extra Divider. |
| Export/Import | **B**, spacing/region clarity only | Desktop capture has `Download export` immediately adjacent to `Import JSON` label. Existing textarea is correctly labelled and label click focuses it. Paste malformed → valid explorer document → unknown document gives Malformed → numeric preview 0 → Unrecognized, clearing/reappearing error. No import writes. Actual export Blob is valid explorer export containing fixture profile/tags. Field/Textarea replacement is not required to fix a broken label/parser. |
| Skill Library | **A** narrowly for filtered-empty feedback; editor/grid replacement **NEITHER** | Actual nested input receives unmatched query: zero rows and no empty explanation/clear action. Save and Cancel already type=button, zero submit events; Save 500 retains all draft fields, Cancel closes without further write/navigation. Enter on actual input causes no write/navigation and keeps form open (zero submit events observed in this browser; this is not a requirement that Enter always emits zero). Content label already associated with actual textarea. Distinct list/action testid naming may help automation but duplicate simultaneous errors were NOT established. |
| Stats | **A** for accessible counters/local overflow; content regression-only | Exact visible counters 0/2/7/1 and widths 0/28.6/100/14.3% agree with max 7. All-zero reload gives four finite 0% widths. Zero progress semantics/names and zero named regions. 390px capture document scrollWidth=676, horizontal page overflow in both themes. Forty shard rows retained (41 rows including header), status/error/mismatch text present; last shard exists and scrollIntoView reaches viewport. No evidence of row loss. Bounded named keyboard-scroll region remains justified, but actual PageDown/End reachability was NOT exercised. |

A retains more state/fetch/accessibility risk; B is smaller and retains content but cannot supply missing Home summaries or filtered-empty feedback. Neither means retain adequate existing region. A/B are competing designs, not two implementations. No fabricated RED tests.

## Source/build/fixture provenance

All paths below relative to `/home/vaz/Code/Personal/Rust/opencode-agents-memory` unless absolute paths are in the manifest.

- MEASURED before build and at final validation: HEAD `92419e3f2f5836c280bb0b472a6cb908aa1ec972`, clean tracked tree.
- Build: `cd web-ui && trunk build --release --dist ../.tmp/scratch/issue-118-w4-baseline/dist` exit 0, managed shell `sh_mt6zahi5`. Build used existing warm target, no target copy or toolchain edits. Future-incompat warning: proc-macro-error2 2.0.1.
- Served with `node web-ui/scripts/serve-dist.mjs --port 49418 --root .tmp/scratch/issue-118-w4-baseline/dist`, managed shell `sh_mleulteo`, URL `http://127.0.0.1:49418`.
- Browser: existing CDP endpoint 9222, MCP-created isolated context `issue118-w4-baseline`, page 2. Operator page 1 untouched. Node scripts attach only the dedicated fixture-origin page. MCP screenshot and accessibility snapshot also taken.
- Fixture: `.tmp/scratch/issue-118-w4-baseline/fixture.json`; generated 40-shard data and scenario response logic in `lane.cjs`. Synthetic only. Interception before boot, native Response objects, unknown API paths return 599. No real engine/storage involved.
- Exact source hashes, fixture SHA256, scripts/artifact SHA256 and served index/css/js/wasm hashes: `.tmp/scratch/issue-118-w4-baseline/provenance.json` and `SHA256SUMS`. Every served asset fetched back and byte hash matched disk. `extra.json` records browser-loaded asset URLs and decoded sizes.
- Captures: 24 matrix PNGs: six routes × light/dark × 1280×800/390×844, named `<route>-<theme>-<width>.png` in the same scratch directory. `matrix.json` contains DOM text, requests, controls, links, overflow and three consecutive ready samples per capture. No nested Thaw-node component counting.
- Build-side measured paths after: `web-ui/target` 10,050,854,912 bytes; scratch baseline directory 2,682,880 bytes at validation (later text provenance adds bytes). Not a whole-repository growth measurement; no precise reclaimed/added total claimed.

## Raw observations and limitations

- `matrix.json`: full steady-state matrix. Three 100ms clean samples, no spinner/loading, populated terminal route content verified by recorded text/request lists. Stats captures show real counters and shards, not mid-load placeholders.
- `transitions.json`: actual-input filtered-empty, rejected Save draft preservation, Save/Cancel button types and events, real CDP Enter, populated profile selector, paste transitions, Stats nonzero/zero bars and last-row location, native Sessions/Agents select presence.
- `extra.json`: rejected Delete action produces exactly one `skills-error`; confirmation display also exposes prompt-specific erasure copy in a skill-delete dialog. This is an additional measured cross-domain wording defect, not permission to edit shared confirmation infrastructure in this wave.
- Error sites are NOT proven duplicate concurrent nodes. Rejected Save uses `.skill-form-error`, not `skills-error`; rejected Delete uses one `skills-error`. List-fetch 500 was not separately completed. Do not claim duplicate-error RED from two source sites alone.
- Profile native SELECT unlabelled; Sessions native SELECT has Project label; Agents native SELECT unlabelled. Sessions/Agents select existence is regression-only; their timeline/milestone payloads in this reconnaissance fixture were incomplete and displayed malformed responses. Those are fixture omissions, NOT product failures. No Sessions/Agents source changes authorized.
- Stats exact numeric denominator 7 is visually inferred from measured widths and fixture values plus READ `daily_peak` source; no accessible max attribute exists. Zero floor=1 is READ in helper source, not observable from all-zero widths alone. A revised oracle must require exact max=7/max=1 on proposed accessible progress, not merely max>=1.
- Last shard reached by DOM scrollIntoView, not keyboard. Keyboard-reachable bounded region remains pending implementation verification; no fabricated pass.
- No Home independent failure permutations exist to test baseline. No claim of backend auth correctness: no authenticated marker/header lane run.
- Profile response-race/identity transitions, empty-vs-missing fixture transitions, successful Save, list-error recovery, Stats stale-snapshot reload failure, delayed export readiness, prompt exclusion and keyboard-only link focus were not covered. These remain explicit future regression lanes, not observed deficits.
- Initial harness attempts had fixture installation detached at session closure, wrong wrapper-vs-input targeting, and wrong error selector/envelope. They were corrected; early malformed/timeout observations are harness failures, not application defects. Matrix assets were recaptured with installed fixture. `mcp-skills.png` predates corrected fixture and is NON-ACCEPTANCE; use matrix PNGs and `mcp-export.png` instead. Scripts are reviewable scratch probes, not a final reusable acceptance suite.

## Console/network and validation

`transitions.json` events contain the Chromium preload-integrity warning on document navigation, no captured app exception or Network.loadingFailed in successful corrected transition run. Fixture-rejected skill POST/DELETE 500 are intentional and not wire network failures. Initial failed probes generated evaluator errors; excluded from application defect counts, disclosed above. No actual backend API request was used; intercepted request ledger is in raw JSON. Asset requests returned matching bytes.

MEASURED checks: all three scratch scripts `node --check` exit 0; class census `sites=214 literal=190 const=19 brace=5 residue=0`, `emitted=176 hooks=176 missing=0 dead=0 unresolved=0`; token census `missing=0 dead=0`. Source is unchanged; Rust tests, fmt and clippy did not run and are not claimed. TDD-EXEMPT: unchanged-source reconnaissance, no behavior delivered.

## Catch ledger

Outcome-changing rendered catches supporting narrower revised work: blank filtered Skills result; unlabelled Profile picker; narrow Stats page overflow; unnamed decorative Stats bars; Export/Import controls visually run together. Additional finding: skill-delete confirmation uses prompt-specific erasure disclosure. Adequate regions preserved: Tags contrast/wrapping/destination, real textarea label/focus and import validation, Save/Cancel non-submit handling and draft retention, Stats exact visible counts/all rows/status text, native select types, existing shell navigation.

RENDER EVIDENCE:
- build: `cd web-ui && trunk build --release --dist ../.tmp/scratch/issue-118-w4-baseline/dist` → exit 0
- url: `http://127.0.0.1:49418`
- screenshots: absolute paths + SHA256 of each in `.tmp/scratch/issue-118-w4-baseline/SHA256SUMS`; 24 matrix PNGs plus MCP export capture; stale MCP skills capture explicitly excluded
- console: Chromium preload-integrity warning; no captured application exception in corrected run
- network: no captured loadingFailed; intentional fixture 500 for rejected writes; incomplete out-of-scope Sessions/Agents fixtures disclosed
- catches: filtered-empty silence; Profile missing label; Stats narrow overflow/unnamed bars; Export/Import adjacency; skill-delete disclosure wording
- evidence: this report; `matrix.json`, `transitions.json`, `extra.json`, `provenance.json`, `SHA256SUMS` under `.tmp/scratch/issue-118-w4-baseline/`
