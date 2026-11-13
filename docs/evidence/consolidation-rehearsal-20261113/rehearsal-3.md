# Rehearsal-3 — lossless shard consolidation, STEP 10, attempt 3, on `eacb66d`

Scratch root (fresh, this attempt only): `/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/scratch/rehearsal4-20261113T113418Z` (relative: `.tmp/scratch/rehearsal4-20261113T113418Z/`).
Old `rehearsal*` / `rehearsal2d*` directories were not resumed, not read for evidence, not deleted.

## Provenance
- HEAD: `eacb66dd25edeac7d1d41b3b366756a3d380b006` (MEASURED: `git rev-parse HEAD`).
- Binary `target/debug/opencode-agents-memory`: sha256 `9fa3dfdf1db05a2888c1535f5e7eeb80c7ff48f369cffac8542fc92d4decdb7d`, `cargo build --offline --locked` → exit 0 (MEASURED, `setup.txt`). Same hash as rehearsal-2.
- Frozen snapshot manifests, re-verified before and after every run (MEASURED, `frozen-after.txt`):
  `manifest.final.sha256`, `manifest.baseline.sha256`, `manifest.sha256` all `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e`.
  `data` mode 555, `recovery/` has 1 entry, no live reservation file.
- No invocation carried `/home/opencode/.opencode-mem/data`. `grep` over every stdout/stderr: 0 files (MEASURED, `live-s-hits.txt`).

## 1. ANOMALY VERDICT (decides the rest)

**Verdict: the anomaly reproduces for `apply`. It does NOT reproduce for `consolidate-snapshot`. F6's install-side gate holds. The apply-side gap is a real measured exit-0 and is BLOCKING under the brief's rule 2.**

Measured, each on a fresh `cp -a` copy with no custody directory:

| step | command | exit | stderr | label |
|---|---|---|---|---|
| apply, NO custody | `consolidate-shards --plan f6/plan.json --custody f6/custody --storage-root f6/data` | **0** | empty (0 bytes) | MEASURED (`f6/apply.exit`, `f6/apply.stderr`) |
| afterwards | `baseline.sha256` in `f6/custody` | **absent** | — | MEASURED (`f6/custody-inspect.txt`) |
| afterwards | `journal.json` in `f6/custody` | **absent** | — | MEASURED |
| staged-verify, NO baseline | `consolidate-staged-verify --custody f6/custody --staged <staged>` | **0** | empty | MEASURED (`f6/stagedverify.exit`) |
| install after that | `consolidate-install --custody f6/custody --staged <staged>` | **77** | `consolidation.snapshot_refused` "baseline unreadable: No such file or directory (os error 2)" | MEASURED (`f6/install2.stderr`) |
| install with no journal yet | same, before staged-verify | 77 | `consolidation.snapshot_refused` "install requires a StagedVerified journal record" | MEASURED (`f6/install.stderr`) |

Consequences, measured: S and staged intact after the refused install (both `exists=yes`); no baseline ever written.

Reading (INFERRED from the event names and the rehearsal-1 F6 fix text "install should call `verify_snapshot(custody)?` before L3/L4"): **install-side F6 is CLOSED** — the install with no baseline is refused 77 before L3/L4, S untouched. **apply and staged-verify exit 0 without any baseline** and write a sidecar, `staged/`, and `journal.json`, so a tree can be staged on a custody that cannot be restored. Brief rule 2 is literal: apply exits 0 without a baseline ⇒ blocking finding. I report it as blocking. Whether the architecture requires a baseline at apply is a design question I did not settle here (not read in this pass); the decision belongs to the owner, not to this rehearsal.

Empty-stderr 77 from `consolidate-snapshot`: **not reproduced.** Clean invocations:
- Relative `--custody` path → exit **64**, stderr `consolidate-snapshot: --custody <absolute-dir> is required` (MEASURED, `anom/snapshot.stderr`). This was my own mistake, not an engine defect.
- Absolute path, first run → exit 0 (MEASURED, `anom/snapshot.exit`).
- Same path again → exit **77**, stderr `consolidation.snapshot_refused` "custody create: File exists (os error 17)" (MEASURED, `anom/snapshot2.stderr`). **77 carries a message; the empty-stderr 77 is not reproducible.** The most likely source is the earlier mixed directory (`rehearsal2d` holds a `custody` from a prior attempt), which I did not read as evidence.

## 2. Item verdicts

| item | verdict | measurement |
|---|---|---|
| **anomaly: apply exit 0 with no baseline** | **NOT CLOSED (blocking, brief rule 2)** | §1 table |
| **F6 install without custody baseline** | **CLOSED on install** | install refuses 77 `baseline unreadable`, S/staged untouched (§1) |
| F5 marker-plant | **CLOSED — alarm fires and is not silenced** | §3 |
| F3 `2991cca9…` carried shard | **CLOSED as carried-legacy, not a regression** | §4 |
| F3 34 targets | **CLOSED** | §4 |
| Clean-copy integrity / FK / shard_stats | **CLOSED on clean copy** | §4 |
| Oracle reparent (§C.5a) | **CLOSED** — 34/34 exact, `agent_sessions` 888→887, reparent 300, sidecar agrees | §5 |

## 3. F5 marker-plant — both halves

**Half A, the alarm fires.** On `f5/` (fresh copy): apply 0 → staged-verify 0 → install 0 → plant the engine's own `.wal-mode` (byte-identical copy of the frozen `data/.wal-mode`, sha `98a1b243…`, MEASURED `f5/snap-wal-mode.txt`) into installed S → `consolidate-verify` **exit 77**, `consolidation.snapshot_refused` "post-install source edit: installed S differs from the verified staged manifest", journal → `RollingBack` (MEASURED `f5/settle-marker.exit`, `f5/journal-marker.txt`).

**Half B, it is not silenced.** Clean settlement on `run/` (no marker): `consolidate-verify` **exit 0**, journal `Installing → InstalledUnverified → Verified` (MEASURED `run/settle-after-recover.exit`, `run/journal-after-settle.txt`). Source (READ): `M1_EXCLUDED: [&str; 2] = ["status", "engine.lock"]` at `src/shard_consolidation.rs:3011`; settlement compare is plain `manifest_of(storage_root)? != staged_manifest` at :4783; `manifest_with` 0 hits in `src`/`tests`; `SETTLEMENT_EXCLUDED` 0 live hits (one doc comment in `tests/shard_consolidation.rs:5098` names the removed mutant).

Caveat (MEASURED, honest): the marker I planted is a byte copy of the engine's `.wal-mode`, not produced by a live writable open of S. I did not locate a safe, scoped entry point for a live writable open on a copy (`open_guard.rs` grep did not name one). The refusal is measured for that byte content; a live-open-generated marker is not separately measured.

## 4. F3 — carried shard `2991cca9ba1ab013` and the 34 targets

- **Frozen source** (`pristine/data`, the frozen-snapshot copy): `shard_stats_present=False`, `memories=4` (MEASURED, `f3-check.txt`).
- **Staged** (`staged-clean`): `shard_stats_present=False`, `memories=4` (MEASURED).
- Source has no such table ⇒ absence is **carried faithfully** ⇒ **CLOSED as carried-legacy, not a regression.** Not a fix; a property of the carried file.
- **34 targets**: `shard_stats.vector_count` = `COUNT(*)` on **34/34**, 0 mismatch, 0 missing table (MEASURED, `f3-targets.txt`). Rehearsal-1 had 34/35 wrong; that is now correct.

## 5. Oracle — §C.5a reparent model, derived from the draft, not the implementation

Oracle: `.tmp/scratch/oracle3/oracle3.py` (new, written this pass). Note: the prior `oracle2/oracle.py` was **not reused** because it (a) counted reparents over ALL source child rows rather than the child table's post-collapse survivors, (b) lacked §C.5a's "still resolved ⇒ untouched" clause, and (c) hardcoded `agent_sessions`. Reads only the pristine copy, the plan, and the staged tree; the sidecar is compared afterwards and never used as an input.

Command: `python3 .tmp/scratch/oracle3/oracle3.py plan-run.json pristine/data run/consolidation-staged-a9702ebfd7d22414 oracle3/oracle.json` → exit 0 (MEASURED).

- **34/34 memory targets exact**: 0 missing, 0 extra, 0 cell mismatch across 19 columns (MEASURED, `oracle3/summary.txt`). Totals: source 5827, survivors 5827, occurrences 5827, expected 5899 = staged 5899.
- **`agent_sessions` `rows_equal: true`**: source 888, expected 887, staged 887, discarded 1 (MEASURED).
- **Reparent**: one tuple `(episodic.db, agent_sessions, parent_session_id, 01a198b1-…→01a1c7bf-…)`, **300 rows** (MEASURED).
- **Sidecar vs oracle** (`oracle3/sidecar-compare.txt`): set match True, rows match True. Sidecar reparent lines 1, rows 300; oracle 1, 300. **No disagreement.**
- Sidecar 18 lines = 1 divergent + 16 carry (99 rows) + 1 reparent (300 rows); 8254 B.

## 6. Clean-copy integrity / FK / shard_stats (numbers of record)

Run on `staged-clean`, a `cp -a` of the staged tree taken before any probe read it (MEASURED, `integrity-fk-stats.txt`):
- DBs checked: **41**. `PRAGMA integrity_check` not `ok`: **0**.
- FK dangling (derived reference set from `PRAGMA foreign_key_list`, plain-SQL count): **0**.
- Shards with no `shard_stats`: 1 (`2991cca9…`, see §4).
- Earlier rehearsal-2 numbers (integrity/FK on an already-touched tree) are a labelled cross-check only: they agree (integrity ok, FK 0).

## 7. Not run in this pass (honest gaps)

- **Rollback** on this attempt's `run/`: not run. Measured in rehearsal-2 (rollback 0, digests equal baseline); not re-measured here.
- **Repeat-apply 77 at `InstalledUnverified`/`Verified`**: not run here. Measured in rehearsal-2 (77 at both phases).
- **Registry and cosine smoke**: not run here (rehearsal-2 also left them NOT MEASURED).
- **Live writable-open marker** for F5 (see §3 caveat).

## 8. Steps 11–13

Untouched. No commit, push, cutover, live mutation. Live S never opened.

## Findings
- **BLOCKING (brief rule 2)**: `consolidate-shards --apply` and `consolidate-staged-verify` exit 0 with no custody baseline, writing sidecar + journal + staged tree. Install refuses, so no install proceeds — but a staged tree is produced on a custody that cannot restore. Owner decides whether the design requires a baseline at apply.
- Not a defect: `consolidate-snapshot` 64 on relative `--custody` (my invocation error).
- Not reproduced: empty-stderr 77 from `consolidate-snapshot` (repeat snapshot now gives a message).
- Oracle gap closed: `oracle3` models §C.5a; `agent_sessions` matches; no sidecar disagreement.

SETTLEMENT: BLOCKED — the apply/staged-verify path exits 0 with no custody baseline (brief rule 2, measured in §1; install-side F6 is closed). Rollback, repeat-apply, registry and cosine smoke were not run this pass. All other items measured clean on a fresh copy (F5 both halves, F3 carried-legacy and 34/34 targets, clean-copy integrity/FK, oracle 34/34 with sidecar agreement).
