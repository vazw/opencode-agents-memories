# Review 4 — S08 interrupted owned-child restore (independent)

BLUEPRINT: blueprint-20261111-0310-shard-measurement.md (body sha256 d90461a51f51 — recomputed, matches STAMP; prior 0257 body a2ebb3fcff66 / full 3c2a3c25… also re-verified)
LEDGER: 20261111-0252-shard-measurement.ledger.md — shape checker PASS (blueprint gate run verbatim via evidence/ledger-check.py; wave open, WAVE END `-`, no LEDGER_CLOSE)
HANDOFF-4 reviewed in full; handoff-3 remaining-work item 2 confirmed as the S08 rework authorization.

## Verified (all re-derived by this reviewer, not inherited)

- **Named test re-run, exact blueprint command** (`cargo test --offline --locked --manifest-path <scratch>/Cargo.toml --test measurement test_interrupted_cross_db_restore_matches_baseline_and_is_idempotent -- --exact --nocapture`, pinned scratch target): exit 0, 1 passed. Fresh case dirs `runs/s08-*-1794375612…` reproduced the full executor behavior.
- **Real restore, not interface-only**: RED log (`s08-red.log`) panics at the restore-set gate only, with boundary delta asserts (real WAL deltas, `.engine.lock` appeared, registry/aux untouched) passing *before* it — interruption mechanism and missing restore contract discriminated. Test code observes persisted state at every gate: real file walk + `sha256sum` recomputation (no cache), per-phase boundary deltas, Turso `PRAGMA integrity_check` ok on all 4 DBs under fresh custody, typed `Value`-preserving oracle (Real as `to_bits`, Blob bytes+len, sorted sets), committed child writes (`child-stage`, `registry committed`, `aux one committed`) proven absent. No self-reported flags as oracle — debug-1 vacuous-oracle family does not recur.
- **WAL deltas**: phase (a) `changed=[shard.db-wal]`, (b) `[shard.db-wal, registry.db-wal, aux-one.db-wal]`, `aux-two` untouched both; `missing=[]`; measured, not asserted from code.
- **Two-phase restore idempotence**: restore #2 compared byte-set equal before reopen (`restore2_idempotent: true`, recomputed).
- **Stale lock removal + fresh reacquire**: ownership locks excluded from baseline (names match production constants: `engine_lock.rs:41,44`, `operation_lock.rs:30,32`); excluded locks asserted absent post-restore; fresh `acquire_engine_lock` → `Acquired`.
- **Mutation spot-checks (2/3 budget, isolated copies `.tmp/scratch/shard-review4-m1/-m2`, isolated targets, offline locked; live scratch tree never mutated)**:
  - M1 (restore skips `-wal` rewrite) → **101** at `restored bytes must equal the immutable baseline hashes before any reopen` — reproduces executor's M1 exactly.
  - M2 (remove pass retains dead child's `.engine.lock`) → **101** at `restore #1 must leave exactly the baseline file set` — reproduces executor's M2 exactly.
  - Two distinct persisted-state gates kill two distinct mutants: non-vacuous, no equivalent-mutant issue.
- **Scratch-only diff**: git status byte-equal to `fetch-before.status` (only the 3 pre-existing S01-frozen tracked edits); suite diff `rework-suite-final.log` (8 tests) vs `s08-suite.log` (9 tests) = exactly one addition, zero renames/removals; load-bearing prior asserts intact (containment payload `assert_eq(message, root)`, `verify_persisted` + `persisted mismatch` at `src/lib.rs:1172/1234`, cosine delta `1e-6` at `tests/measurement.rs:117`); lib.rs change is `use`→`pub use` re-export + `pub async fn open` (lib.rs:165-178), no behavior change.
- **Immutability baselines re-run twice (before and after my own runs)**: 1399/1399 `fetch-before.sha256` OK; `git status --porcelain` byte-equal; scratch `Cargo.lock` 5f57daf9… and `Cargo.toml` 36926d5d… unchanged. No live store/config/vault/network action in evidence or diff.
- **Ledger transitions**: S08 same row BLOCKED→REWORK→DONE, prior cycles and authorization retained in history, single cursor discipline held; S09/S10 `[!]` BLOCKED (`dependency S08`); ruling INCONCLUSIVE (`results.json.ruling`, handoff §Ruling impact); Phase C blocked (`results.json.phase_c`).
- rustfmt `--edition 2024 --check` re-run by this reviewer on current tree: exit 0. clippy `-D warnings` exit 0 per `s08-final-gates.log` (CLIPPY-EXIT:0), consistent with the 9-pass serial suite in the same log.

## Security Gate (software-security-reviewer, 10 Mandates)

| # | Mandate | Verdict | Evidence |
|---|---|---|---|
| 1 | Authentication | N/A | Offline measurement harness; no auth surface in diff |
| 2 | Authorization | N/A | Single-user scratch; no multi-tenant resource named |
| 3 | Input validation | PASS | Child CLI mode/phase whitelist (`src/main.rs:7-19`, `src/lib.rs:909-913`); root containment re-validated in child incl. canonicalize==root and symlink ancestors (`src/lib.rs:916-933`); kill only own spawned child after bounded handshake (`tests/measurement.rs:830-883`) |
| 4 | Secrets management | PASS | No credential-shaped literals in diff; no tokens involved |
| 5 | Data protection | N/A | Synthetic sentinels only; nothing sensitive at rest/in transit |
| 6 | Privacy by design | PASS | No PII; live stores explicitly refused (`refuse_unsupported_auxiliary`, `src/lib.rs:870-878`); fixture marker required before child acts |
| 7 | Error handling | PASS | `MeasurementError` taxonomy; bounded 30 s handshakes fail toward INCONCLUSIVE, never silent success (`src/lib.rs:977-991`, `tests/measurement.rs:846-856`) |
| 8 | Logging | PASS | Child stdout is the single phase marker; no secrets/PII in any log |
| 9 | Dependency security | PASS | Exact-pinned `turso =0.8.3`, `tokio =1.53.3`, `serde_json =1.0.154` (`Cargo.toml`); lock byte-frozen (hash re-verified); no new dependency introduced, so no new CVE surface this wave |
| 10 | Secure configuration | PASS | No config/production surface touched; child lifetime bounded; nothing listens |

Violations: none. Gate verdict: APPROVED.

## Advisory notes (non-blocking; fold into S10's report/results agreement gate)

- `results.json` top-level `limits[0]` still reads "S08 interface only; no baseline snapshot/restore manifest" — stale against the same file's `s08` block and `restore_executed: true`. Reconcile at S10.
- `evidence/s08-pristine.sha256` is a pre-rustfmt point-in-time snapshot; `sha256sum -c` fails against the current post-format tree (honestly labeled in handoff-4). My independent re-runs supersede it; S10 should record post-format hashes or the supersession explicitly.
- `custody_ms`/`kill_ms` recorded as 0 — sub-millisecond `as_millis` truncation; cosmetic.

## Verdict

Every blueprint step-8 requirement and both test-plan rows for `test_interrupted_cross_db_restore_matches_baseline_and_is_idempotent` are fulfilled with measured, independently reproduced evidence: real owned-child kill after flushed phase markers with per-stage WAL deltas, snapshot-before-checkpoint whole-root manifest with ownership locks excluded, byte-idempotent double restore, fresh lock reacquire, Turso reopen integrity with exact typed oracle, and two non-vacuous mutant kills — all on the unchanged immutable baseline, scratch-only, S09/S10 BLOCKED, ruling INCONCLUSIVE, Phase C blocked.
