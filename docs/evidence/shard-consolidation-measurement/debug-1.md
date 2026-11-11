# debug-1 — S04/S05/S07 rework diagnosis (blueprint `d90461a51f51`)

Scope: diagnose the two reviewer-rejected mutation survivors (S07 auxiliary remap, S03/S07 containment guard)
plus the unpopulated `Observation` (S04) and missing dedicated duplicate `Case` (S05), against
`blueprint-20261111-0310-shard-measurement.md` body sha256 `d90461a51f51` (SYS-APPROVED 2026-11-11T03:13Z).
Inputs read: `review-2.md`, `handoff-2.md`, ledger `20261111-0252`, harness `src/lib.rs`,
`tests/measurement.rs`, `evidence/results.json`. No implementation performed; no production/tracked/live change.

Labels: MEASURED = re-derived this session; READ = source/artifact read; INFERRED = reasoned, not run.

## [REPRO]

Environment: `R=/home/opencode/Code/Personal/Rust/opencode-agents-memory`; offline + locked;
`CARGO_TARGET_DIR` isolated per run; no fetch, no root manifest/lock edit. Mutant copies
`.tmp/scratch/shard-review-mutant-2/-3` carry byte-identical `Cargo.toml`/`Cargo.lock` to the pristine
harness (`cmp` OK; scratch lock sha256 `5f57daf973586cc1…`).

Baseline (MEASURED, `evidence/debug-1-pristine-suite.log`):
```
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR \
  CARGO_TARGET_DIR=$R/.tmp/scratch/shard-measurement/target \
  cargo test --offline --locked --manifest-path $R/.tmp/scratch/shard-measurement/Cargo.toml \
  --test measurement -- --test-threads=1 --nocapture
→ exit 0, 6 passed
```

Survivor S07 (MEASURED, `evidence/debug-1-mutant2-crossdb.log`): mutant 2 = deletion of the auxiliary
remap persistence block (`lib.rs:736-750`, `DELETE FROM {table}` + mapped-row `INSERT`s). Test
`test_cross_db_mapping_preserves_payload_and_rejects_unknowns` (same command, manifest-path mutant-2,
`CARGO_TARGET_DIR=$R/.tmp/scratch/shard-review-debug-m2/target`, `--exact`) → **exit 0, 1 passed**.
Expected red signal exists: re-running this command must stay RED until the reopen-and-assert oracle lands.

Survivor S03/S07 guard (MEASURED, `evidence/debug-1-mutant3-containment.log`): mutant 3 = removal of
`|| root.exists()` (`lib.rs:41`), plus the remap deletion. Test `test_refuses_outside_or_symlink_root`
→ **exit 0, 1 passed**. Same command shape, target `shard-review-debug-m3/target`.

Diagnosis-delta probe (MEASURED, `evidence/debug-1-mutant3-literal-prescription.log`): review-2 finding-2's
literal prescription ("pre-create an empty directory under `runs/` and assert `run_case` refuses it and
leaves it empty") was applied word-for-word to mutant-3's test and returned **exit 0, 1 passed** — it does
not kill the mutant. Probe edit reverted; mutant copies remain as review-2 left them.

## [MINIMAL CASE]

- S07: `run_case`/`auxiliary_fixture` on one fresh `runs/` case root; persistence block deleted; suite green.
  Eliminated as load-bearing: `remap_identity` unit asserts (pure mapping, no persistence), facts/vault
  sentinel refusals (separate `guard-*` dirs, read-only), the `shards` registry `UPDATE` (re-queried in the
  same open handle at `lib.rs:753-766`, never reopened), `inspect_auxiliary` preservation checks
  (`lib.rs:707-735`, in-memory). Remaining load-bearing element: the oracle never observes a reopened DB.
- S03: one pre-created empty directory under `runs/`, `run_case(Case::Containment, …)`. Eliminated:
  sibling-prefix, traversal and symlink inputs (guard-independent; refusal reproduced without the clause),
  nonempty-reuse input (`tests/measurement.rs:37-41`; `create_dir` AlreadyExists at `lib.rs:59` yields the
  same `is_err`). Remaining load-bearing element: only the error *payload* differs between guard refusal
  (`Containment(root.display())`) and incidental `create_dir` failure (`Containment(io::Error::to_string())`).

## [FINDINGS] (ranked, falsifiable)

1. **CONFIRMED (S07) — self-report oracle, no reopened persisted state.** The named test asserts JSON flags
   the code under test writes itself (`tests/measurement.rs:435-436` → `payload_preserved` /
   `collision_refused`, written at `lib.rs:751`, `lib.rs:767`). Handles are dropped at `lib.rs:768-771`;
   nothing reopens `episodic.db`/`prompts.db`/`skills.db`/`metadata.db`. Prediction: delete the remap writes
   → still green. MEASURED green. Also unproven by the same gap: insertability under real UNIQUE/PK/FK
   constraints (the mapped INSERTs at `lib.rs:740-749` are the only place constraints are exercised, and
   their outcome is never re-read).
2. **CONFIRMED (S03 guard) — existing-root input missing; nonempty-reuse is an equivalent observable.**
   With `root.exists()` removed, every current input still returns `Err` for the same user-visible reason
   (`is_err`, untouched dir). MEASURED green. Prediction: add an empty-existing-root input asserting the
   guard-emitted payload → mutant goes RED. (Error-payload equality is the discriminator, not `is_err` —
   see finding 5.)
3. **CONFIRMED (S04) — `Observation` returned as zero-values.** `run_case` builds
   `schema: Null, row_differences: [], duration_ms: 0, artifact_paths: []` unconditionally
   (`lib.rs:65-75`). `raw_copy` measures all of it (`lib.rs:213`, `lib.rs:402`) but discards it after
   writing `raw-copy.json` and returns `MResult<()>` (`lib.rs:408`). The test asserts only `.status` and
   artifact presence (`tests/measurement.rs:48-50`). READ + `results.json` limits entry agree.
4. **CONFIRMED (S05) — no dedicated duplicate `Case` experiment.** `Case` has only
   `Containment`/`RawCopy` (`lib.rs:3-7`). The S05/S06/S07 experiments call `inspect`/`auxiliary_fixture`
   directly from the tests, so no per-case persisted observation artifact and no populated
   `Observation.duplicate_occurrences` exist (`tests/measurement.rs:127-182`, `:402-406`).
5. **CONFIRMED (prescription correction) — review-2's literal finding-2 fix is itself vacuous.** Measured:
   `is_err + leaves-empty` stays GREEN under mutant 3 (log above), because `create_dir` on an existing dir
   returns the same observable refusal. Prediction: asserting `matches!(err, Containment(m) if m ==
   root.display().to_string())` kills mutant 3; the mutant's payload is the OS error string
   (`Os { code: 17, kind: AlreadyExists, message: "File exists" }`). Must be carried into the coder brief.
6. Falsified: "the S05/S06 inspect() assertions are vacuous." They exercise real typed rows and reverse
   order; they do not cover the persistence/route seam the Case contract requires — scope gap, not a
   vacuous inspect oracle (READ; consistent with review-2 S06 accept).

Class context (memory hits, all-projects): this is the repo's recurring vacuous-oracle family — fixture
written from the correct implementation's inputs, lacking the wrong implementation's distinguishing input
(`mem_1793066631237_8evoph8hb`, `mem_1792550463914_xk0dvwgxd`); mutation must be run in a scratch copy with
its own `CARGO_TARGET_DIR` (same memory; done here).

## [ROOT CAUSE]

CONFIRMED. The measurement harness treats in-process self-reports (JSON manifests/flags, zero-value
`Observation`) as the oracle and omits the two boundary inputs that make guard and persistence observable
(empty-existing root; reopened mapped state). One shared cause across the four rejected rows: the oracle
observes the *correct implementation's own claims* rather than the persisted state or the wrong
implementation's distinguishing input. Not a production defect: the harness contains no production code;
the blueprint's contract fields (reopen/typed comparison/negative controls) were implemented in the
fixture builders but not in the assertions.

## [FIX PRESCRIPTION] (coder; scratch-only)

Ordering (ledger protocol): reopen S03/S04/S05/S07 rows to `[~]`/REWORK with prior proof/history, RED each
new assertion in the shape named below, then implement, GREEN, then the full serial suite. No
manifest/lock/tracked/live change; see constraints at the end.

### 1. S03 — pin the guard at its observable (kills mutant 3)
`tests/measurement.rs::test_refuses_outside_or_symlink_root`, append after the nonempty-reuse block:
```rust
let empty = runs.join(format!("empty-existing-{}", std::process::id()));
std::fs::create_dir(&empty).unwrap();
let err = run_case(Case::Containment, &empty).unwrap_err();
let MeasurementError::Containment(message) = err else {
    panic!("existing empty root must be refused as containment");
};
assert_eq!(
    message,
    empty.display().to_string(),
    "existing empty root must be refused by the existing-root guard, not by create_dir"
);
assert!(empty.is_dir() && std::fs::read_dir(&empty).unwrap().next().is_none());
```
RED proof: run against `.tmp/scratch/shard-review-mutant-3` (isolated target) → must fail on `assert_eq!`
(payload is the OS error string). GREEN on pristine. Do **not** reduce to `is_err` — measured vacuous
(finding 5). Import `MeasurementError`.

### 2. S04 — populate and assert `Observation` (kills the zero-Observation code)
`src/lib.rs`:
- change `async fn raw_copy(root: &Path) -> MResult<()>` (`lib.rs:212`) to return the measured `Json`:
  after writing the artifact (`lib.rs:403-407`) return `Ok(result)` (line 402's value).
- in `run_case` (`lib.rs:60-64`) capture the result and build `Observation` from it: `schema =
  result["schema_columns"]`, `row_differences = Vec::new()` (truthful for a lossless copy — do not invent
  entries), `duration_ms = result["duration_ms"].as_u64().unwrap_or(0) as u128`, `artifact_paths =
  vec![root.join("raw-copy.json")…]`, `limitations` from `result["limitations"]`.
  `duplicate_occurrences`/`barrier_observations` stay `Null` for RawCopy (not measured in this case —
  honest; note S05/S09 in `limitations`).
`tests/measurement.rs::test_raw_blob_and_all_columns_roundtrip` add after line 50:
`assert_eq!(observation.schema, result["schema_columns"])` (Observation cross-checked against the artifact),
`assert!(observation.duration_ms > 0)`, and every `artifact_paths` entry is an existing file.
RED proof: the new asserts fail on the current tree (Observation is zero-value) — that is the RED step; no
mutant copy needed. Do not weaken the existing empty-`row_differences` assertion.

### 3. S05 — dedicated `Case` experiments through `run_case` (fixed-enum contract)
`src/lib.rs`:
- extend `pub enum Case` (`lib.rs:3-7`) with `ExactDuplicates`, `DivergentIds`, `CrossDb` (blueprint's
  fixed enum, not arbitrary input).
- `run_case` match arms: `ExactDuplicates` → build the typed inputs (reuse the existing in-case baseline
  rows, e.g. run the RawCopy build in-case and read its `before.typed_rows`, keeping `inspect()` unchanged),
  call `inspect`, persist `duplicates.json` `{plan_rows, occurrences, duration_ms}`, return `Observation`
  with `duplicate_occurrences = {id: [occurrence sources]}` and `artifact_paths = [duplicates.json]`.
  `DivergentIds` → per-non-association-column divergent run over the 13 columns, persist `divergent.json`
  `{conflicts, work_tuple_hashes_unchanged: true, duration_ms}`.
`tests/measurement.rs::test_identical_ids_collapse_only_after_association_normalization` add: call
`run_case(Case::ExactDuplicates, &case_root)`, assert returned `observation.duplicate_occurrences` (3 unique
ids, occurrences sum 5) equals the existing inline `inspect` expectations, and the artifact is a file whose
`occurrences` equal the Observation (two independent surfaces). Keep every existing inspect assertion.
RED proof: current tree has no variant → test fails to compile/finds no artifact; after adding the variant
the assertion is red until the experiment populates it.

### 4. S07 — reopen-and-assert persisted state (kills mutant 2) + refusal manifests
`src/lib.rs`, `CrossDb` arm of `run_case` (and keep `auxiliary_fixture` unchanged):
- after `auxiliary_fixture` returns (handles already dropped, `lib.rs:768-771`), reopen each closed DB with
  fresh handles: `episodic.db` (agent_sessions, episodic_milestones, episodic_raw_context, episodic_segments,
  capture_inbox), `prompts.db` (user_prompts), `skills.db` (skills), `metadata.db` (shards) — 4 files,
  8 table manifests.
- for each table, `SELECT * FROM {table}`, order-insensitive compare against
  `result["tables"][table]["mapped_rows"]` (typed values, not JSON summaries): binding column equals the
  mapped target (`project_id → /home/opencode/synthetic-missing-repo` or `/home/opencode/Code/opencode`;
  `user_prompts.project_path` likewise; `skills.container_tag → synthetic_project_target` for a/b rows and
  `NULL` preserved for the NULL row; unrelated `/unrelated/sibling` untouched), and every non-binding payload
  value byte-equal (blobs as bytes, NULL as NULL). `shards`: `scope_hash='explicit-git-common-target'`,
  `db_path='projects/explicit-target.db'` (as written at `lib.rs:757`) and all other columns equal.
  This is the mutation kill: deleted remap writes leave old identities in the file → reopen assert red.
- refusal manifests: add a small helper `closed_root_manifest(dir) -> Vec<(String, String)>` (sorted
  relative path + sha256 of each regular file) implemented by shelling out to system `sha256sum` — the
  S06 test already does exactly this (`tests/measurement.rs:250-253`); do **not** add a sha2 crate (scratch
  manifest/lock are immutable). For every refusal case (facts/vault/vault-keys/ai-sessions sentinel,
  unknown table/identity encoding, divergent auxiliary key): capture the manifest before and after, assert
  equality, and persist `auxiliary-refusal-manifest.json` in the case root.
- `Observation` for CrossDb: `schema =` the 8-table inventory (columns per manifest), `artifact_paths =
  [auxiliary.json, auxiliary-refusal-manifest.json]`, `limitations` from the auxiliary result (skills DDL
  source, ai-sessions unknown).
- `tests/measurement.rs::test_cross_db_mapping_preserves_payload_and_rejects_unknowns`: route the fixture
  build through `run_case(Case::CrossDb, &root)` (replacing the direct `Containment` + `auxiliary_fixture`
  calls at `:401-406`), assert the reopened-persistence predicate via the returned `Observation` (or a
  helper it calls), and the refusal manifests exist and are unchanged. Keep the `remap_identity` unit
  asserts and the sentinel checks.
RED proof: run this instruction against `.tmp/scratch/shard-review-mutant-2` (isolated target) → reopen
assert must fail; GREEN on pristine. (Mutant 3 also carries the remap deletion, so it is killable by the
same run.) Do not weaken the existing flag assertions; they stay, but they never again stand alone.

### 5. Constraints and non-goals
- Scratch `src/lib.rs`, `tests/measurement.rs` only; artifact JSONs under the case root. No manifest/lock,
  tracked, production, vault, live-store, external-config change; no network; no new dependency (sha256 via
  system `sha256sum`, precedent above). Keep clippy gates green with the pinned contained command.
- Do not touch `remap_identity`/`inspect`/`refuse_unsupported_auxiliary` semantics; the gap is assertion
  coverage, not builder logic. Do not run the remap deletion or guard deletion in the pristine harness.
- Keep `status`/ruling honest: INCONCLUSIVE and Phase C blocked until S08–S10 execute; the rework changes
  oracle strength, not the measurement outcome.

## [STATUS]

DIAGNOSED-NEEDS-CODER
