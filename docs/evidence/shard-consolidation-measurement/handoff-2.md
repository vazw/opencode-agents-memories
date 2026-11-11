# Handoff
BLUEPRINT: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md
STAMP: sha256=d90461a51f51 (SYS-APPROVED 2026-11-11T03:13Z)
BLUEPRINT-ACK: sha256=d90461a51f51
LEDGER=/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.ledger.md

[TEST EVIDENCE]
TDD-EXEMPT: S02 authorized dependency acquisition/bootstrap/source tracing. Single locked fetch sh_6ukyk4em exit 0; offline locked metadata sh_wxw3bouo exit 0. Existing manifests/locks immutable; scratch lock SHA256 5f57daf973586cc12fd62f5d58118ff3165378b791f7e1459455170e51f2e88a. Final baseline verifies 1399 tracked/manifest/lock files and git status unchanged. No repeat fetch after server restart. S01 untouched.

## Exact test command
Run from repository root, background:
`env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR CARGO_TARGET_DIR=/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/scratch/shard-measurement/target cargo test --offline --locked --manifest-path /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/scratch/shard-measurement/Cargo.toml --test measurement TEST_NAME -- --exact --nocapture`

| TEST_NAME | Actual RED → GREEN | Distinguishing input / evidence |
|---|---|---|
| test_refuses_outside_or_symlink_root | 101 → 0 | runs-evil sibling assertion; traversal, symlink ancestor and second call/nonempty root; containment-red.log/containment-green.log |
| test_raw_blob_and_all_columns_roundtrip | 101 → 0 | missing typed oracle initially; 19 actual columns, private-only/mixed text, distinct payload/timestamps/session fields, vectors [2,1,0]/[1,3,0], queries [3,0,1]/[0,2,1], opposite rankings and independent formula; raw-red.log/raw-green.log |
| test_identical_ids_collapse_only_after_association_normalization | 101 → 0 | shared pinned/private ID across target+two sources, five occurrences vs three unique rows; reverse order, zero/singleton/all-equal; duplicate-red.log/duplicate-green.log/duplicate-actual-green.log |
| test_any_nonassociation_difference_aborts_without_writes | 101 → 0 | initial content conflict incorrectly accepted; every 13 non-ID nonassociation payload fields individually changed, vector byte, NULL/blob tags, timestamps/metadata; good row before late conflict; work tuple sha256 unchanged; divergent-red.log/divergent-green.log |
| test_cross_db_mapping_preserves_payload_and_rejects_unknowns | 101 → 0 | initial source identity unchanged; two-to-one missing-repo identity and special JS/opencode target, five episodic tables, prompts/skills/registry; source DDL skills limit; unknown encoding/schema and divergent auxiliary key refusal; cross-db-red.log/cross-db-final.log |

Raw negative controls executed: blob byte flip, constant distances, wrong duplicated query key, injected 2e-6 difference rejected at 1e-6. Raw RED was missing measurement-contract artifact, not a failed production preservation claim.

Final serial command uses same prefix/manifest with `--test measurement -- --test-threads=1 --nocapture`: sh_5irfnvf6 exit **0**, **6 passed**, 0.73s. Sixth test is owned-child outside-root refusal; not an interruption/restore substitute. No named tests silently renamed; S08/S09 named tests remain unimplemented and not passed.

Clippy same contained prefix, `cargo clippy --offline --locked --manifest-path <scratch manifest> --all-targets -- -D warnings`: first exit 101 collapsible_if, fixed; final exit **0**. `rustfmt --edition 2024 --check` scratch lib/main/test exit **0**. Accidental `cargo test --help` on root manifest exit 0 performed no build/test and changed no frozen file; logged, not evidence of behavior.

## Ledger and completion
S01 unchanged DONE. S02–S07 recorded DONE with evidence; S08 BLOCKED for syseng execution; S09–S10 BLOCKED dependency S08. Final exact blueprint checker LEDGER_CLOSE=1 PASS. Original closure/start, S02 blocked cycle and all dependency histories retained. Prior handoff/review unchanged. Current blocker closure supersedes historical bootstrap blocker paragraph.

**Independent-review limits:** S04 Observation schema/duration remain artifact-only, not populated in returned Observation. S05 typed duplicate preflight is exercised on real fixture rows but does not commit a dedicated Case experiment. S07 refusal tests do not yet retain full closed-file SHA256 manifests for every auxiliary refusal. Facts/vault presence refusal is a synthetic sentinel guard, not production support. S08 interface added without claiming a completed TDD interruption slice. These gaps remain reviewable requirements; DONE rows require authorized REWORK if rejected. No fabricated green claim for these missing criteria.

## Artifacts and ruling
MEASURED: `evidence/fetch.log`, metadata-2.json, immutable fetch-before.sha256/status, named RED/GREEN logs, schema.json, results.json, suite-2.log/clippy-final.log; per-run raw-copy.json/auxiliary.json and actual schema inventories. Harness src/lib.rs/src/main.rs/tests/measurement.rs and unchanged manifest/lock retained.
INCONCLUSIVE: no A/B victory, actual admission/drain observation, whole-root restore or production completeness established. Phase C remains blocked. Report retains A/B pros/cons/predictions/refutations and limits. Scratch is not durable evidence; approved later tracked copy required before cleanup.

## Remaining syseng / executor work
1. Independent review now. Any S03–S07 corrections use same-row REWORK with prior proof/history; do not repeat DONE mutations without authorization.
2. S08: CLI `target/debug/shard-measurement prepare ROOT` accepts a fresh scratch runs descendant, creates synthetic shard+registry+two auxiliary fixtures and closes handles. Syseng must acquire EngineLock custody and snapshot the entire CLOSED root before any reopen/checkpoint; record immutable bytes/SHA256/absences, excluding ownership locks. No snapshot/restore manifest exists yet.
3. Release parent's custody before child acquisition. Run only owned child `target/debug/shard-measurement child ROOT shard-before-registry` or `aux-one-before-two`; read flushed `SHARD-MEASUREMENT-PHASE <phase>`. Child holds real DB handles and awaits explicit stdin RELEASE (30s bound). Interrupt only spawned child after marker at each boundary; never use production PIDs. These operations were NOT executed.
4. After exit/closed handles, restore every baseline tuple plus absences, remove new tuples, repeat restore and verify byte-idempotence before reopen; reacquire legitimate EngineLock, open actual Turso, verify integrity/full logical baseline, and record timings. Implement/execute exact S08 named test; interface alone proves none of this.
5. S09: implement exact admission test with actual public operation/scope interfaces and permitted auxiliary seams, bounded admitted closure latch, same/different scope, held/released outcomes and honest not_exercised entries, drain/stale-handle gaps. No such observation executed here.
6. S10: run full seven named criterion suite including syseng restore/admission evidence, settle hashes/report/results agreement and A/B ruling. Current six-test suite is partial, not completed S10. ai-sessions.db remains UNKNOWN; facts/vault/production completeness remain blockers.
7. Later authorized wave copies report, both alternatives, raw evidence, harness/lock, source map and hashes into tracked docs/evidence/shard-consolidation-measurement/. No cleanup before orchestrator accepts.

HANDOFF: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.handoff-2.md
