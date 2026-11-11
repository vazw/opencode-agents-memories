# Handoff

BLUEPRINT: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md
STAMP: sha256=d90461a51f51 (SYS-APPROVED 2026-11-11T03:13Z)
BLUEPRINT-ACK: sha256=d90461a51f51
LEDGER=/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.ledger.md
PRIOR HANDOFF: .tmp/progress/20261111-0252-shard-measurement.handoff-5.md (retained verbatim; its pristine-hash claim is superseded by R3 below)
FEEDBACK: .tmp/progress/20261111-0252-shard-measurement.review-5.md (REJECTED) + .tmp/progress/20261111-0252-shard-measurement.debug-2.md (FIX PRESCRIPTION items 1-5)
SCOPE: review-5 rework of S09/S10 only. Wave CLOSED again: S01-S10 DONE, WAVE END 2026-11-11T06:35:49Z, ledger shape PASS including a `LEDGER_CLOSE=1` run (`evidence/ledger-close-check.log`, exit 0). No cleanup performed.

## Test Evidence

Command template (offline+locked, scratch manifest, contained target):

```sh
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR \
  CARGO_TARGET_DIR=<scratch>/target cargo test --offline --locked \
  --manifest-path <scratch>/Cargo.toml --test measurement <TEST> -- --exact --nocapture
```

Full gate: same prefix, `--test measurement -- --test-threads=1 --nocapture`.

| Step | Exit | Distinguishing observable | Artifact |
|---|---|---|---|
| R1 pre-fix reproduction (M3 on the retained superseded copy `.tmp/scratch/review5-m3`) | 0 | `1 passed` — the vacuous oracle survives `from_secs(0)`, reproducing review-5 R1 today | `evidence/s09-m3-before-fix.log` |
| R1 RED (M3a: call-site window `0s`, constant intact) | 101 | `negative poll returned after 1.081608ms; a degenerate window proves nothing` — the live elapsed lower bound (`:1529`) | `evidence/s09-m3-callsite-after-fix.log` |
| R1 RED (M3b: `S09_SCOPE_POLL` `from_secs(1)`→`from_secs(0)`) | 101 | `the persisted negative poll must show the wait on the live contender, got 1081us` — the **persisted** absolute gate (`>= 500000us`), not the live bound: the relative bound `window/2` scales with the mutated constant | `evidence/s09-m3-after-fix.log` |
| R1 RED (M1: `s09_latched_op` drops `release.await`) | 101 | `same-scope contender must not complete while the holder is latched` (`:1513`) | `evidence/s09-m1-after-fix.log` |
| R1 RED (M2: `s09_classify` `"refused" => "completed"`) | 101 | `assertion left == right failed: hardcoded all-blocked/all-completed ledgers cannot pass` (left `completed`, right `refused`) | `evidence/s09-m2-after-fix.log` |
| R1 GREEN (pristine, post-fix) | 0 | `1 passed` in 1.04s — the 1s poll is really waited; measured `scope_same_scope_negative_poll_us = 1001269` against `scope_positive_control_us = 23` | `evidence/s09-green-after-fix.log`, `evidence/admission.json` |
| Full serial suite (post-fix) | 0 | `10 passed; 0 failed` (2.72s), `-- --test-threads=1` | `evidence/s10-suite-after-fix.log` |
| Gate burst (rustfmt --check / clippy `-D warnings` / named test / serial suite) | 0/0/0/0 | literal `FORMATCODE:0 CLIPPYCODE:0 NAMEDITCODE:0 SUITECODE:0` (1.05s, 2.59s) — closes R4 | `evidence/s09-final-gates.log` |
| R3 pristine manifest (three files, post-format) | 0 | `sha256sum -c` → `src/lib.rs: OK`, `src/main.rs: OK`, `tests/measurement.rs: OK` | `evidence/s09-pristine-check.log` |
| R2 settlement gate | 0 | `settlement: PASS`, all seven checks PASS, live hashes recomputed, own `sha256sum -c` | `evidence/s10-settlement.log` |
| R2 checker non-vacuity (disposable root `.tmp/scratch/settlement-probe`) | 0 then 1×7 | baseline PASS; reintroduced `*_executed`, unscoped unknown, missing `not_exercised` path, live tree byte drift, stale manifest, non-green executed limit, disagreeing counts each refused | `evidence/s10-settlement-probe.log` |
| Immutable baseline (after the rework) | 0 | `sha256sum -c evidence/fetch-before.sha256` → 1399/1399 OK; `git status --porcelain` byte-equal to `fetch-before.status`; scratch `Cargo.toml` `36926d5d…`, `Cargo.lock` `5f57daf9…` unchanged | `evidence/fetch-after.status`, `commands.log` |

All mutants ran on `.tmp/scratch/rework5-mutants` (copy of `review5-m3` including its target, canonical `src/`+`tests/` overlaid, own `CARGO_TARGET_DIR`, offline+locked); the live tree was never mutated and the copy was restored from canonical after every run. Both `src/lib.rs` and `src/main.rs` are byte-identical to the review-5 state (`e755acc5…`, `75f3d936…`); only `tests/measurement.rs` changed (`37a5bca6…` → `951c9a92…`).

## R1 — the `blocked` row is measured, not asserted

`S09_SCOPE_POLL = 1s` (one constant, shared by the negative poll and the positive control). The negative poll now records `Instant::now()` around `timeout(S09_SCOPE_POLL, &mut contender_rx)`, keeps `poll.is_err()`, adds the structural check `contender_rx.try_recv() == Err(TryRecvError::Empty)` (queued, sender alive), and lower-bounds the wait at `poll_elapsed >= S09_SCOPE_POLL / 2` (a monotone timer cannot fire early, so a lower bound is load-safe). The uncontended same-key control (`Scope::User`, `scope-a`, no holder) runs inside the same window after the drain and asserts both completion and signal delivery. Both are persisted as `scope_same_scope_negative_poll_us` / `scope_positive_control_us`; the old `contender_blocked_poll_1s: bool` is gone from `admission.json`. A persisted-artifact gate requires `>= 500000us` on the negative poll and presence of the control duration.

Disclosures a reviewer should weigh:
- The local `contender_blocked = true` from rule 1b was dropped, not kept: rule 1d removed its only reader, so keeping it would be an unused-variable error under `-D warnings`. Rule 1b's assertion sequence is otherwise verbatim.
- Rule 1b's bound is **relative** to `S09_SCOPE_POLL`, so a mutant of the constant passes it. Rule 1e is what kills that mutant; the measured evidence shows exactly that split (M3a dies at the live bound, M3b at the persisted gate). An absolute 500000us live bound would have duplicated 1e.
- The positive control alone cannot kill M3 (tokio's `Timeout` polls the inner future before the delay); it gives the row its contrast — measured 23µs completed vs 1001269µs queued.

## R2 — settlement is derived and machine-gated

`results.json`: top-level `restore_executed` and `online_admission_executed` deleted; `unknowns` regenerated from `admission.json.phases.not_exercised[].path` with `production:` (3) / `not_exercised:` (5) prefixes. Derivation is reproducible: `evidence/update-results-rework-5.py` (rebuilds `evidence/admission.json` from the newest green run `runs/s09-admission-1794378746376652797` and rewrites the derived fields; the older `update-results-rework.py`, which wrote the drifted `unknowns`, is superseded by it and left in place as history).

`evidence/check-settlement.py` implements debug-2 item 2c in full: (1) `s08`/`s09` `green_exit == 0` and no top-level `*_executed`; (2) every `unknowns[]` scoped; (3) `not_exercised:` entries == `admission.phases.not_exercised[].path` and `s09.measured_outcomes == admission.status_counts` (now including `not_exercised` = entry count); (4) `final_tree_sha256` recomputed with `hashlib` from the live files; (5) its own `sha256sum -c` exit 0 **and** manifest mapping == `final_tree_sha256`; (6) every `limits[]` starting `S0x executed` has a matching block with `green_exit == 0`; (7) `settlement: PASS`. Result: `evidence/s10-settlement.log`, `EXITCODE:0`. The checker was then run against seven disposable corruptions (plus a clean baseline) and refused each with exit 1 — the gate is not vacuous.

## R3 — pristine manifest and the retained false claim

`evidence/s09-pristine.sha256` was regenerated **after** the final source edit (rustfmt write pass, then `--check`), now covering all three files and matching `results.json.final_tree_sha256`; `sha256sum -c` exit 0. The superseded 06:05:49Z two-file snapshot, taken before the `src/lib.rs` 06:06:10Z rustfmt write and the `tests/measurement.rs` 06:06:29Z `bool_assert_comparison` fixes, is recorded as `results.json.s09.pristine_snapshot_superseded` with its values (tests `a745c97d…`, lib `fe603303…`) rather than erased. `report:49` corrected; `handoff-5:20` superseded here; `ledger.md:28` left intact as the historical claim with an appended correcting `WAVE HISTORY` record.

## R4/R5 cleanups

R4: `evidence/s09-final-gates.log` re-captured as a single gate burst with literal `FORMATCODE`/`CLIPPYCODE`/`NAMEDITCODE`/`SUITECODE` codes, and the post-fix clippy/suite logs carry literal `EXITCODE:` lines; the two retained historical logs `s09-green.log` and `s09-red.log` still contain the old mangled `EXIT:$?` echo and are left as history (disclosed in `commands.log`), superseded by `s09-green-after-fix.log`. R5: the S09 row carries the explicit authorization/release note in its rework proof (`debug-2 ... authorized under d90461a51f51 SYS-APPROVED 2026-11-11T03:13Z`), and the duplicated S02 dependency-blocker text was collapsed to one occurrence in the S10 history (each other row keeps its single copy).

## Ledger state

Reopened 2026-11-11T06:29:46Z (prior `WAVE END 2026-11-11T06:12:23Z` retained in `WAVE HISTORY`). S09 moved DONE→REWORK (06:29:51Z) then REWORK→DONE (06:34:58Z); S10 moved DONE→REWORK (06:35:05Z) then REWORK→DONE (06:35:36Z) — same rows, prior cycles retained in `history`, no replayed DONE row. Shape checker PASS after every write (non-close), and PASS with `LEDGER_CLOSE=1` at `WAVE END 2026-11-11T06:35:49Z`.

## Constraints held

Scratch-only; `Cargo.toml`/`Cargo.lock`/tracked/live/config/vault untouched (baseline 1399/1399 and `git status` byte-equal after the rework); no network, no new dependency; `runs/`, mutant copies and prior evidence retained; `ruling: INCONCLUSIVE`, recommendation A policy-only, `phase_c: blocked` unchanged; S08 not re-litigated.

## Residual risks

- The scope-barrier result still says nothing about the 5 `not_exercised` admission paths or about a bypass; R1 makes the observation falsifiable, it does not widen coverage.
- `admission.json.phases` is a copy of the newest green run artifact, so it is only as current as that run (`runs/s09-admission-1794378746376652797`, the suite's last S09 execution). `check-settlement.py` cross-checks the counts and the `not_exercised` paths, not the per-path details.

STATUS: READY-FOR-REVISION-REVIEW — review-5 R1/R2/R3 closed with mutant-discriminating evidence, R4/R5 cleanups applied, settlement machine-gated (`settlement: PASS`, exit 0), wave re-closed with history preserved.
HANDOFF: /home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.handoff-6.md
