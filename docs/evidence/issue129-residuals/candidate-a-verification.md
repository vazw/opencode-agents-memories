# Candidate A Verification — Stats Shard Walk Narrow Projection

## Summary

Candidate A narrows the stats shard walk from `SELECT * FROM memories ORDER BY created_at DESC`
to `SELECT container_tag, type, created_at, tags FROM memories` (no `ORDER BY`), via a new
defaulted `MemoryPort::stats_memories` seam and `MemoryStatsRow` DTO.

## Measurements / Evidence

- `cargo fmt --all --check`: PASS (exit 0)
- `cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used`: PASS (exit 0)
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation`: PASS (exit 0)
- `cargo test --workspace`:
  - run 1 (2026-11-09, coder): FAILED — `mem-bridge purge_cli
    test_relative_cargo_target_dir_does_not_leak_into_crate_tree` (concurrent symlink creation;
    passes in isolation, 29/29).
  - run 1 (2026-11-09, reviewer): FAILED — `tests/production_api_wiring.rs::test_startup_audit_inventory`
    (`settled_listing` raced the async decay sweep; that file is untouched by this diff and the
    test passes in isolation and in run 2). NOT reproduced by the coder's run.
  - run 2 (2026-11-09, reviewer, 6m55s): exit 0.
  - run 3 (2026-11-10, rework): same `purge_cli` flake as the coder's run 1 (27 passed,
    `test_relative_cargo_target_dir_does_not_leak_into_crate_tree` FAILED). That flake is
    pre-existing and environmental — a stale `.tmp/scratch/issue-125-probe/` directory left by a
    prior run collapses the test's remove-then-symlink step into `AlreadyExists` and, when a
    leftover build artefact remains, makes the inner `cargo build` fail at
    `sccache: No such file or directory` on the probe symlink path. The file is untouched by this
    diff; the test exercises `--`&gt;`cargo build` via a symlink and is unrelated to the stats seam.
    Verified: `cargo test -p mem-bridge --test purge_cli` still fails; `git status` shows the
    failing test's file is unmodified by this wave; `cargo test -p mem-web
    --test stats_contract --test stats_candidate_a_sqlite --test shard_resurrection` exit 0.

## Tests

- `cargo test -p mem-store --test golden_sql test_golden_sql_stats_memories`: PASS
- `cargo test -p mem-store --test vector_store_subset test_get_stats_memories_narrow_projection_no_order_by`: PASS
- `cargo test -p mem-web --test stats_contract test_stats_memories_default_fallback`: PASS
- `cargo test -p mem-web --test stats_contract test_stats_uses_narrow_projection_seam`: PASS
- `cargo test -p mem-web --test stats_contract test_stats_order_independent_timestamp_and_tag_accumulation`: PASS
- `cargo test -p mem-web --test stats_contract test_stats_by_type_skips_empty_kind`: PASS (new; review-1 Fix 2)
- `cargo test -p mem-web --test stats_candidate_a_sqlite`: PASS (2 tests — e2e parity + adapter guard pin)
- `cargo test -p mem-web --test shard_resurrection`: PASS (5 tests)

## Non-vacuity proof (review-1 Fix 1 / Fix 2 mutants)

Each mutant was applied to the delivered tree, the named test observed RED, then the file restored.

- Mutant 1 — deleted `self.stat_shard_file(shard).await?;` at
  `crates/mem-web/src/api/store_adapter.rs:506` (`ProductionMemory::stats_memories`):
  - `test_stats_memories_adapter_refuses_missing_shard_file` FAILED:
    `the adapter must refuse before open_shard recreates the file: Err(Store("Parse error: no such table: memories"))`.
  - Handler layer stayed green (16/16): `stats_contract` 10, `stats_candidate_a_sqlite` e2e 1,
    `shard_resurrection` 5 — proving the new test is the only killer of the adapter guard.
- Mutant 2a — deleted `.filter(|kind| !kind.is_empty())` at `crates/mem-web/src/api/stats.rs:152`:
  - `test_stats_by_type_skips_empty_kind` FAILED on whole-map equality:
    `left: {"": 2, "context": 1}`, `right: {"context": 1}`.
- Mutant 2b — deleted `.filter(|tags| !tags.is_empty())` at `crates/mem-web/src/api/stats.rs:157`:
  - `test_stats_by_type_skips_empty_kind` FAILED on `duplicateTagCount`: `left: 1`, `right: 0`.

## Guard Preservation

- `ProductionMemory::stats_memories` calls `self.stat_shard_file(shard).await?` before `self.open(shard).await?`,
  preserving the issue 130 stat-before-open read-lane guard. The adapter-level guard is now pinned by a
  direct `ProductionMemory::stats_memories` call on a missing `db_path`
  (`test_stats_memories_adapter_refuses_missing_shard_file`), because `stats.rs`'s `db_path.exists()`
  pre-check makes the handler path unreachable for the guard.

## SQL Constant

`crates/mem-store/src/sql.rs`:
```rust
pub const GET_STATS_MEMORIES: &str = r#"SELECT container_tag, type, created_at, tags FROM memories"#;
```

No `ORDER BY`, no `*`, no parameters (static string — no SQL injection surface).
