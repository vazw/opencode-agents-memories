# Independent review 2 — partial S02–S07 fixture (blueprint d90461a51f51)

Scope: coder-owned S03–S07 fixture against `blueprint-20261111-0310-shard-measurement.md` (sha256 d90461a51f51, SYS-APPROVED 2026-11-11T03:13Z). S08 owned-child execution, S09 admission, S10 report are pending, not claimed. Verdict is partial-acceptance; the wave measurement remains INCONCLUSIVE regardless.

## Re-executed verification (MEASURED, this review)

- Full serial suite, exact blueprint command/prefix, offline locked: **exit 0, 6 passed** in 0.68s (`evidence/review-2-suite.log`). All five S03–S07 named tests present with blueprint-exact names; sixth test (`test_owned_child_refuses_outside_root`) is an addition for the S08 interface, not a rename.
- `cargo clippy --offline --locked --all-targets -- -D warnings` on the scratch manifest: **exit 0** (`evidence/review-2-clippy.log`). The reported first failure (`collapsible_if`, exit 101) is corrected; final state green. `rustfmt --edition 2024 --check` lib/main/tests: **exit 0**.
- Immutability re-derived, not inherited: all **1399** entries in `evidence/fetch-before.sha256` match working-tree bytes (0 mismatches); scratch `Cargo.lock` sha256 = `5f57daf973586cc12fd62f5d58118ff3165378b791f7e1459455170e51f2e88a` (matches recorded); `git status --porcelain` byte-equal to `fetch-before.status` (same 3 pre-existing tracked edits, unchanged). `metadata-2.json`: `target_directory` = exact scratch target, workspace root = scratch, turso 0.8.3.
- Ledger: exact blueprint shape checker (incl. LEDGER_CLOSE semantics via non-`-` WAVE END) **PASS** under my own execution; WAVE HISTORY retains prior end verbatim; S02 REWORK history retained; handoff-1/review-1 preserved.
- RED/GREEN: per-slice RED logs exist with genuine test panics (`containment-red.log`, `raw-red.log`, `duplicate-red.log`, `divergent-red.log`, `cross-db-red.log`); GREEN exits recorded in `commands.log` with shell ids. REDs are measurement-contract panics (missing artifact/oracle assertions in the harness tests), not production regressions — the harness contains no production code. Historical panic line numbers cannot be re-derived against the evolved test file; noted as a READ limit, not a finding.
- `evidence/restore.json` correctly absent — no restore was executed, matching S08 BLOCKED. `results.json` honestly records INCONCLUSIVE, `restore_executed: false`, `online_admission_executed: false`, and the five admitted limits.

## Mutation spot-checks (2 probes, scratch copies `shard-review-mutant-2`/`-3`, isolated `target/review-mutant-*`; live workspace untouched)

1. **SURVIVED ⇒ Vacuous oracle (S07).** Mutant: delete the auxiliary remap persistence block in `auxiliary_fixture` (`.tmp/scratch/shard-measurement/src/lib.rs:736-750`, the `DELETE FROM {table}` + mapped-row INSERTs). `test_cross_db_mapping_preserves_payload_and_rejects_unknowns` stays **green** (`evidence/review-2-mutant.log`). The test never re-opens any auxiliary DB after the fixture build; `payload_preserved`/`collision_refused` are flags the same code writes into its own JSON. Missing distinguishing input: re-open each of the 8 DBs and assert persisted identity-binding columns equal the remapped targets and payload bytes are unchanged. Consequence: the remapped values (`synthetic_project_target`, `/home/opencode/synthetic-missing-repo`, `/home/opencode/Code/opencode`) are never proven insertable under the real UNIQUE/PK/FK constraints — a collision or constraint failure on the mapped write path is invisible.
2. **SURVIVED ⇒ guard clause unpinned (S03).** Mutant: remove `|| root.exists()` from the `run_case` containment guard (`src/lib.rs:41`). `test_refuses_outside_or_symlink_root` stays **green** (`evidence/review-2-mutant3.log`) because `create_dir` then fails identically for non-existing roots, making the clause's removal unobservable by the suite. The blueprint's row-1 declared mutant "existing-root guard removed" survives. Missing distinguishing input: pre-create an **empty** directory under `runs/` and assert `run_case` refuses it and leaves it empty (non-empty reuse is caught only incidentally via `create_dir` failure).

## Admitted-gap adjudication (handoff §Independent-review limits)

- **S04 Observation schema/duration — REJECT the DONE claim as-is.** Blueprint shared contract requires `Observation` to record schema, typed per-column differences, blob lengths/bytes, counts, cosine before/after and duration. `run_case` always returns the zero `Observation` (`src/lib.rs:65-75`: `schema: Null`, `row_differences: []`, `duration_ms: 0`, empty `artifact_paths`) even for `RawCopy`, which measured all of it into `raw-copy.json`. The test asserts only the artifact file. All measured data exists; the contract field population does not.
- **S05 dedicated duplicate Case commit — REJECT the DONE claim as-is.** Test-plan seam is `run_case(ExactDuplicates)`; `Case` has only `Containment`/`RawCopy`. `inspect` is exercised with real typed fixture rows (harvested via the RawCopy artifact), reverse-order/zero/singleton/all-equal and occurrence-manifest assertions are genuinely non-vacuous — but no Case experiment, no persisted duplicate observation artifact, no populated `Observation.duplicate_occurrences`.
- **S07 auxiliary-refusal closed-file hashes — folded into finding 1.** Refusal paths are asserted in-memory (unknown table/identity, facts/vault sentinels, divergent auxiliary key) and sentinel bytes are checked unchanged, but no SHA-256 manifest over the closed case root is retained per refusal, and (per finding 1) persisted mapped state is never re-verified.
- Honesty check: the handoff does not fabricate green for these; S08/S09 are explicitly not claimed (`no interruption/restore executed`, `No such observation executed here`). That candor is correct and is why this is a bounded rework, not a process violation.

## Per-row disposition

- S01 DONE: **accepted.**
- S02 DONE: **accepted.** Fetch exit 0 (single, locked, `evidence/fetch.log`), immutability re-verified by me, offline locked metadata contained, Turso 0.8.3, ledger/history intact.
- S03 DONE: **accepted, with required hardening** — add the empty-existing-root refusal assertion (finding 2) in the same rework pass; it pins the guard the rework will touch.
- S04 DONE: **rejected → authorized same-row REWORK.** Populate `Observation` (schema, row_differences, duration_ms, artifact_paths, and the raw-copy payload) from the measured values before returning, and assert those fields non-zero/non-empty in `test_raw_blob_and_all_columns_roundtrip`.
- S05 DONE: **rejected → authorized same-row REWORK.** Add `Case::ExactDuplicates` (and `Case::DivergentIds`/`Case::CrossDb` per the fixed-enum contract), route through `run_case`, persist per-case observation JSON, populate `duplicate_occurrences`.
- S06 DONE: **accepted.** All gate bullets met (13 non-association columns individually diverged, vector byte flip, tags NULL-vs-blob, good row before late conflict, work-tuple hashes unchanged, `examined == columns-6`). The preflight-before-copy *ordering* against a real copy pipeline remains unproven until the S05 Case work lands; covered there, not a separate defect.
- S07 DONE: **rejected → authorized same-row REWORK.** Fix finding 1 (re-open + persisted-state oracle), route through `run_case(Case::CrossDb)`, and retain closed-file SHA-256 manifests for every auxiliary refusal case.
- S08 BLOCKED: **correct.** Interface compiled, refusal-only test, no execution claim. No snapshot/restore manifest exists; syseng custody gate outstanding.
- S09/S10 BLOCKED (dependency S08): **correct.** Six-test suite is partial; no admission observation ran; report draft retained in `.tmp/drafts/` pending S10.

## Security gate (software-security-reviewer, 10 Mandates)

| # | Mandate | Verdict | Evidence |
|---|---|---|---|
| 1 | Authentication | N/A | offline synthetic harness; no auth surface exists in the diff |
| 2 | Authorization/IDOR | N/A | no multi-tenant surface; local root-scoping guards substitute (src/lib.rs:31-58, 820-847) |
| 3 | Input validation | PASS | path/component/symlink/existing-root checks lib.rs:35-58; child phase whitelist lib.rs:822 and containment lib.rs:825-846; untraced identity encodings refused lib.rs:503-519; unknown auxiliary schemas refused lib.rs:529-538 |
| 4 | Secrets management | PASS | no credential-shaped literals in harness/evidence; `fetch.log` sanitized, no tokens |
| 5 | Data protection | PASS | synthetic sentinels only; no live store opened; encryption disabled deliberately for fixtures (lib.rs:96-98); `refuse_unsupported_auxiliary` guards facts/vault/ai-sessions (lib.rs:781-788) |
| 6 | Privacy by design | PASS | no live PII read or copied; fixture text is synthetic; `/home/vaz/...` strings are fixture literals inside scratch only |
| 7 | Error handling | PASS | `MeasurementError` enum, no silent skips; child handshake timeout/EOF returns Err, not panic (lib.rs:894-900); main exits 1 (src/main.rs:18-21) |
| 8 | Logging | PASS | only the flushed phase marker (lib.rs:885); commands.log records commands/exits, no secrets |
| 9 | Dependency security | PASS | `cargo audit --stale --file` scratch lock: **exit 0, 0 advisories, 339 dependencies** (`evidence/review-2-audit.log`); turso pinned `=0.8.3` (Cargo.toml:9) |
| 10 | Secure configuration | PASS | manifests/locks immutable (1399/1399 hashes re-verified); Cargo output contained — `target_directory` confirmed via locked metadata; no debug/service endpoints added |

Zero VIOLATION — security gate passes.

## Process disclosure

This review performed one read-only external action beyond offline bounds: `cargo audit` refreshed its default advisory-DB git cache (`--stale` permitted the stale DB; the refresh is cargo's default behavior). No repository, scratch-measurement, live store, config, or vault mutation resulted; all other review actions were offline and contained. Scratch mutation copies `.tmp/scratch/shard-review-mutant-{2,3}/` and logs `evidence/review-2-*.log` are retained for orchestrator inspection.

## Required rework (same-row, prior proof/history preserved per ledger protocol)

1. S07: persisted-state oracle after `auxiliary_fixture` (re-open all 8 DBs; assert remapped bindings + byte-equal payloads), `Case::CrossDb` routing, refusal-case SHA-256 manifests.
2. S05: `Case::ExactDuplicates` (+ `DivergentIds`/`CrossDb`) experiments through `run_case` with persisted observation artifacts and populated `Observation`.
3. S04: populate the returned `Observation` for `RawCopy` and assert it in the named test.
4. S03: empty-existing-root refusal assertion in `test_refuses_outside_or_symlink_root`.

These are bounded fixture/test changes to scratch files only; no manifest/lock, tracked, or live-surface change is authorized. Until rework lands and S08–S10 execute, the measurement stays INCONCLUSIVE and Phase C remains blocked.
