# Independent archive review 1
STATUS: REJECTED

Scope: prerequisite Step 1 and prepared Step 2 only, blueprint d71b540b11fd. No production implementation or commit approval.

## Actionable finding

- **Incomplete referenced run closure.** `docs/evidence/shard-consolidation-measurement/README.md:23` claims complete tuple/sidecar closure, but `reference-index.json:70–117` misclassifies 35 existing synthetic target artifacts as contextual exclusions. Seven retained runs lack `target.db`, `target.db-wal`, `target-schema.json`, `work/target.db` and `work/target.db-wal`: `divergent-12955`, `divergent-4807`, `divergent-5210`, `duplicates-12955`, `duplicates-4564`, `raw-copy-12955`, `raw-copy-2952`. MEASURED by enumerating each original named run and comparing every file against relocation-map; shell `sh_gyihnrcw` failed on the first missing target; `sh_ji7szcug` enumerated all 35. These are persisted measurement outputs, not compiler targets. Copy them byte-for-byte, add mappings, correct reference classifications, regenerate the final manifest and handoff metrics, then repeat independent closure acceptance. Do not alter originals or run DB recovery. This blocks Step 2 acceptance even though hashes of the incomplete archive pass.

## Verified evidence and limits

- MEASURED: all 271 declared original mappings are byte-identical; exact archive payload set equals the manifest's 276 entries; every payload digest recomputes correctly. Complete current archive is 277 files / 6,202,849 logical bytes. Manifest digest `a0d8fad9570eecc1e913c18c716d9de174d68b8e146f8d95ce06e33e9ee8ae4f`. No symlinks, target directories, .git directories or Python caches found. These checks preceded the named-run closure failure in `sh_gyihnrcw`.
- MEASURED: relocated settlement exits 0 with every original check passing (`sh_ji7szcug`). READ: adapter `check-relocated-settlement.py:4–10` changes exactly one locator in memory; the historical checker remains byte-identical. Its narrower success does not verify archive reference closure.
- MEASURED: additional stale-source probe in `.tmp/scratch/consolidation-review-1-mutant` appends a source comment after manifests. Unchanged relocated runner exits 1 with checks 4 and 5 failing (`sh_fkce7pes`). Original archive untouched. Historical Rust suite was not rerun, as explicitly prohibited for this preservation review.
- READ/MEASURED: TDD exemption applies to historical preservation, not a new production implementation. The new relocation adapter was executed; all archived Python files AST-parse. Harness Cargo.toml/Cargo.lock/src/tests/source-map and rejected/debug history are retained. README:39–51 honestly documents future dependency/RUNS relocation rather than claiming a new reproduction. No Markdown linter configured; Rust builds/clippy/format were not run for this byte-preservation scope.
- MEASURED: archived result is INCONCLUSIVE, Phase C blocked, and exactly five unknown admission paths agree across admission/results. READ: README:7–17 retains policy-only A preference, B alternative and all five unknowns.
- MEASURED: four protected baseline SHA-256 values match ledger:19–22; index empty; HEAD `df34c522cd349ddb59f021fc82eae64190123e2f`; Git status matches the handoff. READ: Step 2 remains unchecked, Step 3 is a stop point, Steps 4–13 remain unapproved; no authority was expanded.
- MEASURED/READ: executor `sh_m4ovunee` failed with an unterminated Python string at parse time, before its script could execute. Corrected saved-script initialization `sh_up2rzsod` and subsequent validation `sh_rnxfntmm` exited 0. This is a corrected infrastructure error, not an unresolved archive gate; it does not excuse the independent closure failure above.
- READ: recalled bug-pattern `mem_1794379534835_00e156e24` warns against stale final manifests and self-reported flags. Current hashes pass independent recomputation; this review does not reopen previously fixed historical oracle findings.

## Security gate

Archive-source/config preservation only; no deployed service change. No production DB was opened by this review. Synthetic fixture provenance is READ from harness source and byte mappings, not inferred from filenames alone. Credential pattern scan of archive text found no credential-shaped keys/private-key headers; this is not a blanket proof of absence.

| # | Mandate | Verdict | Evidence |
|---|---|---|---|
| 1 | Authentication | N/A | README.md:3; no service/auth change |
| 2 | Authorization | N/A | README.md:3; no API/resource-access change |
| 3 | Input validation | PASS | check-relocated-settlement.py:7–9 requires one exact trusted locator; historical harness src/lib.rs:37 onward bounds synthetic roots |
| 4 | Secrets management | PASS | Text credential scan `sh_fkce7pes`; README.md:23 excludes private production roots |
| 5 | Data protection | N/A | README.md:23 synthetic fixture bytes, no transport or production storage change |
| 6 | Privacy by design | PASS | harness/src/lib.rs:193–216,315–380 synthetic fixture construction; relocation-map original paths; no live data collection |
| 7 | Error handling | PASS | check-relocated-settlement.py:8 fails closed; historical check-settlement.py:118–121 exits nonzero |
| 8 | Logging | PASS | relocated-settlement.log and historical check-settlement.py:24–28 record artifact checks, not production row payloads |
| 9 | Dependency security | PASS | Cargo.toml/Cargo.lock byte-preserved; root `cargo audit --no-fetch` in `sh_jjrdpfhh` exits 0, local 1,446-advisory DB / 495 dependencies; no network fetch. Initial unsupported `--offline` invocation exited 2, corrected before this result. Historical harness lock is retained evidence, not deployed dependency integration |
| 10 | Secure configuration | PASS | README.md:3,41–51 explicitly prohibits direct historical harness execution and production operations; no ports/services enabled |

Inverted world: acceptance closes only if all required named-run files resolve byte-for-byte and the final manifest/metrics reflect that complete set; a green settlement checker alone cannot close it. Step 2 remains open and the archive remains uncommitted.
