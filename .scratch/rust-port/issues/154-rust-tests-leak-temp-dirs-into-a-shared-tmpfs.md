# 154 — the `mem-store` test suite leaks every temp dir into a 7.8 G shared tmpfs, and exhaustion reads as a mass regression in an unrelated suite

Status: needs-triage — 2026-11-19 (defect measured with a discriminating before/after; fix shape re-priced: a dead-PID sweep inside `temp_dir()` needs zero call-site changes, an RAII guard needs 46 because `StoreConfig` holds a bare `PathBuf`)

*(Filed after a false red on `main` was traced to disk exhaustion rather than to code. This is test-harness hygiene, not a defect in the shipped store.)*

## What happened

`cd plugin && node --test tests/*.test.mjs` on `main` (`c20d8b5`, clean tree, staged engine present) reported:

```
ℹ tests 248
ℹ pass 129
ℹ fail 119
EXIT: 1
```

Every one of the 119 carried the same error, `Error: ENOSPC:` — no space left on device. The failures spanned idle
capture, compaction hooks, child-session restore and the e2e shutdown family: a shape that reads exactly like a broken
branch, and for one review round it was treated as one.

The identical tree, with nothing changed but free space, then reported:

```
ℹ tests 248
ℹ pass 248
ℹ fail 0
EXIT: 0
```

Same commit, same files, same commands. **That pair is the discriminating measurement**: the red set was environmental.

## The cause, measured

`/tmp` is a **7.8 G tmpfs** (`tmpfs 7.8G 7.7G 113M 99% /tmp`) while `/` had 62 G free — so the pressure was invisible to any
`df` of the repo. Its contents, before cleanup:

- **3435 `/tmp/mem-store-*` directories, 831 M**, oldest `Nov 16 17:31`, newest `Nov 19 06:57` — three days of runs.
- 1.26 G of dead `/tmp/cargo-install*` leftovers from interrupted installs (`Nov 10`, `Nov 16`).
- 2.1 G `/tmp/opencode` (left alone — live host data, not ours to clear).

The source of the 3435 dirs is `crates/mem-store/tests/common/mod.rs`:

- `temp_dir(prefix)` (`:20-32`) builds `{std::env::temp_dir()}/mem-store-w12-{prefix}-{pid}-{counter}-{nanos}` and calls
  `create_dir_all`. It returns a bare `PathBuf`. **There is no drop guard**, so nothing removes the directory.
- `cleanup_temp_dir(dir)` (`:47-50`) exists and is correct — but it is a *separate, explicit* call each test must remember.

Counts across `crates/mem-store/tests/`: **83** `temp_dir(` creations, **175** `cleanup_temp_dir(` calls, and **30 test
files that create temp dirs and never mention cleanup at all** (listed below). The dirs therefore accumulate on every run,
and every aborted or panicking run leaks everything it created.

Files that never call `cleanup_temp_dir`: `capture_id_migration`, `db_containment`, `engine_lock`, `episodic_v6_convergence`,
`episodic_v6_migration`, `episodic_v7_not_before`, `golden_sql`, `issue53_probe`, `prompt_stamp_migration`, `retention_sweep`,
`retired_shard_reaping`, `scope_wide_delete`, `scope_wide_memory_ids`, `search_cost_bench`, `shard_inventory`,
`shard_one_active`, `shard_open_premise`, `shard_path_migrate`, `shard_stored_path`, `shard_write_scope`,
`turso_auxiliary_db_upgrade`, `turso_connection_lifecycle`, `turso_connection_manager`, `turso_engine_migrator`,
`turso_exact_fallback`, `turso_ready_gate`, `turso_shard_recreate`, `turso_shard_rotation`, `turso_vector_search`,
`vector_store_subset`.

## Why it surfaces as someone else's failure

The Rust suite and the wrapper suite share `std::env::temp_dir()`, and the wrapper's `os.tmpdir()` tests put their
fixtures in the same 7.8 G filesystem. Neither suite reports "the disk is full" as its own condition — Node's fs errors
propagate as per-test `ENOSPC`, so a Rust-suite leak manifests as 119 **plugin** failures against code nothing touched.
The same mechanism reddened the 147 baseline earlier the same day for a different reason (a missing `.tmp/`), so this is
the second time in one day that an environment fault was read as a code defect.

## Ask

Bound the growth, or make cleanup unavoidable. Four shapes, re-priced after checking what the helper is embedded in:

0. **Sweep stale dirs by dead PID, inside `temp_dir()` itself.** On first use, list `std::env::temp_dir()`, match the
   `mem-store-w12-*` names, extract the PID, and remove the dir when `/proc/<pid>` does not exist. **Zero call sites
   change**, one function grows ~20 lines, and it bounds accumulation permanently instead of fixing one release's worth
   of dirs. It also handles today's exact failure — three days of dirs from dead runs — without touching 46 call sites.
   Cost: Linux-specific (fine here), and PID reuse is theoretically possible, so the guard should also require an
   age threshold. This is the option I would take first.
1. **RAII guard in one helper.** `temp_dir()` returns a guard (`Deref<Target = Path>`, removes on `Drop`). **Not
   drop-in as I first wrote it:** `store_config()` (`common/mod.rs:66-78`) puts the path into
   `StoreConfig.storage_path`, which is a plain `PathBuf` (`crates/mem-store/src/config.rs:13`) and cannot hold a
   guard. So this option necessarily changes `store_config`'s signature and therefore **46 call sites** across the test
   tree, not 30. Correct by construction, and the most expensive.
2. **30 per-file edits.** Add the missing `cleanup_temp_dir` calls. Cheap to write, but it is the same class of defect
   issue 147's defect 4 fixed in `plugin/tests/` — discipline the next test forgets — and it does not clean what
   panicking or aborted runs already left.
3. **Move test temp dirs into the repo's `.tmp/`.** It contradicts the house law in `docs/agents/testing.md` ("temp dirs
   under `std::env::temp_dir()` only"), so it is an owner ruling, not a triage pick.

Option 0 and option 1 are complementary rather than exclusive: 0 bounds what leaks, 1 stops the leak. If only one is
taken, 0 buys the most per line changed.

## Out of scope

- `/tmp/opencode` (live host data) and `/tmp/node-compile-cache` (regenerable) — clearing them is housekeeping, not a fix.
- The seven pre-existing `cargo test --workspace` reds; they are unrelated and recorded in issue 149's ledger.
- Adding `/tmp` monitoring. A `df -h /tmp` in the diagnosis loop is a habit, not a deliverable.

## Inverted world

If a full `cargo test --workspace` on a freshly cleared `/tmp` leaves **zero** new `mem-store-*` directories behind, then
`temp_dir()` is not the leak and this issue closes as measurement. The observation to run: count the dirs before, run the
workspace suite to completion, count again. Measured today, the count went 3435 → 0 only because the dirs were deleted;
the next full run's delta is the test of this diagnosis.

**That observation has now been run, and it confirms the diagnosis.** On a cleared `/tmp` (0 `mem-store-*` dirs), a single
`cargo test -p mem-store --no-fail-fast` left **276 new directories, 68 MB**:

```
=== mem-store dirs BEFORE ===
0
=== mem-store dirs AFTER ===
276
68      total
```

Per run, not per three days. At 68 MB a run, the 7.8 G tmpfs has room for roughly a hundred runs — which is why the
failure arrived three days after the residue began rather than on the first run. One `mem-store` run also exited 101 with
`user_prompts_schema_v2::test_v2_file_is_a_noop_and_stays_2` failing, one of the seven pre-existing reds independently
recorded in issue 149's ledger; the abort path leaks whatever it had created, which is a second source on top of the
files that never clean up.
