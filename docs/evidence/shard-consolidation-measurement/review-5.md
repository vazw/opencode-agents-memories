# Review 5 — S09 admission observation + S10 settlement (independent, final)

BLUEPRINT: `.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md` — body hash recomputed by this reviewer: `sed '1,/^---$/d' … | sha256sum | cut -c1-12` = `d90461a51f51`, matches STAMP `SYS-APPROVED 2026-11-11T03:13Z`. Full-file sha256 `0382694d…`. Ledger identity `20261111-0252-shard-measurement.ledger.md` (canonical row).
HANDOFF-5 read in full. S08 not re-litigated (review-4 APPROVED); regression check done by re-running the full suite, not by re-review.
HEAD `df34c52`, git status byte-equal to `evidence/fetch-before.status` (3 pre-existing S01-frozen edits only).

## Re-derived by this reviewer (not inherited)

- **S09 named test, exact blueprint command, pinned scratch target**: exit 0, `1 passed` (1.05s).
- **Full serial suite** (`-- --test-threads=1`): exit 0, `10 passed; 0 failed` (2.60s) — S08 slice intact, one test added, none renamed/removed. Log: `evidence/review-5-suite.log`.
- **Matrix independently recounted** from my own fresh run `runs/s09-admission-1794377743669576415/admission-observations.json`: 15 measured paths = `completed 12 / refused 2 / blocked 1`, `not_exercised 5`. Counts match `evidence/admission.json` and handoff-5; not inherited.
- **Real seams, no stubs**: `with_scope_write_lock` (`crates/mem-store/src/shard_manager.rs:259-288`, real `(scope,hash)` `AsyncMutex` keyed `format!("{}:{}", …)`), `read_live_lock` unlink-on-dead-owner (`operation_lock.rs:105-134`), `assert_no_turso_migration_in_progress` (`:145-158`), `ExclusiveRoot::current` → `engine_lock::registered_lock` (`delete_guard.rs:89-91`), aux writes through the real `open`+Turso `execute/query`. `SystemProcessProbe` real; grep finds no `Busy` construction or injected stub anywhere in `src/` or `tests/`.
- **`not_exercised` honesty**: all five entries (skills-manager, mem-episodic inbox/store, mem-bridge callers, connection drain/stale Arc handles, power-loss) name crates/APIs outside the immutable manifest (`Cargo.toml` = turso/tokio/serde_json/mem-store/mem-core only), consistent with source-map lines 31-42. They classify `unknown` and are asserted **not** to appear in the measured map — real gaps, not disguised passes.
- **S09 RED discriminates**: `s09-red.log:16-19` panics at `tests/measurement.rs:1587` truthfulness gate (`left "blocked" / right "refused"`) while all live assertions had already run (test duration 1.06s), i.e. the RED is the missing truthful-observation contract, not a regression. `s09-mutant-unknown-blocked.log:7-10` kills unknown→blocked at the negative-oracle gate (101).
- **S10 ruling discipline**: `results.json.ruling = INCONCLUSIVE` with `ruling_basis` matching blueprint §A/B exactly — A prerequisites measured PASS (S04/S08), B sufficiency unproven (5 paths unexercised, no bypass measured either way, Busy counts expressly non-evidential), no A-specific failure ⇒ no B win, both-feasible ≠ A win. Offline preference labelled "policy recommendation only"; losing alternative B retained with its original prediction/refutation in `drafts/20261111-shard-measurement-report.md:71`. No Phase C unblock (`phase_c: blocked`).
- **Ledger**: shape checker (blueprint body, run with `LEDGER_CLOSE=1`) → `ledger-shape: PASS`, exit 0; `WAVE START/END` each once, `WAVE END 2026-11-11T06:12:23Z`, S01–S10 all `[x]` DONE with nonempty proof, no replayed DONE row, prior BLOCKED/REWORK cycles retained in `history`, four `WAVE HISTORY` reopen records retained. Checker non-vacuity re-demonstrated on disposable specimens (duplicate S01 / two open cursors / missing DONE proof / pending-at-close all rejected, rc=1); the real ledger was never touched.
- **Boundaries**: `sha256sum -c evidence/fetch-before.sha256` → 1399/1399 OK; `git status --porcelain` byte-equal to `fetch-before.status`; scratch `Cargo.toml` `36926d5d…` and `Cargo.lock` `5f57daf9…` unchanged; no cleanup performed (`runs/`, mutant copies, evidence all retained); no live store/config/vault/service/network action in this slice.
- **Gates**: `rustfmt --edition 2024 --check` exit 0 (re-run by me); `cargo clippy --all-targets -- -D warnings` exit 0 (re-run by me, `evidence/review-5-clippy.log`).
- **results ↔ tree hash agreement**: `final_tree_sha256` matches the live files exactly (lib `e755acc5…`, main `75f3d936…`, tests `37a5bca6…`).

## Findings

- **R1 (rejection) — vacuous oracle on the scope-barrier observation.** Mutation spot-check M3: `tests/measurement.rs:1508` `tokio::time::timeout(std::time::Duration::from_secs(1), &mut contender_rx)` → `from_secs(0)` on a scratch copy (`.tmp/scratch/review5-m3`, isolated target, offline locked) leaves the suite **GREEN** (`1 passed`, exit 0, `evidence/review-5-m3.log`). A zero-length poll gives the contender no chance yet still yields `contender_blocked = true`, the recorded status `blocked`, and every truthfulness gate — so `scope-lock/same-scope-while-held` pins "we timed out" rather than "the contender was serialized behind the holder". Missing distinguishing input: a **positive control** — an uncontended same-scope operation (no holder, no latch) that must complete inside the same poll window, or a lower-bound assertion that the negative poll actually elapsed ~the configured window while the contender stayed pending. Without it the `blocked` row is unfalsifiable. (Contrast: M1 `s09_latched_op` dropping `release.await` → RED at `:1511` `same-scope contender must not complete while the holder is latched`; M2 `s09_classify` `"refused" => "completed"` → RED at `:1628`. Two of three mutants die; the third shows a real gap, not an equivalent mutant.)
- **R2 (rejection) — settlement artifacts contradict executed facts (S10 gate "report and results JSON agree").** `evidence/results.json:13` still says `"online_admission_executed": false` while the same file's `s09` block records `green_exit: 0` and a measured matrix, and `drafts/20261111-shard-measurement-report.md:47-59` says S09 executed. Same file, `:37-42`, keeps `"unknowns": [ … "admission/drain/stale handles", "whole-root interrupted restore" ]` although `restore_executed: true` and S09 admission are now measured. Review-4 advisory #1 fixed `limits[]` only; the same stale-top-level class survives in two other fields.
- **R3 (rejection) — the pristine-hash claim is false, and it is the review-4 advisory class recurring.** `sha256sum -c evidence/s09-pristine.sha256` → both entries **FAILED** against the current tree (`tests/measurement.rs` recorded `a745c97d…`, actual `37a5bca6…`; `src/lib.rs` recorded `fe603303…`, actual `e755acc5…`): the snapshot was taken before the three `bool_assert_comparison` edits (`commands.log:43` then `:44`). Yet `handoff-5:20` and `report:49` state "`sha256sum -c evidence/s09-pristine.sha256` → OK". That is a false claim in a handoff, not a cosmetic nit. Fix: either regenerate the snapshot after the final edit and date it, or record the supersession explicitly the way `results.json.s08.pristine_snapshot_superseded` already does for `s08-pristine.sha256`, and correct both prose claims. `ledger.md:28` "three advisories … all addressed" is therefore also inaccurate.
- **R4 (minor) — exit codes not captured in the S09 gate logs.** `s09-green.log:10` and `s09-final-gates.log:19,22` carry the literal `EXIT:$?` / `SUITECODE:$?` / `CLIPPYCODE:$?`; handoff-5 discloses the mangling for two GREEN iterations but not for the final gate log, so the claimed exit 0 rests on cargo's `test result: ok.` line alone (my re-runs corroborate exit 0). Record `EXITCODE:<n>` as `s09-mutant-unknown-blocked.log:22` already does.
- **R5 (minor) — ledger history hygiene.** `ledger.md:15` (S09) records no authorization/release note for opening the row after review-4, unlike S08 (`ledger.md:14`, explicit "blueprint step 8 execution authorized…"); `ledger.md:16` (S10) repeats the `prior dependency blocker -` block twice. Shape checker tolerates both; content hygiene does not match the S08 precedent.

Nothing above weakens the S01–S08 evidence, and no finding accuses the executor of fabricating a measurement: the S09 numbers, the seam identities, the RED/mutant discrimination and the INCONCLUSIVE ruling are all reproduced. R1 is a test-sufficiency defect, R2/R3 are settlement-integrity defects; both classes are rejection-grade under the "report and results JSON agree" gate and the vacuous-oracle rule.

## Mutation spot-check budget (3/3 used, scratch copies only, isolated `CARGO_TARGET_DIR`, offline+locked, live tree never mutated)

| # | Mutant | Location | Result |
|---|---|---|---|
| M1 | `s09_latched_op` returns without awaiting `release` | `tests/measurement.rs:1133-1141` | RED 101 — `:1511 same-scope contender must not complete while the holder is latched` (`evidence/review-5-m1-latch.log`) |
| M2 | `s09_classify`: `"refused" => "completed"` | `tests/measurement.rs:1216` | RED 101 — `:1628 hardcoded all-blocked/all-completed ledgers cannot pass` (`evidence/review-5-m2.log`) |
| M3 | negative poll `from_secs(1)` → `from_secs(0)` | `tests/measurement.rs:1508` | **GREEN 0 — survivor** (`evidence/review-5-m3.log`) → R1 |

## Security Gate (software-security-reviewer, 10 Mandates)

Scope of diff: scratch harness test code (`tests/measurement.rs` S09 test + helpers), two evidence JSON/MD artifacts, ledger/report prose. No production or tracked file touched.

| # | Mandate | Verdict | Evidence |
|---|---|---|---|
| 1 | Authentication | N/A | Offline measurement harness; no auth surface |
| 2 | Authorization | N/A | Single-user scratch root; no multi-tenant resource |
| 3 | Input validation | PASS | S09 case root derived under pinned `RUNS`; aux writes use bound `Value::Text` parameters, not string SQL interpolation (`tests/measurement.rs:1187-1191`); stale-lock fixture is a spawned+reaped own child, never a guessed PID (`:1360-1372`) |
| 4 | Secrets management | PASS | No credential-shaped literal anywhere in the S09 slice or evidence |
| 5 | Data protection | N/A | Synthetic sentinels (`synthetic-measurement`, `scope-a/b`); no live data read |
| 6 | Privacy by design | PASS | Fixtures only; no PII; live stores untouched (`fetch-before` 1399/1399, status byte-equal) |
| 7 | Error handling | PASS | Every concurrency wait bounded (1s poll, 10s handshakes); failure closes toward a recorded non-`completed` status, never silent success |
| 8 | Logging | PASS | Logs carry counts, pids of own children, and synthetic payloads only; no secrets |
| 9 | Dependency security | PASS | Manifest unchanged (`Cargo.toml 36926d5d…`, `Cargo.lock 5f57daf9…`), Turso `=0.8.3`; no new crate, no new CVE surface |
| 10 | Secure configuration | PASS | No config/env/production surface touched; `CARGO_TARGET_DIR` contained; nothing listens |

Violations: none. Gate verdict: APPROVED (no security finding).

## Verdict

**STATUS: REJECTED.** Three required modifications, all in the S09/S10 slice:

1. Add the missing positive control / elapsed lower bound for the scope-barrier poll (R1) so the `blocked` observation dies when the poll is degenerate; re-run the named test plus the full serial suite.
2. Reconcile `results.json` with executed reality: `online_admission_executed` must become `true` (or be removed), and `unknowns` must drop `admission/drain/stale handles` and `whole-root interrupted restore` or restate them as production-scope limits, matching `limits[]` (R2).
3. Make the pristine-hash story true: regenerate or explicitly supersede `evidence/s09-pristine.sha256`, and correct `handoff-5:20` / `report:49` / `ledger.md:28` (R3). R4/R5 as cheap cleanups in the same pass.

Both rows go same-row `REWORK` (never replay), `WAVE END` cleared to `-` with a retained `WAVE HISTORY` reopening record; the wave **cannot be signed off as closed** in its current state. Scratch-only, baseline-intact, Phase C blocked, INCONCLUSIVE ruling and the policy-only status of recommendation A are all confirmed and must survive the rework.
