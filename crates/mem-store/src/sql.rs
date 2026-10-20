//! Verbatim upstream SQL locus (blueprint W1.2, golden rule D2).
//!
//! Every statement here is a byte-transliteration of an upstream
//! `src/services/turso/*.ts` template literal (after JS string unescaping —
//! the exact runtime strings), pinned by `tests/golden_sql.rs`. Upstream SQL
//! must run on `turso` unchanged; a mismatch with upstream bytes is a
//! blocking defect. Constants are named after their upstream site.

#![allow(dead_code)]

// schema-migrations.ts L88-93 (memoryShardMigrations v1 stmt 1).
pub fn shard_metadata_ddl() -> &'static str {
    r#"
            CREATE TABLE IF NOT EXISTS shard_metadata (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            )
          "#
}

// schema-migrations.ts L151-163 (METADATA_DB_MIGRATIONS v1 stmt 1).
pub const SHARDS_DDL: &str = r#"
          CREATE TABLE IF NOT EXISTS shards (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            scope TEXT NOT NULL,
            scope_hash TEXT NOT NULL,
            shard_index INTEGER NOT NULL,
            db_path TEXT NOT NULL,
            vector_count INTEGER DEFAULT 0,
            is_active INTEGER DEFAULT 1,
            created_at INTEGER NOT NULL,
            UNIQUE(scope, scope_hash, shard_index)
          )
        "#;

// schema-migrations.ts L166-170 (v1 stmt 2).
pub const SHARDS_ACTIVE_INDEX: &str = r#"
          CREATE INDEX IF NOT EXISTS idx_active_shards
          ON shards(scope, scope_hash, is_active)
        "#;

// schema-migrations.ts L181-196 (USER_PROMPTS_MIGRATIONS v1 stmt 1).
pub const USER_PROMPTS_DDL: &str = r#"
          CREATE TABLE IF NOT EXISTS user_prompts (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            project_path TEXT,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            captured INTEGER DEFAULT 0,
            user_learning_captured BOOLEAN DEFAULT 0,
            linked_memory_id TEXT,
            capture_attempts INTEGER DEFAULT 0,
            provider_id TEXT,
            model_id TEXT
          )
        "#;

// schema-migrations.ts L96-116 (memoryShardMigrations v1 stmt 2): `F32_BLOB(${dims})`.
pub fn memories_ddl(dims: u32) -> String {
    format!(
        r#"
            CREATE TABLE IF NOT EXISTS memories (
              id TEXT PRIMARY KEY,
              content TEXT NOT NULL,
              vector F32_BLOB({dims}) NOT NULL,
              tags_vector F32_BLOB({dims}),
              container_tag TEXT NOT NULL,
              tags TEXT,
              type TEXT,
              created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL,
              metadata TEXT,
              display_name TEXT,
              user_name TEXT,
              user_email TEXT,
              project_path TEXT,
              project_name TEXT,
              git_repo_url TEXT,
              is_pinned INTEGER DEFAULT 0
            )
          "#,
        dims = dims,
    )
}

/// L118-121 in DDL order (see `MEMORIES_INDEXES`).
pub fn memories_indexes() -> [&'static str; 4] {
    MEMORIES_INDEXES
}

/// schema-migrations.ts L118-121 (memories v1 index list, in DDL order).
pub const MEMORIES_INDEXES: [&str; 4] = [
    r#"CREATE INDEX IF NOT EXISTS idx_container_tag ON memories(container_tag)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_type ON memories(type)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_created_at ON memories(created_at DESC)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_is_pinned ON memories(is_pinned)"#,
];

// schema-migrations.ts L129 (v2 stmt 1).
pub const MEMORY_SHARD_V2_ALTER: &str = r#"ALTER TABLE memories ADD COLUMN session_id TEXT"#;

// schema-migrations.ts L130 (v2 stmt 2).
pub const MEMORY_SHARD_V2_INDEX: &str =
    r#"CREATE INDEX IF NOT EXISTS idx_session_id ON memories(session_id)"#;

// schema-migrations.ts L132-138 (v2 stmt 3, `json_extract(metadata, '$.sessionID')`).
pub const MEMORY_SHARD_V2_BACKFILL: &str = r#"
            UPDATE memories
            SET session_id = json_extract(metadata, '$.sessionID')
            WHERE metadata IS NOT NULL
              AND session_id IS NULL
              AND json_extract(metadata, '$.sessionID') IS NOT NULL
          "#;

/// The three memoryShardMigrations v2 statements (L129-138), DDL order.
pub fn memory_shard_v2_statements() -> [&'static str; 3] {
    [
        MEMORY_SHARD_V2_ALTER,
        MEMORY_SHARD_V2_INDEX,
        MEMORY_SHARD_V2_BACKFILL,
    ]
}

/// schema-migrations.ts L198-209 (user_prompts v1 indexes, in DDL order).
pub const USER_PROMPTS_INDEXES: [&str; 6] = [
    r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_session ON user_prompts(session_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_captured ON user_prompts(captured)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_created ON user_prompts(created_at DESC)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_project ON user_prompts(project_path)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_linked ON user_prompts(linked_memory_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_user_learning ON user_prompts(user_learning_captured)"#,
];

/// L198-209 in DDL order (see `USER_PROMPTS_INDEXES`).
pub fn user_prompts_indexes() -> [&'static str; 6] {
    USER_PROMPTS_INDEXES
}

// schema-migrations.ts L218-232 (USER_PROFILES_MIGRATIONS v1 stmt 1).
pub const USER_PROFILES_DDL: &str = r#"
          CREATE TABLE IF NOT EXISTS user_profiles (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            user_name TEXT NOT NULL,
            user_email TEXT NOT NULL,
            profile_data TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            last_analyzed_at INTEGER NOT NULL,
            total_prompts_analyzed INTEGER NOT NULL DEFAULT 0,
            is_active BOOLEAN NOT NULL DEFAULT 1
          )
        "#;

// schema-migrations.ts L234-245 (USER_PROFILES_MIGRATIONS v1 stmt 2).
pub const USER_PROFILE_CHANGELOGS_DDL: &str = r#"
          CREATE TABLE IF NOT EXISTS user_profile_changelogs (
            id TEXT PRIMARY KEY,
            profile_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            change_type TEXT NOT NULL,
            change_summary TEXT NOT NULL,
            profile_data_snapshot TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (profile_id) REFERENCES user_profiles(id) ON DELETE CASCADE
          )
        "#;

/// schema-migrations.ts L246-255 (USER_PROFILES_MIGRATIONS v1 indexes, in
/// DDL order).
pub const USER_PROFILES_INDEXES: [&str; 4] = [
    r#"CREATE INDEX IF NOT EXISTS idx_user_profiles_user_id ON user_profiles(user_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_profiles_is_active ON user_profiles(is_active)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_profile_changelogs_profile_id ON user_profile_changelogs(profile_id)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_user_profile_changelogs_version ON user_profile_changelogs(version DESC)"#,
];

/// schema-migrations.ts L246-255 in DDL order (see `USER_PROFILES_INDEXES`).
pub fn user_profiles_indexes() -> [&'static str; 4] {
    USER_PROFILES_INDEXES
}

// vector-search.ts L50-55.
pub const INSERT_WITH_TAGS: &str = r#"
  INSERT INTO memories (
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
    metadata, session_id, display_name, user_name, user_email, project_path, project_name, git_repo_url
  ) VALUES (?, ?, vector32(?), vector32(?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

// vector-search.ts L57-62.
pub const INSERT_WITHOUT_TAGS: &str = r#"
  INSERT INTO memories (
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
    metadata, session_id, display_name, user_name, user_email, project_path, project_name, git_repo_url
  ) VALUES (?, ?, vector32(?), NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

// vector-search.ts L427 (countVectors by container_tag).
pub const COUNT_VECTORS_BY_TAG: &str =
    r#"SELECT COUNT(*) as count FROM memories WHERE container_tag = ?"#;

// vector-search.ts L434 (countAllVectors).
pub const COUNT_ALL_VECTORS: &str = r#"SELECT COUNT(*) as count FROM memories"#;

pub const COUNT_VECTORS_BY_CONTAINER_TAG: &str =
    r#"SELECT COUNT(*) as count FROM memories WHERE container_tag = ?"#;

// vector-search.ts L385 (getAllMemories).
pub const GET_ALL_MEMORIES: &str = r#"SELECT * FROM memories ORDER BY created_at DESC"#;

// vector-search.ts L439-449 (getDistinctTags, 7-column DISTINCT).
pub const GET_DISTINCT_TAGS: &str = r#"
      SELECT DISTINCT
        container_tag,
        display_name,
        user_name,
        user_email,
        project_path,
        project_name,
        git_repo_url
      FROM memories
    "#;

// vector-search.ts L455-464 (getProjectPathCounts).
pub const GET_PROJECT_PATH_COUNTS: &str = r#"
      SELECT
        project_path AS project_path,
        container_tag AS container_tag,
        COUNT(*) AS cnt
      FROM memories
      WHERE project_path IS NOT NULL AND project_path != ''
      GROUP BY project_path, container_tag
      ORDER BY cnt DESC, project_path ASC
    "#;

// vector-search.ts L483-492 (updateProjectAssociation).
pub const UPDATE_PROJECT_ASSOCIATION: &str = r#"
      UPDATE memories SET
        container_tag = ?,
        project_path = ?,
        project_name = ?,
        display_name = ?,
        git_repo_url = ?
      WHERE container_tag = ?
    "#;

// ---------------------------------------------------------------------------
// W1.3 exact-scan search + pins + re-embed source (blueprint step 2; FROZEN
// after this step — pinned by tests/golden_sql.rs)
// ---------------------------------------------------------------------------

// vector-search.ts L257-263 (exactScanKind, columnName=vector, unfiltered).
pub const SEARCH_EXACT_VECTOR_ALL: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.vector IS NOT NULL
        ORDER BY dist ASC
        LIMIT ?
      "#;

// vector-search.ts L264-270 (exactScanKind, columnName=vector, filtered).
pub const SEARCH_EXACT_VECTOR_TAGGED: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.vector IS NOT NULL AND m.container_tag = ?
        ORDER BY dist ASC
        LIMIT ?
      "#;

// vector-search.ts L257-263 (exactScanKind, columnName=tags_vector, unfiltered).
pub const SEARCH_EXACT_TAGS_VECTOR_ALL: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.tags_vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.tags_vector IS NOT NULL
        ORDER BY dist ASC
        LIMIT ?
      "#;

// vector-search.ts L264-270 (exactScanKind, columnName=tags_vector, filtered).
pub const SEARCH_EXACT_TAGS_VECTOR_TAGGED: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.tags_vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.tags_vector IS NOT NULL AND m.container_tag = ?
        ORDER BY dist ASC
        LIMIT ?
      "#;

/// Upstream hydration select (vector-search.ts L127-149): 15 columns +
/// `content_dist`/`tags_dist` CASE, `WHERE id IN (<id_count × '?'>)`
/// [+ `AND container_tag = ?`]. Ids are BOUND PARAMS (count-clamped to
/// 1000; never interpolated values).
pub fn hydrate_candidates_sql(id_count: usize, filtered: bool) -> String {
    let count = id_count.min(1000);
    let placeholders = vec!["?"; count].join(", ");
    if filtered {
        format!(
            r#"SELECT id, content, tags, created_at, metadata, container_tag,
             display_name, user_name, user_email, project_path, project_name,
             git_repo_url, is_pinned, session_id,
             vector_distance_cos(vector, vector32(?)) AS content_dist,
             CASE WHEN tags_vector IS NOT NULL
               THEN vector_distance_cos(tags_vector, vector32(?))
               ELSE NULL END AS tags_dist
      FROM memories
      WHERE id IN ({placeholders}) AND container_tag = ?
    "#
        )
    } else {
        format!(
            r#"SELECT id, content, tags, created_at, metadata, container_tag,
             display_name, user_name, user_email, project_path, project_name,
             git_repo_url, is_pinned, session_id,
             vector_distance_cos(vector, vector32(?)) AS content_dist,
             CASE WHEN tags_vector IS NOT NULL
               THEN vector_distance_cos(tags_vector, vector32(?))
               ELSE NULL END AS tags_dist
      FROM memories
      WHERE id IN ({placeholders})
    "#
        )
    }
}

// vector-search.ts L227-232 (keywordScores, unfiltered).
pub const KEYWORD_SCORES_ALL: &str = r#"
          SELECT id FROM memories
          WHERE content LIKE ? ESCAPE '\' OR IFNULL(tags, '') LIKE ? ESCAPE '\'
          LIMIT ?
        "#;

// vector-search.ts L233-238 (keywordScores, filtered).
pub const KEYWORD_SCORES_TAGGED: &str = r#"
          SELECT id FROM memories
          WHERE container_tag = ?
            AND (content LIKE ? ESCAPE '\' OR IFNULL(tags, '') LIKE ? ESCAPE '\')
          LIMIT ?
        "#;

// vector-search.ts L410-417 (getMemoriesBySessionID).
pub const GET_MEMORIES_BY_SESSION_ID: &str = r#"
      SELECT * FROM memories
      WHERE session_id = ?
         OR (session_id IS NULL AND metadata LIKE ?)
      ORDER BY created_at DESC
    "#;

// vector-search.ts L505 (pinMemory).
pub const PIN_MEMORY: &str = r#"UPDATE memories SET is_pinned = 1 WHERE id = ?"#;

// vector-search.ts L509 (unpinMemory).
pub const UNPIN_MEMORY: &str = r#"UPDATE memories SET is_pinned = 0 WHERE id = ?"#;

// migration-service.ts L269-275 (rebuildShardSafely re-embed source select).
pub const SELECT_REEMBED_SOURCE: &str = r#"
      SELECT
        id, content, container_tag, tags, type, created_at, updated_at, metadata,
        display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned
      FROM memories
      ORDER BY created_at DESC
    "#;

// legacy-migrator.ts L172-174 (shard_metadata value probe; Rust generalizes
// the key to a bound param).
pub const GET_SHARD_METADATA_VALUE: &str = r#"SELECT value FROM shard_metadata WHERE key = ?"#;

// legacy-migrator.ts L360 (SELECT * FROM memories, unordered source read).
pub const GET_MEMORIES_UNORDERED: &str = r#"SELECT * FROM memories"#;

// shard-manager.ts L129-135 (getActiveShard).
pub const GET_ACTIVE_SHARD: &str = r#"
      SELECT * FROM shards
      WHERE scope = ? AND scope_hash = ? AND is_active = 1
      ORDER BY shard_index DESC LIMIT 1
    "#;

// shard-manager.ts L146-152 (getAllShards, empty hash wildcard).
pub const GET_ALL_SHARDS_FOR_SCOPE: &str = r#"
          SELECT * FROM shards
          WHERE scope = ?
          ORDER BY shard_index ASC
        "#;

// shard-manager.ts L154-161 (getAllShards, scoped).
pub const GET_ALL_SHARDS_FOR_SCOPE_HASH: &str = r#"
          SELECT * FROM shards
          WHERE scope = ? AND scope_hash = ?
          ORDER BY shard_index ASC
        "#;

// shard-manager.ts L186-190 (createShard INSERT).
pub const CREATE_SHARD: &str = r#"
      INSERT INTO shards (scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at)
      VALUES (?, ?, ?, ?, 0, 1, ?)
    "#;

// shard-manager.ts L194-199 (createShard unique-violation re-fetch).
pub const FIND_SHARD_BY_SCOPE_INDEX: &str = r#"
          SELECT * FROM shards
          WHERE scope = ? AND scope_hash = ? AND shard_index = ?
        "#;

// shard-manager.ts L234-243 (registerExistingShard upsert `ON CONFLICT(...) DO UPDATE`).
pub const REGISTER_EXISTING_SHARD: &str = r#"
        INSERT INTO shards (
          scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(scope, scope_hash, shard_index) DO UPDATE SET
          db_path = excluded.db_path,
          vector_count = excluded.vector_count,
          is_active = excluded.is_active
      "#;

// shard-manager.ts L247 (registerExistingShard re-fetch + createShard refetch shortcut consumers).
pub const FIND_SHARD_BY_KEY: &str =
    r#"SELECT * FROM shards WHERE scope = ? AND scope_hash = ? AND shard_index = ?"#;

// shard-manager.ts L266-271 (initShardDb dims `INSERT OR REPLACE`).
pub const SHARD_METADATA_INSERT_DIMENSIONS: &str = r#"
          INSERT OR REPLACE INTO shard_metadata (key, value)
          VALUES ('embedding_dimensions', ?)
        "#;

// shard-manager.ts L273-279 (initShardDb model).
pub const SHARD_METADATA_INSERT_MODEL: &str = r#"
          INSERT OR REPLACE INTO shard_metadata (key, value)
          VALUES ('embedding_model', ?)
        "#;

// shard-manager.ts L305 (hasMatchingEmbeddingDimensions).
pub const SHARD_METADATA_SELECT_DIMENSIONS: &str =
    r#"SELECT value FROM shard_metadata WHERE key = 'embedding_dimensions'"#;

// shard-manager.ts L429 (setVectorCount; also L338 syncShardVectorCount).
pub const SET_VECTOR_COUNT: &str = r#"UPDATE shards SET vector_count = ? WHERE id = ?"#;

// shard-manager.ts L350-352 (isShardValid memories-table probe).
pub const SQLITE_MASTER_MEMORIES_TABLE: &str =
    r#"SELECT name FROM sqlite_master WHERE type='table' AND name='memories'"#;

// shard-manager.ts L406 (markShardReadOnly).
pub const MARK_SHARD_INACTIVE: &str = r#"UPDATE shards SET is_active = 0 WHERE id = ?"#;

// shard-manager.ts L411 (incrementVectorCount).
pub const INCREMENT_VECTOR_COUNT: &str =
    r#"UPDATE shards SET vector_count = vector_count + 1 WHERE id = ?"#;

// shard-manager.ts L418-421 (decrementVectorCount, `AND vector_count > 0`).
pub const DECREMENT_VECTOR_COUNT: &str =
    r#"UPDATE shards SET vector_count = vector_count - 1 WHERE id = ? AND vector_count > 0"#;

// shard-manager.ts L434 (getShardById; deletes/archive use the same statement inline).
pub const GET_SHARD_BY_ID: &str = r#"SELECT * FROM shards WHERE id = ?"#;

// shard-manager.ts L446 (getShardByPath, `LIKE '%/' || ? ESCAPE '\'`).
pub const GET_SHARD_BY_PATH: &str =
    r#"SELECT * FROM shards WHERE db_path LIKE '%/' || ? ESCAPE '\'"#;

// shard-manager.ts L473 (deleteShard; archiveShard L499 same statement).
pub const DELETE_SHARD_ROW: &str = r#"DELETE FROM shards WHERE id = ?"#;

// shard-manager.ts L531-536 (reassignShardScope).
pub const REASSIGN_SHARD_SCOPE: &str = r#"
      UPDATE shards
      SET scope_hash = ?, db_path = ?, vector_count = ?, is_active = ?
      WHERE id = ?
    "#;

// shard-path-migration-service.ts L299-312 (staged-shard cleanup UPDATE, `WHERE container_tag != ?`).
pub const UPDATE_MEMORIES_PROJECT_ASSOCIATION: &str = r#"UPDATE memories SET
             container_tag = ?, project_path = ?, project_name = ?,
             display_name = ?, git_repo_url = ?
           WHERE container_tag != ?"#;
// ---------------------------------------------------------------------------
// PRAGMA helpers
// ---------------------------------------------------------------------------

/// `getUserVersion` probe (schema-migrations.ts L14) — classified READ by
/// `is_read_query` (bare PRAGMA without `=`/`;`).
pub const PRAGMA_USER_VERSION: &str = "PRAGMA user_version";

/// `setUserVersion` (schema-migrations.ts L18-20): `PRAGMA user_version = N`
/// — an assignment, therefore classified WRITE.
pub fn set_user_version_sql(version: i64) -> String {
    format!("PRAGMA user_version = {version}")
}

/// `PRAGMA foreign_keys = ON` — classified WRITE (assignment). The
/// connection manager runs it on every new handle.
pub const PRAGMA_FOREIGN_KEYS_ON: &str = "PRAGMA foreign_keys = ON";

// ---------------------------------------------------------------------------
// is_read_query — port of upstream `turso-db.ts` L24-26
// ---------------------------------------------------------------------------

/// Upstream `/^\s*(SELECT|WITH)\b/i.test(sql) ||
/// /^\s*PRAGMA\s+[^=;]+$/i.test(sql.trim())`, hand-rolled (no regex crate):
/// a statement counts as READ when it (ignoring leading whitespace and
/// ASCII case) starts with SELECT or WITH followed by a non-identifier
/// character, or when its TRIMMED form is PRAGMA + whitespace + a body
/// holding no `=` and no `;` (an assignment or multi-statement PRAGMA is a
/// WRITE).
pub fn is_read_query(sql: &str) -> bool {
    let trimmed_start = sql.trim_start_matches(WHITESPACE);
    if starts_with_keyword(trimmed_start, "SELECT") || starts_with_keyword(trimmed_start, "WITH") {
        return true;
    }

    // PRAGMA branch: upstream applies `/^\s*PRAGMA\s+[^=;]+$/i` to the TRIMMED string.
    let trimmed = sql.trim();
    let lower = trimmed.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("pragma") else {
        return false;
    };
    // At least one whitespace character must separate PRAGMA from its body
    // (`pragmaX` / bare `pragma` cannot match `PRAGMA\s+`).
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_whitespace() {
        return false;
    }
    let body = rest.trim_start_matches(WHITESPACE);
    !body.is_empty() && !body.contains('=') && !body.contains(';')
}

const WHITESPACE: &[char] = &[' ', '\t', '\n', '\r', '\u{0B}', '\u{0C}'];

/// ASCII-case-insensitive keyword check with the upstream `\b` word boundary:
/// `SELECTX` and `WHERE` must not match, `  select x` must.
fn starts_with_keyword(text: &str, keyword: &str) -> bool {
    if text.len() < keyword.len() {
        return false;
    }
    let head = &text[..keyword.len()];
    let next = text[keyword.len()..].chars().next();
    head.eq_ignore_ascii_case(keyword)
        && next
            .map(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .unwrap_or(true)
}
