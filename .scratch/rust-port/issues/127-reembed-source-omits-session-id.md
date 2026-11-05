# 127 — rebuild_shard_safely SELECT_REEMBED_SOURCE omits session_id column

Status: done — 2026-11-05

## What happened

During the Issue 101 technical audit of `crates/mem-store/src/migration_service.rs`, a silent data corruption defect was identified at line 772:

```rust
session_id: None, // not part of SELECT_REEMBED_SOURCE
```

The underlying query `sql::SELECT_REEMBED_SOURCE` (`crates/mem-store/src/sql.rs:358-364`) selects 15 columns:

```sql
SELECT
  id, content, container_tag, tags, type, created_at, updated_at, metadata,
  display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned
FROM memories
ORDER BY created_at DESC
```

It omits `session_id`. When `rebuild_shard_safely` constructs `MemoryInsert` for the newly staged shard, `record.session_id` is set to `None`.

When `insert_vector_statement` runs (`crates/mem-store/src/vector.rs:91`), it calls `resolve_session_id(value.session_id.clone(), value.metadata.as_deref())`. While `resolve_session_id` attempts fallback extraction of `sessionID` from `metadata` JSON, any memory whose `session_id` column was populated directly (e.g., via session capture or v2 direct insert) without being mirrored in `metadata` JSON will have its `session_id` silently overwritten with `NULL` upon re-embedding.

## Why it is a real defect

* **Data loss on migration**: Scope re-embedding (`--mode=reembed-scope`) is expected to change only vector embeddings when switching embedding models or dimensions. Silently clearing `session_id` corrupts session associations for historical memories.
* **Inconsistent fallback**: Memories inserted through routes that set `session_id` column without storing `sessionID` in `metadata` lose their session identity permanently across re-embedding.

## What would close it

- [x] 1. Update `sql::SELECT_REEMBED_SOURCE` in `crates/mem-store/src/sql.rs` to select `session_id` (16 columns total).
- [x] 2. Update `rebuild_shard_safely` in `crates/mem-store/src/migration_service.rs` to extract `session_id`:
  `session_id: memory.text("session_id").filter(|id| !id.is_empty()),`
- [x] 3. Add an integration test in `crates/mem-store/tests/reembed.rs` (or equivalent migration test suite) verifying that a memory with a populated `session_id` and absent `sessionID` in `metadata` retains its `session_id` after re-embedding.
- [x] 4. Non-vacuous mutant: omit `session_id` from `SELECT_REEMBED_SOURCE` or set `session_id: None` in `rebuild_shard_safely` and verify the test fails with `expected Some(session_id), got None`.

## Evidence

* `crates/mem-store/src/migration_service.rs:772`: `session_id: None, // not part of SELECT_REEMBED_SOURCE`
* `crates/mem-store/src/sql.rs:358-364`: `SELECT_REEMBED_SOURCE` definition omitting `session_id`
* `crates/mem-store/src/vector.rs:91`: `resolve_session_id(value.session_id.clone(), value.metadata.as_deref())`
* Discovered and documented by Issue 101 audit (`docs/evidence/issue-101-mem-store-audit/README.md`).

## Comments

### 2026-11-05 — Closeout (Issue 127)
- **16-column SELECT_REEMBED_SOURCE**: In `crates/mem-store/src/sql.rs:358-365`, `session_id` appended as the 16th column. `crates/mem-store/src/migration_service.rs:772` extracts `session_id: memory.text("session_id").filter(|id| !id.is_empty())`.
- **Golden SQL tracked divergence**: In `crates/mem-store/tests/golden_sql.rs:414-425`, `FROZEN_SELECT_REEMBED_SOURCE` updated to match 16 columns with tracked divergence note documenting why upstream TypeScript omitted `session_id` (due to reliance on `metadata.sessionID` JSON parsing) while Rust port requires direct preservation for tier-1 session associations.
- **Integration test matrix**: Dedicated integration test `test_reembed_preserves_session_id_without_metadata_fallback` in `crates/mem-store/tests/turso_reembed_migration.rs` verifies a 3-case row matrix (Case A: direct `session_id` without metadata key; Case B: metadata `sessionID` fallback; Case C: null on both). Passed GREEN in 0.09s (`1 passed; 0 failed`).
- **Mutant kill verified**: Setting `session_id: None` at `crates/mem-store/src/migration_service.rs:772` fails Case A with:
  `assertion left == right failed: Case A: direct session_id must survive re-embedding without metadata fallback (left: None, right: Some("session-live-001"))`.
  Reverting mutant restores GREEN.
- **Verification & Gates**: `cargo fmt --all --check` clean, `cargo clippy --workspace --lib` clean (0 warnings), `cargo clippy --workspace --all-targets` clean (0 warnings), `turso_reembed_migration` (11/11 passed), `golden_sql` (16/16 passed). Zero schema changes, zero new dependencies.
