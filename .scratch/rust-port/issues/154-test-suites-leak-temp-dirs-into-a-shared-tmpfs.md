# 154 — the test suites leak temp dirs into a 7.8 G shared tmpfs, and exhaustion reads as a mass regression in an unrelated suite

Status: fixed — 2026-11-23. Wrapper suite in b31d9ad (single exit-hook run root, 37 os.tmpdir sites); mem-store in 2367def: dead-PID sweep inside tests/common/mod.rs temp_dir() — one std::sync::Once per process, liveness via SystemProcessProbe, SWEEP_MIN_AGE 900 s derived from the suite's per-step child bounds (60 s handshakes, 120 s probe) with 7x margin, zero call sites changed; verified by a two-run private-TMPDIR probe with hard assertions (run-1 w12 set intersection must be 0; nothing in the namespace older than the rail) and five guard mutants (no sweep / no liveness rail / no age rail / no prefix check / leftmost-numeric parse). The rail is pinned by the test to (300 s, 1200 s] (a 300 s fixture must stay, a 1200 s one must go); the foreign live pid is a real `sleep` child, not the test binary. Ceilings named: residue is bounded to the runs of the last 900 s, so the previous run's dirs survive until a later run sweeps them; a SIGKILLed run's dirs are swept by the next over-age run; Linux /proc only, elsewhere the sweep is a no-op. Follow-up families: mem-store-w51-* (engine_lock.rs:26), mem-store-w8-* (search_cost_bench.rs:1072), mem-inv-*/mem-pm-* (shard_inventory.rs, shard_path_migrate.rs), plus the mem-bridge/mem-embed/mem-episodic/mem-profile helpers; follow-up families tracked in issue 158.

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

## The wrapper suite leaks more, and faster

A second measurement the same day, after issue 150 merged its seven new tests into `main`:

- **`/tmp/om-*`: 5887 directories, 1845 MB**, oldest `Nov 18 18:36`, newest `Nov 19 07:49` — 1.4 days.
- `plugin/tests/**`: **36** `mkdtempSync` calls against **13** `rmSync` cleanups, so most fixtures have no removal path at
  all. The names are `om-<purpose>-XXXXXX` with a random suffix and **no PID**, so the dead-PID sweep proposed below
  cannot identify them; the wrapper needs its own answer — a single per-run root (`om-run-<pid>/`, whose parent a sweep
  can identify), a `t.after` per test, or the runner's own temp handling.
- Clearing both suites' residue (`om-*`, `toast-*`, `opencode-mem-*`, `mem-store-*`, about **2.1 G**) took `/tmp` from
  1.8 G free to **3.9 G free**, without touching `/tmp/opencode` (2.1 G, live host data that is not ours to delete).

Together the two suites account for essentially all the pressure: 68 MB per `mem-store` run plus 5887 wrapper dirs over
1.4 days. Either one alone would eventually fill a 7.8 G tmpfs; they did it together, and the symptom landed on the
suite that happened to run second.

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

## Re-measured 2026-11-23 — the wrapper now dominates, and one file owns half of it

Fresh count on `main` at `3d29955` (the tree after issue 150's suite run, i.e. one wrapper run old):

```
wc -l /tmp listing       785 entries
du -sh /tmp              611M          (tmpfs, RAM — `tmpfs /tmp tmpfs rw,...`)
ls -d /tmp/om-* | wc -l  739
du -sch /tmp/om-*        571M          → 93% of the tmpfs is the wrapper's residue
```

Attribution, `du -sm /tmp/om-* | sort -rn | head`:

- **4 × 81 M, all `om-log-sink-*`** = 324 M, i.e. **57% of all wrapper residue in this snapshot**. Contents:
  `om-log-sink-*/opencode-mem/engine/linux-x64/opencode-agents-memory`, `83389016` bytes — the real staged engine,
  copied into each case's temp root by the cases that need it, and never removed.
  `plugin/tests/log-sink.test.mjs:40-51` (`runCase`) `mkdtempSync`es the root, `:42` stages the package, and the file
  registers **no** removal for it — not `t.after`, not `process.on("exit")`.
- The remaining 735 `om-*` dirs are ~1 M each: 119 `om-refresh-*`, 96 `om-capture-*`, 83 `om-inject-*`,
  72 `om-compaction-*`, 68 `om-telemetry-*`, 48 `om-v2-*`, …

Two corrections to the sections above, which were right about the mechanism and stale about the shape:

1. **Option 0 (dead-PID sweep) no longer buys the most.** Only **114** of the 785 names carry a PID at all
   (`om-home-<pid>-`); **671** are `mkdtempSync` random suffixes. Of the 114 with a PID, 46 are dead — **8.9 M**.
   The sweep is worth ~1.5% of the residue: the wrapper's `om-<purpose>-XXXXXX` names cannot be identified by it, as
   already noted above. The Rust `mem-store` side (276 dirs / 68 M a run) still fits option 0 and remains its own wave.
2. **The wrapper's answer is one mechanism, not 43.** `plugin/tests/**` now has **43** `mkdtempSync` sites against
   **13** `rmSync` cleanups. `plugin/tests/harness/bridge.mjs:53-54` already ships the pattern that fixes them:
   `fs.mkdtempSync(...)` followed by `process.on("exit", () => fs.rmSync(root, { recursive: true, force: true }))`.
   A single exported `tempRoot(prefix)` helper carrying that pair, adopted at the call sites, inverts the default from
   leak to clean in one place. Named ceiling of the exit-hook shape: a `SIGKILL`ed suite still leaks, exactly as today.

Order of value, measured: the 324 M in `log-sink` costs the fewest lines; the remaining 247 M costs the helper rollout;
the 8.9 M dead-PID Rust sweep is the smallest prize of the three.
