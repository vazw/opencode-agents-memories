//! SQL statements for shard retention sweep and schema v4 stamp (issue 39).

/// Adds the nullable `source_prompt_created_at` retention stamp column to `memories` (shard v4).
pub const RETENTION_STAMP_ALTER: &str =
    "ALTER TABLE memories ADD COLUMN source_prompt_created_at INTEGER";

/// Selects all shards from the registry ordered by scope, scope_hash, and shard_index (issue 39).
pub const LIST_ALL_SHARDS_FOR_RETENTION: &str =
    "SELECT * FROM shards ORDER BY scope ASC, scope_hash ASC, shard_index ASC";

/// Selects memory candidates for retention sweep based on updated_at and the retention stamp.
pub const RETENTION_CANDIDATES_SELECT: &str = "SELECT id, container_tag, is_pinned, source_prompt_created_at FROM memories WHERE updated_at < ?1 AND (source_prompt_created_at IS NULL OR source_prompt_created_at < ?2)";
