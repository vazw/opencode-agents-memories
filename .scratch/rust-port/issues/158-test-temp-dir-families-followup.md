# 158 — the three temp-dir families issue 154 deferred

Status: fixed — 2026-11-23, commit `e11b87a`. The wave lands no production code: `crates/mem-bridge/tests/common/mod.rs` renames `temp_dir` to `process_temp_dir` (5 process-lifetime call sites) and gives `TestDir` a `Deref<Target = Path>`, the 41 bare `temp_dir(` sites convert onto `TestDir`, `crates/mem-store/tests/{engine_lock,search_cost_bench}.rs` each change one format literal into the swept `mem-store-w12-` namespace, and `crates/mem-profile/tests/common/mod.rs` gains a copied dead-pid sweep plus `tests/temp_hygiene.rs` (its eight-fixture copy).

Follow-up of issue 154 (whose mem-store half closed the sweep for its own namespace). The tracker below records the three families this wave owns, the two crates measured as needing no work, and the ceilings each shape accepts.

## Measured baselines (private `TMPDIR` probe, one suite run per crate)

- mem-bridge 70 dirs / 227 entries / 7 MB, rc=0 — residue outside the five `OnceLock<LogSink>` sink names.
- mem-store 263 `mem-store-w12-*` + 7 `mem-store-w51-*` + 6 `mem-store-w8-*` dirs, rc=101 (one pre-existing red, `user_prompts_schema_v2::test_v2_file_is_a_noop_and_stays_2`) — plus 3 `mem-inv-*` dirs owned by `shard_inventory.rs` and outside this wave.
- mem-profile 20 dirs / 106 entries / 5 MB, rc=101 (two pre-existing `user_prompts_schema` reds).
- mem-embed: 1 dir, its helper returns an RAII `TempDir` — no work. mem-episodic: 0 dirs — no work.

## Shapes taken

1. **mem-bridge — convert onto the crate's existing `TestDir`, no second mechanism.** `TestDir` (273 uses) already removes its tree on `Drop`; `temp_dir` becomes `pub fn process_temp_dir` for the 5 process-lifetime log sinks only, and `impl Deref for TestDir` lets the receiver-position sites compile unchanged. Generic-bound positions (`.path()`, `.path().to_path_buf()`) and the 15 dead `remove_dir_all` / `cleanup_temp_dir` lines are edited explicitly. Predicted residue: 70 → 5.
2. **mem-store — rename the two duplicate helpers into the swept namespace.** `engine_lock.rs` becomes `mem-store-w12-w51-{prefix}-{pid}-0-{nanos}` and `search_cost_bench.rs` becomes `mem-store-w12-w8-{prefix}-{pid}-{counter}-{nanos}`; both then parse under the shipped `dead_pid_from_name`. `temp_path` stays non-creating (`:1977` asserts `!fresh.exists()`).
3. **mem-profile — copy the mem-store sweep with the `mem-profile-w31-` prefix** (~60 lines) **and the `temp_hygiene.rs` test copy** (~150 lines, eight fixtures). Rail 900 s, 15x the crate's largest bounded child wait (`CHILD_TIMEOUT` 60 s).

## Ceilings accepted

- Duplicated sweep code between mem-store and mem-profile; a shared testkit crate is rejected (new crate + three dev-dependency edges + ADR-0001 table amendment for ~60 duplicated lines).
- mem-bridge keeps 5 dirs per run, growing 5/run; bounded only by a sweep, which this wave rejects.
- mem-store residue is bounded to the last 900 s of runs; a lone `engine_lock` / `search_cost_bench` binary has no `mod common;` and relies on another mem-store binary in the same `TMPDIR` to sweep.
- Both sweeps are no-ops without `/proc`; the `mem-inv-*` family is closed (line 27) and `mem-pm-*` remains measured, not edited.
- No source guard: a future test can still write a bare creator, visible only to the next residue probe.
- 2026-11-23: residual `mem-inv-*` family (`shard_inventory.rs`) closed — three creators now `common::temp_dir`; `mem-pm-*` measured 0 leaked, latent, not edited.
