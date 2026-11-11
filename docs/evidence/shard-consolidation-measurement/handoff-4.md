# Handoff

BLUEPRINT: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md
STAMP: sha256=d90461a51f51 (SYS-APPROVED 2026-11-11T03:13Z)
BLUEPRINT-ACK: sha256=d90461a51f51
LEDGER=/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.ledger.md
PRIOR HANDOFF: .tmp/progress/20261111-0252-shard-measurement.handoff-3.md (retained verbatim)
SCOPE: S08 syseng slice only — interrupted owned-child interruption/restore (blueprint step 8). S09/S10 untouched, still BLOCKED.

## Test Evidence

Exact blueprint command template (offline+locked, scratch manifest, canonical scratch target):
`env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR CARGO_TARGET_DIR=<scratch>/target cargo test --offline --locked --manifest-path <scratch>/Cargo.toml --test measurement <TEST> -- --exact --nocapture`

| Step | Exit | Distinguishing observable | Artifact |
|---|---|---|---|
| S08 RED (stubbed restore) | 101 | panic tests/measurement.rs:954 `restore #1 must leave exactly the baseline file set (zero extra tuple files)`; boundary asserts (delta changed=shard.db-wal only, `.engine.lock` appeared, registry/aux untouched) passed BEFORE the restore gate — interface-only is discriminated from executed restore | s08-red.log, s08-red-direct.log |
| S08 GREEN (restore implemented) | 0 | `test result: ok. 1 passed` — both phases in one run | s08-green.log |
| Mutant M1 (restore skips `-wal`) | 101 | `restored bytes must equal the immutable baseline hashes before any reopen` — leaving committed WAL caught at hash gate | s08-mutant1-leavewal.log |
| Mutant M2 (restore retains dead child's `.engine.lock`) | 101 | `restore #1 must leave exactly the baseline file set` — stale PID lock never survives as active custody | s08-mutant2-retainslock.log |
| Gates: rustfmt --check / serial suite / clippy `-D warnings` | 0 / 0 / 0 | `9 passed; 0 failed` (1.56s) | s08-final-gates.log, s08-suite.log |

Pristine tree proven after mutants: `sha256sum -c evidence/s08-pristine.sha256` OK (pre-rustfmt hashes; post-format hashes in commands.log).

## What was executed (first time, MEASURED)

Per phase, one fresh scratch root (`runs/s08-<phase>-<nanos>`, never reused):

1. `prepare ROOT` CLI — fresh closed synthetic fixture (shard.db + registry/aux-one/aux-two `fixture` tables).
2. Parent `acquire_engine_lock` → `Acquired` (legitimate custody, this process).
3. Baseline typed oracle read under custody via Turso (`PRAGMA integrity_check` ok; typed rows per table, `Value`-preserving, sorted-set serialization); handles dropped; root closed.
4. Snapshot of the entire CLOSED root before any child open/checkpoint: bytes in memory + SHA-256/len manifest + absence manifest; ownership locks (`.engine.lock`, `.turso-operation.lock`, `.turso-migrate.lock`, `status`) excluded from restorable data and recorded as `excluded_locks`.
5. Custody released (lock file unlinked, asserted).
6. Owned child CLI `child ROOT <phase>` spawned with piped stdio; parent waits ≤30 s for the flushed `SHARD-MEASUREMENT-PHASE <phase>` line (explicit handshake, never sleep-based); child lifetime bounded (marker at 12–20 ms measured).
7. Child killed immediately after the marker — no RELEASE ever sent; reaped, non-clean exit asserted.
8. Interruption delta measured against baseline: (a) `shard-before-registry` → only `shard.db-wal` changed + `.engine.lock` appeared, registry/aux-one/aux-two byte-untouched; (b) `aux-one-before-two` → `shard.db-wal` + `registry.db-wal` + `aux-one.db-wal` changed, aux-two untouched. These boundary deltas are the discriminator that the child died exactly at the declared stage.
9. Restore #1: baseline bytes rewritten, every non-baseline file removed (including the killed child's lock). Gates: exact baseline hash set, exact file set (zero extra tuple files), excluded locks absent.
10. Restore #2 before any reopen: byte-idempotent (full recomputed hash set equal).
11. Fresh `acquire_engine_lock` → `Acquired` (new ownership; the dead child's lock was never restored).
12. Turso reopen under new custody: `PRAGMA integrity_check` ok on all four DBs; recovered typed oracle exactly equals baseline oracle; committed child writes (`child-stage`, `registry committed`, `aux one committed`) proven absent from the recovered logical state.

Measured timings (final green artifacts, evidence/restore.json): marker 12–20 ms, kill <1 ms, restore1 2–3 ms, restore2 1–2 ms, reacquire <1 ms, reopen verify 25–33 ms per phase.

## Code touch (minimal)

- `.tmp/scratch/shard-measurement/src/lib.rs`: `use`→`pub use` for mem_store db/engine_lock/platform symbols + `pub async fn open` — re-export surface only, no behavior change.
- `.tmp/scratch/shard-measurement/tests/measurement.rs`: S08 test + helpers (`s08_restore`, snapshot/hash walk, typed oracle). No existing test renamed; none weakened.

## Artifacts

- `evidence/restore.json` — merged green phase reports (manifests, deltas, timings, gates), red + mutant references.
- `evidence/results.json` — `restore_executed: true` + `s08` block; suite_tests 9.
- `evidence/commands.log` — S08 block appended (ledger reopen, RED/GREEN, mutants, gates, timings, baseline proof).
- `evidence/s08-*.log` — all runs; `evidence/ledger-check.py` — verbatim blueprint shape checker.
- Ledger: wave reopened (prior end 04:37:32Z retained in WAVE HISTORY); S08 BLOCKED→REWORK→DONE same-row, prior cycles retained; shape checker PASS after every write. S09/S10 remain BLOCKED. Wave left OPEN (no WAVE END, no LEDGER_CLOSE).

## Immutable baseline (re-verified before and after)

1399/1399 `fetch-before.sha256` entries match working bytes; `git status --porcelain` byte-equal to `fetch-before.status`; scratch `Cargo.lock` sha256 unchanged `5f57daf973586cc12fd62f5d58118ff3165378b791f7e1459455170e51f2e88a`; scratch `Cargo.toml` `36926d5db86775dccb7236f0016e5cfadcba7369e2be6017d35fea07ac557554`. Root and tracked files untouched. No live store/config/vault/service touched. No network action. Scratch not cleaned (disposable; no cleanup before orchestrator acceptance).

## Limits (honest)

Process-kill recovery only — no power-loss or filesystem-atomicity claim. Byte equality after reopen not required (reopening may rewrite sidecars). Auxiliary coverage is synthetic registry/aux-one/aux-two only; facts/vault/ai-sessions remain UNKNOWN. Ownership-lock names excluded per mem-store constants (`ENGINE_LOCK_FILE`, `OPERATION_LOCK_FILE`, legacy migrate lock, `status`).

## Ruling impact

S08 prerequisite now MEASURED for draft A (owned-child interrupted restore passes, idempotent, with real custody discipline). Overall ruling remains INCONCLUSIVE until S09 (admission observation) and S10 (final suite + A/B settlement) complete. Phase C remains blocked.

## Remaining work

1. Independent review of this S08 evidence against blueprint step 8 and the test-plan rows for `test_interrupted_cross_db_restore_matches_baseline_and_is_idempotent`. Any rejection → same-row REWORK, never replay.
2. S09 syseng/coder: `test_existing_barriers_report_observed_coverage` — existing public operation/scope lock interfaces on synthetic root; truthful per-path observations; any unexercised writer/reader path keeps B inconclusive.
3. S10: final seven-criterion serial suite, report/results agreement, A/B settlement under the blueprint's ruling rules; INCONCLUSIVE is a valid outcome.
4. Later authorized wave: tracked copy of report/evidence/harness under `docs/evidence/shard-consolidation-measurement/`. Until then scratch is NOT durable evidence; no cleanup before orchestrator acceptance.

Relay to orchestrator (for approved memory recording only — no memory adds from this provisional blueprint): restore evidence is the tuple set (snapshot-before-checkpoint ordering, byte-idempotent double restore, fresh-lock reacquire), not counts; a stale PID lock is custody debris to remove, never data to restore; scratch is disposable.

HANDOFF: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.handoff-4.md
