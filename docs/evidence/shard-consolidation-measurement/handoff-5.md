# Handoff

BLUEPRINT: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md
STAMP: sha256=d90461a51f51 (SYS-APPROVED 2026-11-11T03:13Z)
BLUEPRINT-ACK: sha256=d90461a51f51
LEDGER=/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.ledger.md
PRIOR HANDOFF: .tmp/progress/20261111-0252-shard-measurement.handoff-4.md (retained verbatim; its three review-4 advisories addressed below)
SCOPE: S09 admission observation + S10 final bounded measurement/settlement. Wave CLOSED: S01–S10 all DONE, WAVE END 2026-11-11T06:12:23Z, ledger checker PASS incl. LEDGER_CLOSE=1 run. No cleanup performed.

## Test Evidence

Exact command template (offline+locked, scratch manifest, canonical scratch target):
`env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR CARGO_TARGET_DIR=<scratch>/target cargo test --offline --locked --manifest-path <scratch>/Cargo.toml --test measurement <TEST> -- --exact --nocapture`

| Step | Exit | Distinguishing observable | Artifact |
|---|---|---|---|
| S09 RED (blueprint target mutant: hardcoded all-blocked ledger) | 101 | panic at persisted-artifact truthfulness gate — `operation-lock/second-acquire-while-held` expected `refused`, got `blocked`; all real exercises had already run and passed their live assertions first | s09-red.log |
| S09 GREEN (measured statuses) | 0 | `test result: ok. 1 passed` (1.04s); persisted admission-observations.json carries per-path measured statuses + µs timings | s09-green.log, evidence/admission.json |
| Mutant M2 (`s09_classify` unknown→blocked) | 101 | negative-oracle gate: fabricated path must classify `unknown`, never `blocked`; also kills not_exercised mislabeling | s09-mutant-unknown-blocked.log |
| Pristine after mutant | — | `sha256sum -c evidence/s09-pristine.sha256` → tests/measurement.rs OK, src/lib.rs OK (supersedes pre-rustfmt s08-pristine per review-4 advisory) | s09-pristine.sha256 |
| Gates: rustfmt --check / serial suite / clippy `-D warnings` | 0 / 0 / 0 | `10 passed; 0 failed` (2.60s), `-- --test-threads=1`; clippy clean after 3× `bool_assert_comparison` fixes | s09-final-gates.log, s09-clippy-final.log |
| Baseline re-verify (S10) | 0 | `sha256sum -c evidence/fetch-before.sha256` → 1399/1399 OK; git status diff vs fetch-before.status empty; Cargo.lock `5f57daf9…`, Cargo.toml `36926d5d…` unchanged | .tmp/baseline-check.txt (transient), commands.log |

Two GREEN-iteration command wrappers echoed literal `EXIT:$?` (shell-tool mangling); those runs are evidenced by cargo `test result:` lines in the captured logs, noted in commands.log.

## S09 measured outcome (MEASURED, first execution)

Matrix over existing public operation/scope/delete-guard seams, synthetic root, real Turso aux writes, timings in µs: **12 completed / 2 refused / 1 blocked / 5 not_exercised** (evidence/admission.json).

- Operation lock: second acquire while held refused `MigrationLocked{pid=self}`; completes after release; migration assert refuses while live, ok after release.
- Stale dead-PID lock: `read_live_lock` unlinks it (reclaim); acquire after reclaim completes. Expectation corrected to measured mechanism — `acquire_turso_operation_lock` runs the migration assert first, whose live-lock read reclaims transparently, so the first acquire completes; the discarded "first acquire refuses once" expectation is recorded in commands.log, not asserted.
- Scope barrier `ShardManager::with_scope_write_lock` (per-(scope,hash) async mutex): holder latched; same-scope contender blocked (bounded 1s negative poll); distinct scope completed while held; queued contender drained only after release. Scoped, not root-wide.
- Delete guard: `ExclusiveRoot::current` Some while custody held; None only after BOTH the `ExclusiveRoot` proof token and custody drop — the proof holds a strong `Arc<EngineLock>` (measured drop-order requirement).
- Aux Turso read/write completed under the operation lock, under the scope lock, and after release — unbarred direct SQL bypasses barriers by construction.
- 5 not_exercised with reasons (skills-manager, mem-episodic direct SQL, mem-bridge real callers, connection drain/stale cloned handles, power-loss durability); missing paths classify `unknown` (negative-oracle asserted). No invented root-wide barrier, no injected Busy stubs.

Harness-bug fixes during GREEN iterations (documented commands.log): released-lock drop order for the migration assert; stale-expectation correction; ExclusiveRoot/custody drop order.

## S10 settlement (MEASURED)

- Final serial suite: 10 passed, 0 failed, exit 0 (2.60s). Clippy `-D warnings` exit 0; rustfmt `--edition 2024 --check` PASS.
- results.json reconciled: status `complete`, tests_passed 10, stale limits[0]/[1] replaced with S08/S09-executed truth, review-4 advisories recorded (pristine supersession; custody/kill_ms µs note), final tree hashes lib.rs `e755acc5…` / main.rs `75f3d936…` / tests `37a5bca6…`. Report draft (S09 + S10 sections) and source-map S09 appendix appended; commands.log appended.
- **A/B ruling: INCONCLUSIVE** per the blueprint decision rules: (1) A prerequisites measured PASS — S04 lossless raw typed copy, S08 interrupted owned-child restore byte-idempotent; (2) B barrier sufficiency UNPROVEN — 5 traced admission paths not_exercised, no measured bypass either way; Busy/refusal counts alone prove nothing; (3) no A-specific failure observed ⇒ no B win; both-feasible is not an automatic A win. Losing alternative B retained with its bootstrap prediction/refutation condition. Policy recommendation only (never a measured claim): prefer draft A (offline maintenance) at the later design gate, since future downtime is accepted by recorded user policy.

## Remaining work

1. Independent review of this S09/S10 evidence against blueprint steps 9–10; any rejection → same-row REWORK, never replay.
2. Orchestrator acceptance of the scratch artifacts; no cleanup before acceptance (scratch `runs/` case roots retained).
3. Memory adds remain forbidden under this provisional blueprint — relay S09/S10 facts (matrix, reclaim mechanism, per-scope barrier, strong-Arc custody lifecycle, INCONCLUSIVE ruling + policy recommendation A, Phase C blocked) to the orchestrator for approved recording.
4. Later authorized wave: tracked copy of report/evidence/harness under `docs/evidence/shard-consolidation-measurement/`; until then scratch is NOT durable evidence.

HANDOFF: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.handoff-5.md
