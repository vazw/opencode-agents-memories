# Evidence: Issue 101 — mem-store Unexamined Surface Audit Report

## Executive Summary and Scope Boundary

This audit examines the six modules in `crates/mem-store` that were left unexamined by the 2026-11-01 opsec audit (`.tmp/progress/opsec-audit-20261101T090956Z.md:167`). The opsec audit excluded rotation internals, migration service, reaping, delete guards, and count drift arithmetic, citing them as a potential home for silent corruption.

### Out of Scope (Already Owned)

The following modules and behaviors were already owned by specific issues and are NOT part of this audit:
* One-active-shard rotation, reactivation, reconcile skip: Issue 43 W4 (`030a0bd`, ADR-0022).
* Retired-shard reaping: Issue 43 W7 (`43d9954`).
* Sidecar deletion contract (`delete_guard.rs`): Issue 46.
* `legacy_migrator` reconcile / `ActiveShardConflict`: Issue 77 (`-22-2` residual).
* Scope-wide delete-by-id: Issue 72.

### In Scope (Audited Here)

The six modules examined here total 3 379 lines across:
1. `crates/mem-store/src/migration_service.rs` (855 lines)
2. `crates/mem-store/src/shard_path_migration.rs` (1 040 lines)
3. `crates/mem-store/src/shard_inventory.rs` (697 lines) & `crates/mem-store/src/shard_registry_sql.rs` (54 lines)
4. `crates/mem-store/src/prompts.rs` (29 lines)
5. `crates/mem-store/src/user_prompts_sql.rs` (26 lines)
6. `crates/mem-store/src/shard_facts.rs` count paths (678 lines)

Each module was audited across four axes:
* Lock ordering and deadlock potential
* Arithmetic safety (overflow, underflow, truncation)
* Error propagation and clean resource recovery
* Silent data corruption risks

## Current Measurements

Measured on 2026-11-04 in target worktree `track-a-docs`:
* Module count: `find crates/mem-store/src -name '*.rs' | wc -l` yields 32 modules.
* Line count: `find crates/mem-store/src -name '*.rs' | sort | xargs wc -l | tail -n 1` yields 16 870 total lines.
* Baseline delta: The 2026-11-01 baseline was 16 724 lines across 32 modules. The delta is +146 lines across subsequent waves.

## Re-Derivation of Issues 43.2 and 43.3 Claims

### Issue 43.2: `vector_count` write path self-healing
* Claim: `metadata.db.shards.vector_count` is a denormalized counter that only drifts.
* Measured status: Refuted by code inspection (MEASURED).
* Live citations:
  * In `crates/mem-store/src/shard_manager.rs:1341-1358`, whenever `prepare_write_shard` resolves an active shard, it calls `crate::shard_facts::prepare_write_shard`.
  * Inside `prepare_shard_facts` (`crates/mem-store/src/shard_facts.rs:336`), `sync_count(db)` executes `SYNC_COUNT_SQL`:
    `INSERT INTO shard_stats (id, vector_count) SELECT 1, COUNT(*) FROM memories ON CONFLICT(id) DO UPDATE SET vector_count = excluded.vector_count` (`crates/mem-store/src/shard_facts.rs:47`).
  * The resulting count is read back via `read_stored_count(db)` and returned as `outcome.facts.vector_count`.
  * In `crates/mem-store/src/shard_manager.rs:1357`, `self.sync_shard_vector_count(shard, outcome.facts.vector_count)` updates `shards.vector_count` in `metadata.db` using `SET_VECTOR_COUNT` (`crates/mem-store/src/shard_registry_sql.rs:37`).
  * Conclusion: Any drift in `metadata.db` is self-healed by an exact `COUNT(*)` on every write shard resolution.

### Issue 43.3: `shard_metadata.embedding_model` read/write status
* Claim: `shard_metadata.embedding_model` is written at init but read by nothing.
* Measured status: True when originally filed in Issue 43, but resolved in Wave 3 (ADR-0018) (MEASURED).
* Live citations:
  * In `crates/mem-store/src/shard_facts.rs:379-427`, query `SELECT_MODEL_FACTS` (`SELECT key, value FROM shard_metadata WHERE key IN ('embedding_fingerprint','embedding_model')`) reads `embedding_model` alongside `embedding_fingerprint`.
  * `read_model_facts` is called by `prepare_shard_facts` (line 308) and evaluated by `ModelAuthority` to verify model compatibility (`crates/mem-store/src/shard_facts.rs:396-410`).
  * In `crates/mem-store/src/migration_service.rs:219, 295`, `metadata_value(&db, "embedding_model")` is actively read to detect dimension and model mismatches for re-embedding decisions.
  * Conclusion: `embedding_model` is an active authority read and enforced in multiple production and migration code paths.

## Module-by-Module Technical Audit Findings

### 1. `crates/mem-store/src/migration_service.rs` (855 lines)
* Lock ordering (MEASURED): Atomic guard (`is_running: AtomicBool`, `Ordering::SeqCst` swap/store at lines 334, 347, 476, 480) precedes operation lock (`acquire_turso_operation_lock` at lines 380, 512), which precedes scope write lock (`with_scope_write_lock` at line 452), which precedes individual database connection acquisitions. Lock releases are guaranteed in `finally`/`match` blocks (`operation_lock.release()`, `is_running.store(false)`). No lock inversion or deadlock risk observed.
* Arithmetic safety (MEASURED): Lines 216-217 dimension parsing defaults to 0 on failure; lines 431 and 464 maintain re-embedded count accumulators; line 795 exact equality comparison `staged_count != expected as u64` prevents partial/truncated swaps.
* Error propagation (MEASURED): `migrate_catch_all` maps errors into `MigrationResult { success: false, error: Some(...) }` while ensuring cleanup (staged DB deleted at lines 670-684, swap journal deleted at lines 685-687); failed swap rename rolls back via `.pre-reembed-*.bak` restoration before rethrowing original error (lines 834-842).
* Silent data corruption risk (MEASURED - DEFECT FOUND):
  Line 772: `session_id: None, // not part of SELECT_REEMBED_SOURCE`.
  Query `sql::SELECT_REEMBED_SOURCE` (`crates/mem-store/src/sql.rs:358-364`) selects 15 columns but omits `session_id`. When `rebuild_shard_safely` constructs `MemoryInsert`, `record.session_id` is set to `None`. While `vector.rs:91` attempts fallback extraction of `sessionID` from `metadata` JSON, any v2 memory whose `session_id` column was populated directly without being mirrored in `metadata` JSON will have its `session_id` silently overwritten with `NULL` upon re-embedding.
  Logged as defect issue: `.scratch/rust-port/issues/127-reembed-source-omits-session-id.md`.

### 2. `crates/mem-store/src/shard_path_migration.rs` (1 040 lines)
* Lock ordering (MEASURED): Acquires `"path-migration"` operation lock; no deadlock interactions with `ShardManager` internal locks.
* Arithmetic safety (MEASURED): Lines 677-684 conflict check tests both `live_count` (live `SELECT COUNT(*)`) and `shard.vector_count` (`live_count.max(shard.vector_count)`); line 944 verifies staged count against expected count before swap journal is written (`staged_count != mv.vector_count`).
* Error propagation (MEASURED): Robust `PathMigrationError` mapping; staging errors abort cleanly before swap journal creation; swap state journal (`write_swap_state`) supports crash recovery (`recover_path_migration`).
* Silent data corruption risk (MEASURED): None. Strict refusal on non-empty target shards prevents data clobbering.

### 3. `crates/mem-store/src/shard_inventory.rs` (697 lines) & `crates/mem-store/src/shard_registry_sql.rs` (54 lines)
* Lock ordering (MEASURED): Inventory operations are read-only filesystem and database probes; `shard_registry_sql.rs` statements execute inside single transactions (`SHARDS_REPAIR_EXTRA_ACTIVE` + `SHARDS_ONE_ACTIVE_INDEX` in v2 schema migration).
* Arithmetic safety (MEASURED): Safe u32 parsing for shard indexes (`parse_project_shard_filename`); checked sums across descriptors.
* Error propagation (MEASURED): Missing, unregistered, or corrupt SQLite files gracefully mapped to `ShardInventoryStatus` without panic.
* Silent data corruption risk (MEASURED): None. `SHARDS_ONE_ACTIVE_INDEX` partial unique index enforces database-level guarantee of exactly one active shard per `(scope, scope_hash)`. `MARK_SHARD_INACTIVE_IN_SCOPE` strictly scopes deactivation by `(id, scope, scope_hash)` preventing cross-scope tenant deactivations.

### 4. `crates/mem-store/src/prompts.rs` (29 lines)
* Trait definition `PromptPathUpdater` and `NoopPromptPathUpdater`.
* Lock ordering, arithmetic safety, and corruption risks (MEASURED): N/A. Clean no-op contract returning `Ok(0)`.

### 5. `crates/mem-store/src/user_prompts_sql.rs` (26 lines)
* Defines `USER_PROMPTS_V2_COLUMNS` (`capture_attempts`, `provider_id`, `model_id`) and DDL helper `user_prompts_v2_alter`.
* Lock ordering, arithmetic safety, and corruption risks (MEASURED): Clean. Applied dynamically via `PRAGMA table_info` probe in `schema_migrations.rs:apply_user_prompts_schema`, preventing duplicate column ALTER errors.

### 6. `crates/mem-store/src/shard_facts.rs` count paths (678 lines)
* Lock ordering (MEASURED): `ModelFactsCache` uses an internal mutex held exclusively during in-memory map lookup/insert; all DB queries (`read_model_facts`) run outside the lock. Epoch-based generation tracking prevents stale read-write cache races.
* Arithmetic safety (MEASURED): `SHARD_STATS_DDL` enforces `vector_count INTEGER NOT NULL CHECK (vector_count >= 0)`; counts parsed with `u64::try_from(count).unwrap_or(u64::MAX)` and clamped at 0.
* Error propagation (MEASURED): DB errors during count execution propagate immediately via `?`.
* Silent data corruption risk (MEASURED): None. `SYNC_COUNT_SQL` uses `SELECT 1, COUNT(*) FROM memories` inside the shard database itself, guaranteeing the count is statement-measured rather than accumulated via error-prone deltas.

## Findings Summary Table

| Finding ID | Module | Citation | Status | Description | Action |
|---|---|---|---|---|---|
| F-01 | `migration_service.rs` | `sql.rs:358-364`, `migration_service.rs:772` | MEASURED | `SELECT_REEMBED_SOURCE` omits `session_id`, causing `session_id` to be overwritten with `NULL` during re-embedding if absent in `metadata` JSON | Defect issue 127 filed |
| F-02 | `shard_facts.rs` | `shard_facts.rs:47,151-155`, `shard_manager.rs:1357` | MEASURED | Claim 43.2 refuted: `vector_count` self-heals on write-shard resolution via `SYNC_COUNT_SQL` | Documented; claim settled |
| F-03 | `shard_facts.rs` | `shard_facts.rs:379-427`, `migration_service.rs:219,295` | MEASURED | Claim 43.3 refuted: `embedding_model` is read and enforced as authority via `SELECT_MODEL_FACTS` | Documented; claim settled |
| F-04 | `shard_path_migration.rs` | `shard_path_migration.rs:677-684,944` | MEASURED | Path migration verifies counts and stages cleanly; no corruption risk found | Audit passed |
| F-05 | `shard_inventory.rs` | `shard_inventory.rs:1-697`, `shard_registry_sql.rs:1-54` | MEASURED | Safe inspection and indexing; partial unique index prevents multi-active corruption | Audit passed |
| F-06 | `prompts.rs` | `prompts.rs:1-29` | MEASURED | Clean no-op abstraction | Audit passed |
| F-07 | `user_prompts_sql.rs` | `user_prompts_sql.rs:1-26` | MEASURED | Idempotent dynamic DDL migration | Audit passed |
