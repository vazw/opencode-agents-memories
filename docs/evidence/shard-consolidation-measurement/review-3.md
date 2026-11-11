# Independent review 3 — rework verification (blueprint d90461a51f51)

Scope: coder rework for review-2's four rejection items (debug-1 prescription items 1–4), against
`blueprint-20261111-0310-shard-measurement.md` (sha256 d90461a51f51, SYS-APPROVED 2026-11-11T03:13Z).
Every load-bearing claim below was re-executed by this review, not inherited. S08–S10 were not claimed
and remain BLOCKED; the wave ruling stays INCONCLUSIVE regardless of this verdict.

## Prescription items — independently re-executed (MEASURED, this review)

1. **S03 error-payload containment assertion — VERIFIED.** `tests/measurement.rs:43-54` pins the
   distinguishing observable (`assert_eq!` of `MeasurementError::Containment` message against
   `root.display()`), per debug-1 finding 5 and the falsified-route memory (is_err-only is measured
   vacuous). My own guard mutant v2 (BOTH pre-guard existence clauses `lib.rs:44-45` removed, fresh
   compile 1m10s, isolated target): **RED exit 101**, `left: "File exists (os error 17)"` vs
   `right: <root path>` (`evidence/review-3-fresh-guardv2.log`). Pristine serial suite GREEN (below).
2. **S04 populated Observation — VERIFIED.** `run_case` builds `schema`/`duration_ms`/`artifact_paths`
   from the raw-copy artifact (`src/lib.rs:70-90`); `test_raw_blob_and_all_columns_roundtrip`
   cross-checks `observation.schema == result["schema_columns"]`, `duration_ms > 0`, artifact files
   exist (`tests/measurement.rs:69-84`). Cited RED (`rework-observation-red.log`: `left: Null` vs
   19-column array) is the genuine zero-Observation failure; coverage-set equality
   (`oracle_columns == schema_columns`, measurement.rs:86) and 4 negative controls retained.
3. **S05 dedicated Case experiments with persisted manifests — VERIFIED.** `Case::{ExactDuplicates,
   DivergentIds}` routed through `run_case` (`lib.rs:91-105`, `duplicate_case`); `duplicates.json`/
   `divergent.json` persisted; two-surface assertion `dup.duplicate_occurrences == persisted` plus
   3-unique/5-occurrence shape (`tests/measurement.rs:484-518`). Cited RED is a real compile-RED
   (E0599 on both missing variants, `rework-dedicated-case-red.log:2-11`).
4. **S07 reopened-persisted-state cross-db oracle — VERIFIED.** `verify_persisted` (`src/lib.rs:1170`)
   reopens prompts/skills/metadata/episodic DBs after handle drop and typed-compares every cell
   (`typed`/`untyped`, blobs as bytes, order-insensitive) against `mapped_rows`, plus binding
   predicates (no `/home/vaz/` residual; `skills.container_tag`; `shards.scope_hash/db_path`).
   Routed `Case::CrossDb` via `run_case` (`lib.rs:106-124`, `cross_db_case` at 1275). My own remap
   mutant (DELETE+INSERT block `lib.rs:823-837` deleted, fresh compile, isolated target): **RED exit
   101**, `Engine("persisted mismatch agent_sessions")` (`evidence/review-3-fresh-remap.log`) — the
   reopened persisted state is the killer, exactly as debug-1 required. Refusal SHA-256 manifests via
   system `sha256sum` (`closed_root_manifest`, lib.rs:1128) with independent hash recomputation
   asserted (`tests/measurement.rs:554-574`).

**Equivalent-mutant claim (exists()-only clause): SUPPORTED — not a rejection.** The claim is
supported by cited measurement AND independently re-derived here: (a) cited: `commands.log:24`,
shell `sh_l3ymnio6` run 1, exit 0; (b) mine: exists()-only deletion in a clean isolated build →
**exit 0, 1 passed** (`evidence/review-3-fresh2-guardv1.log`); (c) code argument verified: at
`src/lib.rs:44-45`, `root.exists()` ⇒ `root.symlink_metadata().is_ok()`, so the clause is logically
subsumed and behaviorally dead — an equivalent mutant, correctly DISCARDED (not counted as a
surviving mutant) with the killing mutant v2 substituted. This satisfies the review contract's
equivalent-mutant clause.

## Mutation spot-checks (3 total, at cap; scratch copies `shard-review3-{remap,guardv2,guardv1}`, isolated targets; live workspace untouched)

| Mutant | Pinned behavior | Result |
|---|---|---|
| auxiliary remap persistence block deleted | reopened persisted state equals mapped manifest | RED — `persisted mismatch agent_sessions` |
| both existence clauses removed (`lib.rs:44-45`) | guard emits root-path payload for existing empty root | RED — payload `"File exists (os error 17)"` vs root path |
| `root.exists()` only removed | (equivalence probe) | GREEN, 1 passed — equivalent mutant, discarded |

## Gates re-executed (MEASURED, this review)

- Full serial suite, exact blueprint command/prefix, offline+locked, contained scratch target:
  **exit 0, 8 passed, 0 failed, 1.17s** (`evidence/review-3-suite-raw.log`). The five S03–S07
  blueprint-named tests all present with exact names; additions (`test_dedicated_case_experiments_persist_observations`,
  `test_cross_db_run_case_reopens_and_verifies_persisted_state`, plus the S08-interface-only
  `test_owned_child_refuses_outside_root`) are disclosed additions, no rename/drop.
- Clippy `--offline --locked --all-targets -- -D warnings`: **exit 0** (`evidence/review-3-clippy.log`).
- `rustfmt --edition 2024 --check` lib/main/tests: **exit 0**.
- Immutability re-derived: `sha256sum -c evidence/fetch-before.sha256` → **1399/1399 OK, 0 failures**;
  `git status --porcelain` byte-equal to `fetch-before.status` (same 3 pre-existing tracked edits:
  `.cargo/config.toml`, `AGENTS.md`, `crates/mem-web/src/api/deduplication/mod.rs`);
  `tracked-before.sha256` 3/3 OK; scratch `Cargo.lock` sha256 =
  `5f57daf973586cc12fd62f5d58118ff3165378b791f7e1459455170e51f2e88a` (matches recorded).
- Review-2 mutant copies untouched: `shard-review-mutant-2/-3` src/tests mtimes all predate the
  rework start (03:39–03:59Z < 04:12:20Z); retained mutations byte-faithful (mutant-2: remap block
  absent; mutant-3: exists-only deletion present, remap also deleted per debug-1).
- Ledger: blueprint shape checker **PASS** with `LEDGER_CLOSE=1` under my own execution. S03/S04/S05/
  S07 each transitioned in the SAME row REWORK → DONE with prior timestamps/proof retained in
  `history` and written rework authorization (review-2 finding + debug-1 item); no second status row
  appended, no DONE replayed; WAVE HISTORY retains the prior end verbatim; sequential non-overlapping
  timestamps 04:12:20Z→04:37:32Z.
- S08/S09/S10 remain `[!]` BLOCKED (syseng-owned interruption/restore, admission, report) — correct;
  no restore/admission artifacts exist; `results.json` still `restore_executed: false`,
  `online_admission_executed: false`, `ruling: INCONCLUSIVE`; report retains no A/B winner and
  Phase C blocked.
- No tracked/live/config/vault change: 1399-hash + status proof above; harness opens only synthetic
  scratch DBs; `/home/opencode/synthetic-missing-repo` asserted absent (measurement.rs:429).

## Deviations adjudicated (disclosed, non-silent — accepted)

- debug-1 items 3/4 suggested appending the routed assertions inside the blueprint-named tests; the
  coder added sibling tests instead and kept the named tests intact. The handoff discloses this
  ("two tests added; no test renamed"); every named test still exists, still passes, and its
  original assertions are unchanged, while the new oracles strengthen coverage. No blueprint test
  was silently renamed or dropped.
- Honest failure disclosure verified: transient suite regression (`case not wired` arm) and two
  clippy fixes are logged in `commands.log:29` and report line 45; final states green.

## Review-process disclosure (this review's own footprint)

- Stale-artifact hazard encountered and resolved: my first three mutant runs reused coder-era build
  artifacts in shared target dirs (no `Compiling` line, identical test-binary hash) and produced one
  false RED for the exists()-only probe; re-run per-mutant in fresh isolated targets settled all
  three verdicts above. Logs retained: `evidence/review-3-fresh{,2}-*.log` (+ superseded
  `review-3-mutant-*.log`). Future mutant runs in this repo must use one fresh `CARGO_TARGET_DIR`
  per mutant (cargo 1.99 reused fingerprints across same-name/version package copies created within
  the same second).
- Review scratch retained for orchestrator inspection: `shard-review3-{remap,guardv2,guardv1}` (mutant
  sources) + `shard-review3-target{,-v1,-remap}` (≈13 GB regenerable build artifacts; `rm -rf` was
  policy-blocked for this agent — orchestrator may delete the three target dirs; all evidence lives
  in the logs).

## Security gate (software-security-reviewer, 10 Mandates) — rework delta only

| # | Mandate | Verdict | Evidence |
|---|---|---|---|
| 1 | Authentication | N/A | offline synthetic harness; no auth surface in diff |
| 2 | Authorization/IDOR | N/A | single-user local harness; case-root containment guard substitutes (src/lib.rs:34-61) |
| 3 | Input validation | PASS | absolute/component/prefix/existing/symlink root checks src/lib.rs:38-61; child phase whitelist src/lib.rs:907-909; untraced encodings/tables refused src/lib.rs:567,868 |
| 4 | Secrets management | PASS | no credential-shaped literals in lib.rs/tests/evidence; commands.log records commands/exits only |
| 5 | Data protection | PASS | synthetic sentinels only; no live store opened; fixture encryption deliberately off (src/lib.rs:183-184) |
| 6 | Privacy by design | PASS | `/home/vaz/...` fixture literals confined to scratch; `verify_persisted` actively asserts no `/home/vaz/` residual persists (src/lib.rs:1170+) |
| 7 | Error handling | PASS | `MeasurementError` enum; `verify_persisted` returns per-table Err, no silent skips; child/main exit 1 on error (src/main.rs:18-21) |
| 8 | Logging | PASS | only flushed phase marker from child; evidence logs carry commands/exits, no tokens |
| 9 | Dependency security | PASS | lock unchanged since review-2's `cargo audit` (0 advisories, 339 deps; `evidence/review-2-audit.log`); lock hash re-verified this review; no new dependency — hashing via system `sha256sum` (src/lib.rs:1128) |
| 10 | Secure configuration | PASS | manifests/locks immutable (1399/1399 re-verified); contained offline target; `sha256sum` invoked as fixed single-argv, no shell interpolation; no service/debug endpoints added |

Zero VIOLATION — security gate passes.

## Per-row disposition

- S01, S02 DONE: accepted (carried from review-2; baseline re-verified this review).
- S03 DONE (rework cycle 1): **accepted** — payload assertion pins the guard; killing mutant v2 RED,
  equivalent mutant v1 measured and documented, both re-derived here.
- S04 DONE (rework cycle 1): **accepted** — Observation populated and cross-checked against the
  artifact from two surfaces.
- S05 DONE (rework cycle 1): **accepted** — dedicated Cases through `run_case` with persisted
  manifests and two-surface equality.
- S06 DONE: accepted (carried from review-2; suite green, no regression).
- S07 DONE (rework cycle 1): **accepted** — reopened persisted-state oracle kills the remap mutant
  (re-derived); refusal manifests retained with independent hash recomputation.
- S08/S09/S10 BLOCKED: **correct** — syseng-owned; no execution claims made.

## Standing limits (unchanged)

Full A/B remains **INCONCLUSIVE**: no executed interruption/restore (S08), no admission observation
(S09), no completed final gate (S10); `ai-sessions.db` remains UNKNOWN; facts/vault refusal is a
synthetic sentinel guard, not production support. Phase C remains blocked. Nothing here authorizes
live operations, production design selection, or cleanup of the scratch measurement before
orchestrator acceptance.
