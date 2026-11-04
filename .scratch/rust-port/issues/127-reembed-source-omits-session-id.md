# 127 — rebuild_shard_safely SELECT_REEMBED_SOURCE omits session_id column

Status: ready-for-agent — 2026-11-05 (defect found by issue 101 audit)

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

- [ ] 1. Update `sql::SELECT_REEMBED_SOURCE` in `crates/mem-store/src/sql.rs` to select `session_id` (16 columns total).
- [ ] 2. Update `rebuild_shard_safely` in `crates/mem-store/src/migration_service.rs` to extract `session_id`:
  `session_id: memory.text("session_id").filter(|id| !id.is_empty()),`
- [ ] 3. Add an integration test in `crates/mem-store/tests/reembed.rs` (or equivalent migration test suite) verifying that a memory with a populated `session_id` and absent `sessionID` in `metadata` retains its `session_id` after re-embedding.
- [ ] 4. Non-vacuous mutant: omit `session_id` from `SELECT_REEMBED_SOURCE` or set `session_id: None` in `rebuild_shard_safely` and verify the test fails with `expected Some(session_id), got None`.

## Evidence

* `crates/mem-store/src/migration_service.rs:772`: `session_id: None, // not part of SELECT_REEMBED_SOURCE`
* `crates/mem-store/src/sql.rs:358-364`: `SELECT_REEMBED_SOURCE` definition omitting `session_id`
* `crates/mem-store/src/vector.rs:91`: `resolve_session_id(value.session_id.clone(), value.metadata.as_deref())`
* Discovered and documented by Issue 101 audit (`docs/evidence/issue-101-mem-store-audit/README.md`).
