# Review 6 — review-5 rework (R1/R2/R3 + R4/R5), independent

REVIEW: `/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/20261111-0252-shard-measurement.review-6.md`
STATUS: APPROVED

BLUEPRINT: `.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md` — stamp recomputed by this reviewer:
`sed '1,/^---$/d' … | sha256sum | cut -c1-12` = `d90461a51f51`, matches STAMP `SYS-APPROVED 2026-11-11T03:13Z`.
Full-file sha256 `0382694d525b113d854f92f23c5df0c35a49f1dedb1a80b1bd9c35a3b822c1fa` (unchanged from review-5).
LEDGER read in full (`20261111-0252-shard-measurement.ledger.md`, 35 lines). HANDOFF-6 read in full. S08 not
re-litigated; regression checked by suite re-execution and by hash, not by re-review. HEAD `df34c52`.

## Re-derived by this reviewer (not inherited)

| Check | Result | Artifact (mine) |
|---|---|---|
| Named S09 test, exact scratch command, pinned target | exit **0**, `1 passed` (1.05s) | shell output, this session |
| Full serial suite `-- --test-threads=1` | exit **0**, `10 passed; 0 failed` (2.59s) | `.tmp/scratch/review6-suite2.log` |
| Matrix recounted from my own fresh run `runs/s09-admission-1794379269670402719` | 15 paths = `completed 12 / refused 2 / blocked 1`, `not_exercised 5`; timings poll `1001537us`, control `25us`; no `contender_blocked_poll_1s` key | same run dir |
| `rustfmt --edition 2024 --check` (3 files) | exit 0 | inline |
| `cargo clippy --all-targets -- -D warnings` (scratch manifest, offline) | exit 0 | `.tmp/scratch/review6-clippy.log` |
| `check-settlement.py` from R | `settlement: PASS`, `EXITCODE:0` | shell output |
| Ledger shape, my own run incl. `LEDGER_CLOSE=1` | `ledger-shape: PASS`, exit 0 | shell output |
| Baseline + status | `sha256sum -c evidence/fetch-before.sha256` → **1399 OK, 0 failures**; `git status --porcelain` byte-equal to `fetch-before.status`; scratch `Cargo.toml 36926d5d…`, `Cargo.lock 5f57daf9…` | `.tmp/scratch/review6.status` |
| `evidence/admission.json.phases` vs its declared `observation_source` run artifact | byte-equal (JSON-equal); source `runs/s09-admission-1794378746376652797` exists | inline python |

## R1 — the `blocked` row is now falsifiable

Code: `tests/measurement.rs:1126` `S09_SCOPE_POLL=1s`; `:1511-1517` negative poll + `is_err`; `:1520-1526`
`try_recv()==Empty`; `:1529-1532` live lower bound `>= S09_SCOPE_POLL/2`; `:1552-1572` same-key positive
control; `:1577-1578` persisted µs; `:1696-1708` persisted gates (`>= 500_000us` + control present).
The old bool `contender_blocked_poll_1s` is gone from the artifact, and the generator (`update-results-rework-5.py:36-38`)
asserts its absence — a bool can no longer carry the row.

**Which assertion kills what** (verified by construction and by my own mutant run):
- Constant mutant M3b (`:1126` `from_secs(1)`→`from_secs(0)`): my run → **RED 101 at `:1699`**,
  `the persisted negative poll must show the wait on the live contender, got 1100us` (`review6-m3b.log`).
  Because assertions are sequential and the panic is at `:1699`, the live bound at `:1529` **passed** — the
  handoff's claim is exact: rule 1b's `S09_SCOPE_POLL/2` scales to 0 with the constant; the absolute persisted
  gate is the killing clause.
- Call-site mutant M3 (review-5's literal mutant, `from_secs(0)` at the call site, constant intact): dies at
  `:1529` (retained `evidence/s09-m3-callsite-after-fix.log:9`); by construction `poll_elapsed≈1ms < 500ms`.
  Not re-run here (mutation budget went to M3b/M1/M2 — see table); the structural argument is unambiguous
  because the threshold is a literal 500 ms and the mutant makes the poll return in ~1 ms.
- Positive control is present on the same key `Scope::User,"scope-a"` (`:1559`), after the drain, and asserts
  completion **and** signal delivery. It is not vacuous: it fails if the uncontended op does not complete in
  the window, and it is jointly discriminating with the negative poll (a no-op `with_scope_write_lock` would
  break `:1515` first). Measured 23–25 µs vs 1.001 s.

**Judgement on the relative bound (asked explicitly):** it is not dead weight, and the disclosure is honest.
No mutant distinguishes `poll_elapsed >= S09_SCOPE_POLL/2` from `poll_us >= 500_000` at the current constant,
so the two encode the same threshold; but they read different trust points — the live measurement vs the
persisted artifact — so the live bound still catches a fabricated persisted value and fails before the test
writes evidence. Keeping both is defensible; deleting the live one would let the test persist a vacuous row
before failing. No change required.

## R2 — settlement is derived and machine-gated

- Top-level `restore_executed` / `online_admission_executed` are gone; the live keys are `['status','ruling',
  'phase_c','fetch_exit','offline_metadata_exit','locked_turso_version','lock_sha256','suite_exit','tests_passed',
  'clippy_exit','raw_copy_evidence','raw_copy_duration_ms','schema_columns','unknowns','limits','rework','s08',
  's09','ruling_basis','rustfmt_check_exit','final_tree_sha256','s10','rework_5']` — no `*_executed`.
- `unknowns[]` carries 3 `production: ` + 5 `not_exercised: ` entries; the latter are generated at
  `update-results-rework-5.py:86-90` from `artifact["not_exercised"]` (the same list `admission.phases` copies),
  not retyped. Verified byte-equal to `admission.phases.not_exercised[].path`.
- `check-settlement.py` implements all seven debug-2 2c assertions, each present in code and in the log:
  1 green + no `*_executed` (`:34-43`); 2 scoped unknowns (`:45-48`); 3 not_exercised equality (`:50-58`) and
  counts (`:59-64`); 4 live tree recomputation with `hashlib` (`:66-74`); 5 own `sha256sum -c` + manifest
  mapping (`:76-101`); 6 executed-limit backing (`:103-116`); 7 `settlement: PASS` (`:123`).
- **Non-vacuity, re-probed by me** on a disposable root `.tmp/scratch/review6-settlement` (copied evidence +
  three source files, canonical evidence untouched): baseline `PASS` rc 0; reintroduced
  `online_admission_executed` → rc 1 at check 1; dropped `power-loss-durability` from `unknowns` → rc 1 at
  check 3; `s09.green_exit=101` **and** a byte appended to `tests/measurement.rs` → rc 1 at checks 1, 4, 5 and 6
  (`probeA/B/C.log`). The coder's 7/7 claim is consistent with my 3/3 sample.

## R3 — pristine manifest is post-format and self-verifying

`evidence/s09-pristine.sha256` lists three files; `sha256sum -c` → `src/lib.rs: OK`, `src/main.rs: OK`,
`tests/measurement.rs: OK`, `EXITCODE:0` (`evidence/s09-pristine-check.log`). It maps exactly to
`results.json.final_tree_sha256` (`e755acc5… / 75f3d936… / 951c9a92…`). MTimes confirm the ordering requirement:
`src/lib.rs` 13:06:10+07, `tests/measurement.rs` 13:30:36+07, manifest **13:32:31+07** — the snapshot post-dates
the last source edit. Supersession is recorded (`results.json.s09.pristine_snapshot_superseded`) with the old
values retained, not erased; `handoff-5:20` is superseded in handoff-6, `report` carries the corrected claim, and
`ledger.md:28` is intact with the correction appended at `ledger.md:33/35`.

## R4 / R5

- R4: `evidence/s09-final-gates.log` carries literal, expanded codes `FORMATCODE:0 / CLIPPYCODE:0 /
  NAMEDITCODE:0 / SUITECODE:0`; post-fix logs carry literal `EXITCODE:0` (`s09-green-after-fix.log:12`,
  `s10-suite-after-fix.log:20`, mutant logs `EXITCODE:101`). The old mangled `EXIT:$?` survives only in the two
  retained historical logs, disclosed in `commands.log`. Satisfied (named codes instead of the literal token
  `EXITCODE:` in the gate burst — functionally equivalent, unambiguous, documented).
- R5: S09's row carries the authorization/release note in its rework proof (`d90461a51f51 SYS-APPROVED
  2026-11-11T03:13Z per debug-2 prescription items 1 and 3`), matching the S08 precedent. The S10 row's history
  now holds two *different* BLOCKED clauses (S08 then S02) — a valid chain, not the repeated identical block
  review-5 flagged. Same-row DONE→REWORK→DONE with prior cycles retained; one row per S01–S10; no replayed DONE
  row; shape PASS at every write and at close (`LEDGERCLOSECODE:0`).

## S08 regression (not re-litigated, checked not weakened)

`src/lib.rs` (`e755acc5…`) and `src/main.rs` (`75f3d936…`) are byte-identical to the review-5 state; the test file
diff against the retained review-5 mutant copies shows only the S09 fix region plus each mutant's own line.
The S08 test is untouched and passes in my 10/10 suite run.

## Mutation spot-check (3/3 used, scratch copy `.tmp/scratch/rework5-mutants`, own warm target, offline+locked; live tree never mutated; canonical test file restored and hash-verified `951c9a92…` afterwards)

| # | Mutant | Result | Log |
|---|---|---|---|
| 1 | `S09_SCOPE_POLL` `from_secs(1)`→`from_secs(0)` (`:1126`) | **RED 101** at `:1699` persisted gate (live bound at `:1529` passed) | `.tmp/scratch/review6-m3b.log` |
| 2 | `s09_latched_op` drops `release.await` (`:1143`) | **RED 101** at `:1514` `same-scope contender must not complete while the holder is latched` | `.tmp/scratch/review6-m1.log` |
| 3 | `s09_classify` `"refused" => "completed"` (`:1227`) | **RED 101** at `:1669` `hardcoded all-blocked/all-completed ledgers cannot pass` (left `completed`) | `.tmp/scratch/review6-m2.log` |

No survivor among the three. The review-5 R1 vacuous-oracle family does not recur on this row.

## Security Gate (software-security-reviewer, 10 Mandates)

Scope: scratch test harness (`tests/measurement.rs` S09 region), three evidence scripts, `results.json` /
`admission.json` / pristine manifest, ledger and report prose. No production or tracked file touched.

| # | Mandate | Verdict | Evidence |
|---|---|---|---|
| 1 | Authentication | N/A | Offline measurement harness; no auth surface |
| 2 | Authorization / IDOR | N/A | Single-user scratch root; no multi-tenant resource |
| 3 | Input validation | PASS | Run roots derived under the pinned `RUNS` dir; aux writes use bound `Value::Text` params; dead-PID fixture is a reaped own child, never a guessed PID |
| 4 | Secrets management | PASS | No credential-shaped literal in the diff or evidence |
| 5 | Data protection (TLS/at rest) | N/A | Synthetic sentinels only; no live store opened |
| 6 | Privacy by design | PASS | No PII; 1399/1399 tracked hashes and status byte-equal |
| 7 | Error handling | PASS | Every wait bounded (1s poll, 10s latch/drain handshakes); failure closes to a recorded non-`completed` status |
| 8 | Logging hygiene | PASS | Counts, own-child PIDs and synthetic payloads only |
| 9 | Dependency security | PASS | `Cargo.toml`/`Cargo.lock` unchanged (`36926d5d…`/`5f57daf9…`), Turso `=0.8.3`, no new crate |
| 10 | Secure configuration | PASS | No config/env/production surface; `CARGO_TARGET_DIR` contained; nothing listens |

Violations: none. Gate verdict: APPROVED (no security finding).

## Boundaries

Scratch-only; no tracked, live, config, vault or service change; 1399/1399 baseline OK; `git status --porcelain`
byte-equal; scratch manifests unchanged; no cleanup (runs, mutant copies and prior evidence retained);
`ruling: INCONCLUSIVE`; recommendation A labelled policy-only; `phase_c: blocked`. My own review side effects are
confined to scratch: new `runs/s09-admission-1794379269…` dirs (from the required re-runs) and
`.tmp/scratch/review6-*.log`, `.tmp/scratch/review6-settlement/`, `.tmp/scratch/review6.status`.

## Advisories (non-blocking; all currently consistent, none contradicting a measured fact)

1. `results.json:157-163` `s09.not_exercised` is still a hand-maintained copy of
   `admission.phases.not_exercised[].path`. `check-settlement.py` gates the top-level `unknowns` but not this
   field, and neither generator rewrites it (`update-results-rework-5.py` touches `s09.measured_outcomes`,
   `scope_poll_evidence`, `key_observations`, `logs` — not `not_exercised`). It agrees today; a future S09 run
   with a different traced-path set would leave one file with two disagreeing lists. Cheapest close: delete the
   field (it duplicates the `not_exercised:` entries in `unknowns`) or add one checker line
   `sorted(results['s09']['not_exercised']) == actual`.
2. `results.json:133` `s08.pristine_snapshot_superseded` refers to `s10.final_tree_sha256`, which does not exist;
   the hashes live at top-level `final_tree_sha256` (`:215-219`). Stale pointer, no factual harm.
3. `ledger.md:33` says the review-5 rework was "reopened 2026-11-11T06:35:28Z"; the actual reopen is
   `06:29:46Z` (`ledger.md:31`, handoff-6:65). Other timestamps in that appended record are correct.
4. `handoff-6:28` cites `:1513` for the M1 assert; the canonical file has it at `:1514` (the retained log was
   generated from the pre-final-revision file; my re-run on the final byte-exact file reproduces the same
   observable at `:1514`).
5. Gate scope, disclosed: `check-settlement.py` compares two generator outputs (counts) and derived-vs-artifact
   (paths), so it catches manual drift and artifact disagreement, not a bug inside the generator itself; the
   coder states the same residual in handoff-6:74.

## Verdict

**STATUS: APPROVED.** R1 is closed — the `blocked` row now dies on a degenerate window (both the call-site and
the constant mutant), the pristine tree stays GREEN, and the positive same-key control is real. R2 and R3 are
closed and machine-gated; the settlement checker refuses corruption on my own disposable probes. R4/R5 are
applied without weakening history, S08 is byte-unchanged, and every boundary holds. The five items above are
advisories, not conditions: none of them makes any claim in the deliverable false or any evidence
irreproducible.
