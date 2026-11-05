//! Verbatim-SQL golden battery (blueprint W1.2 — D2 golden rule).
//!
//! Upstream SQL statements must run on `turso` **unchanged**: every library
//! constant in [`mem_store::sql`] is pinned byte-for-byte against the frozen
//! copies in [`frozen`] (cut from the cited upstream template literals), and
//! every executable statement is proven to run on turso 0.8.3 against a real
//! temporary database. A mismatch is a blocking defect.

mod common;
mod frozen {
    #![allow(dead_code)]

    // Frozen upstream SQL literals cut byte-for-byte from the cited upstream
    // template literals (after JS-unescaping — the exact runtime strings).
    // Regenerate with /tmp/opencode/gen_freeze.py; never edit by hand.
    pub const FROZEN_BLOCK_START: () = ();

    /// schema-migrations.ts parse L88-93 (memoryShardMigrations v1 stmt 1)
    pub const FROZEN_SHARD_METADATA_DDL: &str = r#"
            CREATE TABLE IF NOT EXISTS shard_metadata (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            )
          "#;

    /// schema-migrations.ts L96-116 with F32_BLOB(${dims}) dims=1024
    pub const FROZEN_MEMORIES_DDL_1024: &str = r#"
            CREATE TABLE IF NOT EXISTS memories (
              id TEXT PRIMARY KEY,
              content TEXT NOT NULL,
              vector F32_BLOB(1024) NOT NULL,
              tags_vector F32_BLOB(1024),
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
          "#;

    /// schema-migrations.ts L118
    pub const FROZEN_MEMORIES_INDEX_CONTAINER_TAG: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_container_tag ON memories(container_tag)"#;

    /// schema-migrations.ts L119
    pub const FROZEN_MEMORIES_INDEX_TYPE: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_type ON memories(type)"#;

    /// schema-migrations.ts L120
    pub const FROZEN_MEMORIES_INDEX_CREATED_AT: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_created_at ON memories(created_at DESC)"#;

    /// schema-migrations.ts L121
    pub const FROZEN_MEMORIES_INDEX_IS_PINNED: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_is_pinned ON memories(is_pinned)"#;

    /// schema-migrations.ts L129 (memoryShardMigrations v2 stmt 1)
    pub const FROZEN_SESSION_ID_ALTER: &str = r#"ALTER TABLE memories ADD COLUMN session_id TEXT"#;

    /// schema-migrations.ts L130 (v2 stmt 2)
    pub const FROZEN_SESSION_ID_INDEX: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_session_id ON memories(session_id)"#;

    /// schema-migrations.ts L132-138 (v2 stmt 3, `json_extract(metadata, '$.sessionID')`)
    pub const FROZEN_SESSION_ID_BACKFILL: &str = r#"
            UPDATE memories
            SET session_id = json_extract(metadata, '$.sessionID')
            WHERE metadata IS NOT NULL
              AND session_id IS NULL
              AND json_extract(metadata, '$.sessionID') IS NOT NULL
          "#;

    /// schema-migrations.ts L151-163 (METADATA_DB_MIGRATIONS v1)
    pub const FROZEN_SHARDS_DDL: &str = r#"
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

    /// schema-migrations.ts L166-170
    pub const FROZEN_SHARDS_ACTIVE_INDEX: &str = r#"
          CREATE INDEX IF NOT EXISTS idx_active_shards
          ON shards(scope, scope_hash, is_active)
        "#;

    /// schema-migrations.ts L181-196 (USER_PROMPTS_MIGRATIONS v1)
    pub const FROZEN_USER_PROMPTS_DDL: &str = r#"
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

    /// schema-migrations.ts L198-209 (idx_user_prompts_session)
    pub const FROZEN_IDX_USER_PROMPTS_SESSION: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_session ON user_prompts(session_id)"#;

    /// schema-migrations.ts L198-209 (idx_user_prompts_captured)
    pub const FROZEN_IDX_USER_PROMPTS_CAPTURED: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_captured ON user_prompts(captured)"#;

    /// schema-migrations.ts L198-209 (idx_user_prompts_created)
    pub const FROZEN_IDX_USER_PROMPTS_CREATED: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_created ON user_prompts(created_at DESC)"#;

    /// schema-migrations.ts L198-209 (idx_user_prompts_project)
    pub const FROZEN_IDX_USER_PROMPTS_PROJECT: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_project ON user_prompts(project_path)"#;

    /// schema-migrations.ts L198-209 (idx_user_prompts_linked)
    pub const FROZEN_IDX_USER_PROMPTS_LINKED: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_linked ON user_prompts(linked_memory_id)"#;

    /// schema-migrations.ts L198-209 (idx_user_prompts_user_learning)
    pub const FROZEN_IDX_USER_PROMPTS_USER_LEARNING: &str = r#"CREATE INDEX IF NOT EXISTS idx_user_prompts_user_learning ON user_prompts(user_learning_captured)"#;

    /// schema-migrations.ts L218-232 (USER_PROFILES_MIGRATIONS v1 stmt 1)
    pub const FROZEN_USER_PROFILES_DDL: &str = r#"
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

    /// schema-migrations.ts L234-245 (USER_PROFILES_MIGRATIONS v1 stmt 2)
    pub const FROZEN_USER_PROFILE_CHANGELOGS_DDL: &str = r#"
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

    /// schema-migrations.ts L246-255 (idx_user_profiles_user_id)
    pub const FROZEN_IDX_USER_PROFILES_USER_ID: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_profiles_user_id ON user_profiles(user_id)"#;

    /// schema-migrations.ts L246-255 (idx_user_profiles_is_active)
    pub const FROZEN_IDX_USER_PROFILES_IS_ACTIVE: &str =
        r#"CREATE INDEX IF NOT EXISTS idx_user_profiles_is_active ON user_profiles(is_active)"#;

    /// schema-migrations.ts L246-255 (idx_user_profile_changelogs_profile_id)
    pub const FROZEN_IDX_USER_PROFILE_CHANGELOGS_PROFILE_ID: &str = r#"CREATE INDEX IF NOT EXISTS idx_user_profile_changelogs_profile_id ON user_profile_changelogs(profile_id)"#;

    /// schema-migrations.ts L246-255 (idx_user_profile_changelogs_version)
    pub const FROZEN_IDX_USER_PROFILE_CHANGELOGS_VERSION: &str = r#"CREATE INDEX IF NOT EXISTS idx_user_profile_changelogs_version ON user_profile_changelogs(version DESC)"#;

    /// vector-search.ts L50-55
    pub const FROZEN_INSERT_WITH_TAGS: &str = r#"
  INSERT INTO memories (
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
    metadata, session_id, display_name, user_name, user_email, project_path, project_name, git_repo_url
  ) VALUES (?, ?, vector32(?), vector32(?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

    /// vector-search.ts L57-62
    pub const FROZEN_INSERT_WITHOUT_TAGS: &str = r#"
  INSERT INTO memories (
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
    metadata, session_id, display_name, user_name, user_email, project_path, project_name, git_repo_url
  ) VALUES (?, ?, vector32(?), NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

    /// vector-search.ts L385
    pub const FROZEN_GET_ALL_MEMORIES: &str = r#"SELECT * FROM memories ORDER BY created_at DESC"#;

    /// vector-search.ts L434
    pub const FROZEN_COUNT_ALL_VECTORS: &str = r#"SELECT COUNT(*) as count FROM memories"#;

    /// vector-search.ts L427
    pub const FROZEN_COUNT_VECTORS_BY_TAG: &str =
        r#"SELECT COUNT(*) as count FROM memories WHERE container_tag = ?"#;

    /// vector-search.ts L439-449
    pub const FROZEN_GET_DISTINCT_TAGS: &str = r#"
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

    /// vector-search.ts L455-464
    pub const FROZEN_GET_PROJECT_PATH_COUNTS: &str = r#"
      SELECT
        project_path AS project_path,
        container_tag AS container_tag,
        COUNT(*) AS cnt
      FROM memories
      WHERE project_path IS NOT NULL AND project_path != ''
      GROUP BY project_path, container_tag
      ORDER BY cnt DESC, project_path ASC
    "#;

    /// vector-search.ts L483-492
    pub const FROZEN_UPDATE_PROJECT_ASSOCIATION: &str = r#"
      UPDATE memories SET
        container_tag = ?,
        project_path = ?,
        project_name = ?,
        display_name = ?,
        git_repo_url = ?
      WHERE container_tag = ?
    "#;

    /// shard-manager.ts L129-135
    pub const FROZEN_GET_ACTIVE_SHARD: &str = r#"
      SELECT * FROM shards
      WHERE scope = ? AND scope_hash = ? AND is_active = 1
      ORDER BY shard_index DESC LIMIT 1
    "#;

    /// shard-manager.ts L146-152
    pub const FROZEN_GET_ALL_SHARDS_FOR_SCOPE: &str = r#"
          SELECT * FROM shards
          WHERE scope = ?
          ORDER BY shard_index ASC
        "#;

    /// shard-manager.ts L154-161
    pub const FROZEN_GET_ALL_SHARDS_FOR_SCOPE_HASH: &str = r#"
          SELECT * FROM shards
          WHERE scope = ? AND scope_hash = ?
          ORDER BY shard_index ASC
        "#;

    /// shard-manager.ts L186-190
    pub const FROZEN_CREATE_SHARD: &str = r#"
      INSERT INTO shards (scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at)
      VALUES (?, ?, ?, ?, 0, 1, ?)
    "#;

    /// shard-manager.ts L194-199
    pub const FROZEN_FIND_SHARD_BY_SCOPE_INDEX: &str = r#"
          SELECT * FROM shards
          WHERE scope = ? AND scope_hash = ? AND shard_index = ?
        "#;

    /// shard-manager.ts L234-243 (upsert ON CONFLICT ... DO UPDATE)
    pub const FROZEN_REGISTER_EXISTING_SHARD: &str = r#"
        INSERT INTO shards (
          scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(scope, scope_hash, shard_index) DO UPDATE SET
          db_path = excluded.db_path,
          vector_count = excluded.vector_count,
          is_active = excluded.is_active
      "#;

    /// shard-manager.ts L247
    pub const FROZEN_FIND_SHARD_BY_KEY: &str =
        r#"SELECT * FROM shards WHERE scope = ? AND scope_hash = ? AND shard_index = ?"#;

    /// shard-manager.ts L267-271 — frozen with W2's D-16 divergence
    /// (`INSERT OR IGNORE`: the identity keys are write-once; ADR-0018).
    pub const FROZEN_SHARD_METADATA_INSERT_DIMENSIONS: &str = r#"
          INSERT OR IGNORE INTO shard_metadata (key, value)
          VALUES ('embedding_dimensions', ?)
        "#;

    /// shard-manager.ts L274-278
    pub const FROZEN_SHARD_METADATA_INSERT_MODEL: &str = r#"
          INSERT OR IGNORE INTO shard_metadata (key, value)
          VALUES ('embedding_model', ?)
        "#;

    /// shard-manager.ts L305
    pub const FROZEN_SHARD_METADATA_SELECT_DIMENSIONS: &str =
        r#"SELECT value FROM shard_metadata WHERE key = 'embedding_dimensions'"#;

    /// shard-manager.ts L406
    pub const FROZEN_MARK_SHARD_INACTIVE: &str = r#"UPDATE shards SET is_active = 0 WHERE id = ?"#;

    /// shard-manager.ts L429 (also L338 sync)
    pub const FROZEN_SET_VECTOR_COUNT: &str = r#"UPDATE shards SET vector_count = ? WHERE id = ?"#;

    /// shard-manager.ts L434
    pub const FROZEN_GET_SHARD_BY_ID: &str = r#"SELECT * FROM shards WHERE id = ?"#;

    /// shard-manager.ts L446 (`LIKE '%/' || ? ESCAPE '\'`)
    pub const FROZEN_GET_SHARD_BY_PATH: &str =
        r#"SELECT * FROM shards WHERE db_path LIKE '%/' || ? ESCAPE '\'"#;

    /// shard-manager.ts L473
    pub const FROZEN_DELETE_SHARD_ROW: &str = r#"DELETE FROM shards WHERE id = ?"#;

    /// shard-manager.ts L531-536
    pub const FROZEN_REASSIGN_SHARD_SCOPE: &str = r#"
      UPDATE shards
      SET scope_hash = ?, db_path = ?, vector_count = ?, is_active = ?
      WHERE id = ?
    "#;

    /// shard-manager.ts L351
    pub const FROZEN_SQLITE_MASTER_MEMORIES: &str =
        r#"SELECT name FROM sqlite_master WHERE type='table' AND name='memories'"#;

    /// shard-path-migration-service.ts L299-312 (`WHERE container_tag != ?`)
    pub const FROZEN_UPDATE_MEMORIES_PROJECT_ASSOCIATION: &str = r#"UPDATE memories SET
             container_tag = ?, project_path = ?, project_name = ?,
             display_name = ?, git_repo_url = ?
           WHERE container_tag != ?"#;

    /// vector-search.ts L257-263 (exactScanKind, columnName=vector, unfiltered)
    pub const FROZEN_SEARCH_EXACT_VECTOR_ALL: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.vector IS NOT NULL
        ORDER BY dist ASC
        LIMIT ?
      "#;

    /// vector-search.ts L264-270 (exactScanKind, columnName=vector, filtered)
    pub const FROZEN_SEARCH_EXACT_VECTOR_TAGGED: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.vector IS NOT NULL AND m.container_tag = ?
        ORDER BY dist ASC
        LIMIT ?
      "#;

    /// vector-search.ts L257-263 (exactScanKind, columnName=tags_vector, unfiltered)
    pub const FROZEN_SEARCH_EXACT_TAGS_VECTOR_ALL: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.tags_vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.tags_vector IS NOT NULL
        ORDER BY dist ASC
        LIMIT ?
      "#;

    /// vector-search.ts L264-270 (exactScanKind, columnName=tags_vector, filtered)
    pub const FROZEN_SEARCH_EXACT_TAGS_VECTOR_TAGGED: &str = r#"
        SELECT m.id AS id, vector_distance_cos(m.tags_vector, vector32(?)) AS dist
        FROM memories m
        WHERE m.tags_vector IS NOT NULL AND m.container_tag = ?
        ORDER BY dist ASC
        LIMIT ?
      "#;

    /// vector-search.ts L227-232 (keywordScores, unfiltered)
    pub const FROZEN_KEYWORD_SCORES_ALL: &str = r#"
          SELECT id FROM memories
          WHERE content LIKE ? ESCAPE '\' OR IFNULL(tags, '') LIKE ? ESCAPE '\'
          LIMIT ?
        "#;

    /// vector-search.ts L233-238 (keywordScores, filtered)
    pub const FROZEN_KEYWORD_SCORES_TAGGED: &str = r#"
          SELECT id FROM memories
          WHERE container_tag = ?
            AND (content LIKE ? ESCAPE '\' OR IFNULL(tags, '') LIKE ? ESCAPE '\')
          LIMIT ?
        "#;

    /// vector-search.ts L410-417 (getMemoriesBySessionID)
    pub const FROZEN_GET_MEMORIES_BY_SESSION_ID: &str = r#"
      SELECT * FROM memories
      WHERE session_id = ?
         OR (session_id IS NULL AND metadata LIKE ?)
      ORDER BY created_at DESC
    "#;

    /// vector-search.ts L505 (pinMemory)
    pub const FROZEN_PIN_MEMORY: &str = r#"UPDATE memories SET is_pinned = 1 WHERE id = ?"#;

    /// vector-search.ts L509 (unpinMemory)
    pub const FROZEN_UNPIN_MEMORY: &str = r#"UPDATE memories SET is_pinned = 0 WHERE id = ?"#;

    /// migration-service.ts L269-275 — frozen with Issue 127 tracked divergence:
    /// upstream TypeScript omitted `session_id` because it fell back to `metadata.sessionID`
    /// parsing; Rust port requires `session_id` directly in `SELECT_REEMBED_SOURCE` to preserve
    /// tier-1 session associations across re-embedding when rows have direct `session_id`
    /// without redundant `metadata` JSON mirroring.
    pub const FROZEN_SELECT_REEMBED_SOURCE: &str = r#"
      SELECT
        id, content, container_tag, tags, type, created_at, updated_at, metadata,
        display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned,
        session_id
      FROM memories
      ORDER BY created_at DESC
    "#;

    /// legacy-migrator.ts L360 (SELECT * FROM memories, unordered source read)
    pub const FROZEN_GET_MEMORIES_UNORDERED: &str = r#"SELECT * FROM memories"#;

    /// legacy-migrator.ts L172-174 (shard_metadata value probe; Rust generalizes the key to a bound param)
    pub const FROZEN_GET_SHARD_METADATA_VALUE: &str =
        r#"SELECT value FROM shard_metadata WHERE key = ?"#;
}

use mem_store::config::{Platform, StoreConfig};
use mem_store::db::{BatchMode, ConnectFactory, ConnectOptions, Db, DbHandle, Value};
use mem_store::sql;
use mem_store::sql::set_user_version_sql;
use mem_store::turso_connect_factory;
use mem_store::vector_utils::escape_like_pattern;

/// A hermetic `StoreConfig` with encryption disabled over `dir`.
fn test_config(dir: &std::path::Path, dims: u32) -> StoreConfig {
    StoreConfig {
        storage_path: dir.to_path_buf(),
        embedding_dimensions: dims,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// Opens a real turso 0.8.3 database through the production connect factory.
async fn open_db(config: &StoreConfig, name: &str) -> DbHandle {
    let factory = turso_connect_factory();
    let opts = ConnectOptions::for_config(config, Platform::current());
    factory
        .connect(config.storage_path.join(name), opts)
        .await
        .expect("open test database")
}

/// Executes a whole statement array as one BatchMode::Write batch.
async fn batch_write(db: &dyn Db, stmts: Vec<&str>) {
    let statements = stmts
        .iter()
        .map(|s| mem_store::db::Statement {
            sql: s.to_string(),
            params: vec![],
        })
        .collect();
    db.batch(statements, BatchMode::Write)
        .await
        .expect("batch write");
}

async fn column_names(db: &dyn Db, table: &str) -> Vec<String> {
    let pragma = format!("PRAGMA table_info({table})");
    db.query(&pragma, vec![])
        .await
        .expect("table_info")
        .into_iter()
        .filter_map(|row| row.text("name"))
        .collect()
}

fn text(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}

// ---------------------------------------------------------------------------
// 1/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_memories_ddl_v1_executes_and_matches_library() {
    let dir = common::temp_dir("golden-mem-ddl");
    let config = test_config(&dir, 1024);
    let db = open_db(&config, "shard.db").await;

    assert_eq!(sql::shard_metadata_ddl(), frozen::FROZEN_SHARD_METADATA_DDL);
    assert_eq!(sql::memories_ddl(1024), frozen::FROZEN_MEMORIES_DDL_1024);

    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(1024)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;
    batch_write(db.as_ref(), sql::memories_indexes().to_vec()).await;

    let names = column_names(db.as_ref(), "memories").await;
    assert_eq!(
        names,
        vec![
            "id",
            "content",
            "vector",
            "tags_vector",
            "container_tag",
            "tags",
            "type",
            "created_at",
            "updated_at",
            "metadata",
            "display_name",
            "user_name",
            "user_email",
            "project_path",
            "project_name",
            "git_repo_url",
            "is_pinned",
            "session_id",
        ]
    );
}

// ---------------------------------------------------------------------------
// 2/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_shards_registry_ddl_executes_and_matches_library() {
    let dir = common::temp_dir("golden-shards-ddl");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "metadata-shape.db").await;

    assert_eq!(sql::SHARDS_DDL, frozen::FROZEN_SHARDS_DDL);
    assert_eq!(sql::SHARDS_ACTIVE_INDEX, frozen::FROZEN_SHARDS_ACTIVE_INDEX);

    batch_write(db.as_ref(), vec![sql::SHARDS_DDL, sql::SHARDS_ACTIVE_INDEX]).await;

    let insert = sql::CREATE_SHARD;
    let args = |index: i64| {
        vec![
            text("user"),
            text("a1b2c3d4e5f60001"),
            Value::Integer(index),
            text("users/user_a1b2c3d4e5f60001_shard_0.db"),
            Value::Integer(1700000000000),
        ]
    };
    db.execute(insert, args(0)).await.expect("first insert");
    let duplicate = db.execute(insert, args(0)).await;
    let error = duplicate.expect_err("duplicate (scope, scope_hash, shard_index) insert must fail");
    assert!(
        error.is_unique_violation(),
        "expected unique violation, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// 3/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_user_prompts_ddl_executes_and_matches_library() {
    let dir = common::temp_dir("golden-prompts-ddl");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "user-prompts.db").await;

    assert_eq!(sql::USER_PROMPTS_DDL, frozen::FROZEN_USER_PROMPTS_DDL);
    let indexes = sql::user_prompts_indexes();
    assert_eq!(indexes[0], frozen::FROZEN_IDX_USER_PROMPTS_SESSION);
    assert_eq!(indexes[1], frozen::FROZEN_IDX_USER_PROMPTS_CAPTURED);
    assert_eq!(indexes[2], frozen::FROZEN_IDX_USER_PROMPTS_CREATED);
    assert_eq!(indexes[3], frozen::FROZEN_IDX_USER_PROMPTS_PROJECT);
    assert_eq!(indexes[4], frozen::FROZEN_IDX_USER_PROMPTS_LINKED);
    assert_eq!(indexes[5], frozen::FROZEN_IDX_USER_PROMPTS_USER_LEARNING);

    batch_write(db.as_ref(), vec![sql::USER_PROMPTS_DDL]).await;
    batch_write(db.as_ref(), indexes.to_vec()).await;

    let names = column_names(db.as_ref(), "user_prompts").await;
    assert_eq!(
        names,
        vec![
            "id",
            "session_id",
            "message_id",
            "project_path",
            "content",
            "created_at",
            "captured",
            "user_learning_captured",
            "linked_memory_id",
            "capture_attempts",
            "provider_id",
            "model_id",
        ]
    );
}

// ---------------------------------------------------------------------------
// 3b/12 (W3.1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_user_profiles_ddl_executes_and_matches_library() {
    let dir = common::temp_dir("golden-profiles-ddl");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "user-profiles.db").await;

    assert_eq!(sql::USER_PROFILES_DDL, frozen::FROZEN_USER_PROFILES_DDL);
    assert_eq!(
        sql::USER_PROFILE_CHANGELOGS_DDL,
        frozen::FROZEN_USER_PROFILE_CHANGELOGS_DDL
    );
    let indexes = sql::user_profiles_indexes();
    assert_eq!(indexes[0], frozen::FROZEN_IDX_USER_PROFILES_USER_ID);
    assert_eq!(indexes[1], frozen::FROZEN_IDX_USER_PROFILES_IS_ACTIVE);
    assert_eq!(
        indexes[2],
        frozen::FROZEN_IDX_USER_PROFILE_CHANGELOGS_PROFILE_ID
    );
    assert_eq!(
        indexes[3],
        frozen::FROZEN_IDX_USER_PROFILE_CHANGELOGS_VERSION
    );

    batch_write(
        db.as_ref(),
        mem_store::schema_migrations::user_profiles_migrations()
            .into_iter()
            .flat_map(|migration| migration.statements)
            .map(|statement| statement.sql)
            .collect::<Vec<String>>()
            .iter()
            .map(String::as_str)
            .collect(),
    )
    .await;

    let profile_columns = column_names(db.as_ref(), "user_profiles").await;
    assert_eq!(
        profile_columns,
        vec![
            "id",
            "user_id",
            "display_name",
            "user_name",
            "user_email",
            "profile_data",
            "version",
            "created_at",
            "last_analyzed_at",
            "total_prompts_analyzed",
            "is_active",
        ]
    );
    let changelog_columns = column_names(db.as_ref(), "user_profile_changelogs").await;
    assert_eq!(
        changelog_columns,
        vec![
            "id",
            "profile_id",
            "version",
            "change_type",
            "change_summary",
            "profile_data_snapshot",
            "created_at",
        ]
    );
}

// ---------------------------------------------------------------------------
// 4/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_insert_with_tags_roundtrips_vector32_and_extract() {
    let dir = common::temp_dir("golden-insert-tags");
    let config = test_config(&dir, 3);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(3)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    assert_eq!(sql::INSERT_WITH_TAGS, frozen::FROZEN_INSERT_WITH_TAGS);
    let content_vector = serde_json::to_string(&vec![1.0f32, 0.5, -2.25]).unwrap();
    let tags_vector = serde_json::to_string(&vec![0.0f32, 1.0, 2.0]).unwrap();
    db.execute(
        sql::INSERT_WITH_TAGS,
        vec![
            text("mem_tags_1"),
            text("tagged memory"),
            text(&content_vector),
            text(&tags_vector),
            text("opencode_project_a1b2c3d4e5f60001"),
            text("[\"alpha\"]"),
            text("project"),
            Value::Integer(100),
            Value::Integer(100),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ],
    )
    .await
    .expect("insert with tags");

    let rows = db
        .query(
            "SELECT id, vector, tags_vector, vector_extract(vector) AS extracted, \
             vector_extract(tags_vector) AS extracted_tags FROM memories",
            vec![],
        )
        .await
        .expect("extract query");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let extracted = row.text("extracted").expect("vector_extract text");
    let parsed: Vec<f32> = serde_json::from_str(&extracted).expect("valid JSON vector");
    assert_eq!(parsed, vec![1.0f32, 0.5, -2.25]);
    let extracted_tags = row
        .text("extracted_tags")
        .expect("tags vector_extract text");
    let parsed_tags: Vec<f32> = serde_json::from_str(&extracted_tags).unwrap();
    assert_eq!(parsed_tags, vec![0.0f32, 1.0, 2.0]);
    assert!(!row.is_null("tags_vector"), "tags_vector must be stored");
}

// ---------------------------------------------------------------------------
// 5/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_insert_without_tags_leaves_tags_vector_null() {
    let dir = common::temp_dir("golden-insert-notags");
    let config = test_config(&dir, 3);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(3)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    assert_eq!(sql::INSERT_WITHOUT_TAGS, frozen::FROZEN_INSERT_WITHOUT_TAGS);
    let args = || {
        vec![
            text("mem_plain_1"),
            text("untagged memory"),
            text(serde_json::to_string(&vec![1.0f32, 0.5, -2.25]).unwrap()),
            text("opencode_project_a1b2c3d4e5f60001"),
            Value::Null,
            Value::Null,
            Value::Integer(100),
            Value::Integer(100),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ]
    };
    db.execute(sql::INSERT_WITHOUT_TAGS, args())
        .await
        .expect("insert without tags");

    let rows = db
        .query(
            "SELECT vector, tags_vector, vector_extract(vector) AS extracted FROM memories",
            vec![],
        )
        .await
        .expect("query");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!(row.is_null("tags_vector"), "tags_vector must stay NULL");
    let extracted = row.text("extracted").expect("extract");
    let parsed: Vec<f32> = serde_json::from_str(&extracted).unwrap();
    assert_eq!(parsed, vec![1.0f32, 0.5, -2.25]);

    let duplicate = db
        .execute(sql::INSERT_WITHOUT_TAGS, args())
        .await
        .expect_err("duplicate primary key must fail");
    assert!(
        duplicate.is_unique_violation(),
        "expected unique violation: {duplicate}"
    );
}

// ---------------------------------------------------------------------------
// 6/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_session_id_backfill_extracts_metadata_session_id() {
    let dir = common::temp_dir("golden-session-backfill");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(2)],
    )
    .await;

    assert_eq!(sql::MEMORY_SHARD_V2_ALTER, frozen::FROZEN_SESSION_ID_ALTER);
    assert_eq!(sql::MEMORY_SHARD_V2_INDEX, frozen::FROZEN_SESSION_ID_INDEX);
    assert_eq!(
        sql::MEMORY_SHARD_V2_BACKFILL,
        frozen::FROZEN_SESSION_ID_BACKFILL
    );

    // Pre-v2 fixture insert (v1-era table shape: no session_id column yet —
    // the golden lib INSERTs are v2-shaped and only valid after the chain).
    const PRE_V2_INSERT: &str = r#"
      INSERT INTO memories (
        id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
        metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url
      ) VALUES (?, ?, vector32(?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    "#;
    let payload = |id: &str, metadata: Option<&str>| {
        vec![
            text(id),
            text("session memory"),
            text("[1.0,0.0]"),
            Value::Null,
            text("opencode_project_a1b2c3d4e5f60001"),
            Value::Null,
            Value::Null,
            Value::Integer(1),
            Value::Integer(1),
            match metadata {
                Some(value) => Value::Text(value.to_string()),
                None => Value::Null,
            },
            // pre-v2: display_name, user_name, user_email, project_path,
            // project_name, git_repo_url (no session_id column yet)
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ]
    };
    db.execute(
        PRE_V2_INSERT,
        payload("with_session", Some(r#"{"sessionID":"s1"}"#)),
    )
    .await
    .unwrap();
    db.execute(PRE_V2_INSERT, payload("null_meta", None))
        .await
        .unwrap();

    let v2 = sql::memory_shard_v2_statements();
    assert_eq!(v2.len(), 3);
    for statement in v2 {
        db.execute(statement, vec![]).await.expect("v2 statement");
    }

    let rows = db
        .query(
            "SELECT id, session_id, metadata FROM memories ORDER BY id",
            vec![],
        )
        .await
        .expect("backfill query");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].text("id").as_deref(), Some("null_meta"));
    assert!(rows[0].is_null("session_id"), "NULL metadata row untouched");
    assert_eq!(rows[1].text("id").as_deref(), Some("with_session"));
    assert_eq!(rows[1].text("session_id").as_deref(), Some("s1"));
}

// ---------------------------------------------------------------------------
// 7/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_vector_selects_execute_and_aggregate() {
    let dir = common::temp_dir("golden-selects");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(2)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    assert_eq!(sql::GET_ALL_MEMORIES, frozen::FROZEN_GET_ALL_MEMORIES);
    assert_eq!(sql::COUNT_ALL_VECTORS, frozen::FROZEN_COUNT_ALL_VECTORS);
    assert_eq!(sql::GET_DISTINCT_TAGS, frozen::FROZEN_GET_DISTINCT_TAGS);
    assert_eq!(
        sql::GET_PROJECT_PATH_COUNTS,
        frozen::FROZEN_GET_PROJECT_PATH_COUNTS
    );

    let row = |id: &str, tag: &str, project_path: Option<&str>, created_at: i64| {
        vec![
            text(id),
            text("aggregate memory"),
            text("[1.0,0.0]"),
            text("[0.0,0.0]"), // tags_vector: vector32(?) rejects NULL binds
            text(tag),
            Value::Null,
            Value::Null,
            Value::Integer(created_at),
            Value::Integer(created_at),
            Value::Null,
            Value::Null,
            Value::Null,
            text("Assoc Display"),
            text("assoc@example.invalid"),
            match project_path {
                Some(p) if !p.is_empty() => Value::Text(p.to_string()),
                _ => Value::Null,
            },
            text("AggregateProject"),
            Value::Null,
        ]
    };
    db.execute(
        sql::INSERT_WITH_TAGS,
        row("mem_a2", "tag_a", Some("/proj/a"), 20),
    )
    .await
    .unwrap();
    db.execute(
        sql::INSERT_WITH_TAGS,
        row("mem_a1", "tag_a", Some("/proj/a"), 10),
    )
    .await
    .unwrap();
    db.execute(
        sql::INSERT_WITH_TAGS,
        row("mem_b1", "tag_b", Some("/proj/b"), 30),
    )
    .await
    .unwrap();
    db.execute(sql::INSERT_WITH_TAGS, row("mem_none", "tag_c", None, 40))
        .await
        .unwrap();
    db.execute(
        sql::INSERT_WITH_TAGS,
        row("mem_empty", "tag_c", Some(""), 50),
    )
    .await
    .unwrap();

    // GET_ALL_MEMORIES orders by created_at DESC.
    let ordered = db.query(sql::GET_ALL_MEMORIES, vec![]).await.unwrap();
    let ids: Vec<String> = ordered.iter().map(|r| r.text("id").unwrap()).collect();
    assert_eq!(
        ids,
        vec!["mem_empty", "mem_none", "mem_b1", "mem_a2", "mem_a1"]
    );

    let count_rows = db.query(sql::COUNT_ALL_VECTORS, vec![]).await.unwrap();
    assert_eq!(count_rows.first().and_then(|r| r.i64("count")), Some(5));

    // GET_DISTINCT_TAGS collapses identical associations (7 columns).
    let distinct = db.query(sql::GET_DISTINCT_TAGS, vec![]).await.unwrap();
    assert_eq!(
        distinct.len(),
        3,
        "distinct over (container_tag, display_name, user_name, user_email, project_path, project_name, git_repo_url)"
    );
    let matched: Vec<String> = distinct
        .iter()
        .filter_map(|r| r.text("container_tag"))
        .collect();
    assert!(matched.contains(&"tag_a".to_string()));
    for row in &distinct {
        // all seven DISTINCT columns must be addressable by name
        assert!(row.get("container_tag").is_some());
        assert!(row.get("display_name").is_some());
        assert!(row.get("user_name").is_some());
        assert!(row.get("user_email").is_some());
        assert!(row.get("project_path").is_some());
        assert!(row.get("project_name").is_some());
        assert!(row.get("git_repo_url").is_some());
    }

    // GET_PROJECT_PATH_COUNTS groups + counts + orders, excluding NULL/empty paths.
    let counts = db
        .query(sql::GET_PROJECT_PATH_COUNTS, vec![])
        .await
        .unwrap();
    let summary: Vec<(String, i64)> = counts
        .iter()
        .map(|r| (r.text("project_path").unwrap(), r.i64("cnt").unwrap()))
        .collect();
    assert_eq!(
        summary,
        vec![("/proj/a".to_string(), 2), ("/proj/b".to_string(), 1)]
    );
    assert_eq!(counts[0].i64("cnt"), Some(2));
    assert_eq!(counts[0].text("container_tag").as_deref(), Some("tag_a"));
}

// ---------------------------------------------------------------------------
// 8/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_update_project_association_remaps_and_counts() {
    let dir = common::temp_dir("golden-update-assoc");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(2)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    assert_eq!(
        sql::UPDATE_PROJECT_ASSOCIATION,
        frozen::FROZEN_UPDATE_PROJECT_ASSOCIATION
    );

    fn insert_row<'a>(
        db: &'a dyn Db,
        id: &'a str,
        tag: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            db.execute(
                sql::INSERT_WITHOUT_TAGS,
                vec![
                    text(id),
                    text("remap me"),
                    text("[1.0,0.0]"),
                    text(tag),
                    Value::Null,
                    Value::Null,
                    Value::Integer(1),
                    Value::Integer(1),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                ],
            )
            .await
            .unwrap();
        })
    }
    insert_row(db.as_ref(), "mem_old_1", "opencode_project_oldoldoldolda1").await;
    insert_row(db.as_ref(), "mem_old_2", "opencode_project_oldoldoldolda1").await;
    insert_row(db.as_ref(), "mem_kept", "opencode_project_zzzzzzzzzzzza1").await;

    let changes = db
        .execute(
            sql::UPDATE_PROJECT_ASSOCIATION,
            vec![
                text("opencode_project_newnewnewn1"),
                text("/new/project"),
                text("NewProject"),
                text("New Display"),
                Value::Null,
                text("opencode_project_oldoldoldolda1"),
            ],
        )
        .await
        .unwrap()
        .changes;
    assert_eq!(changes, 2, "only the matching container_tag rows migrate");

    let rows = db
        .query(
            "SELECT id, container_tag, project_path FROM memories ORDER BY id",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let new_path: Vec<String> = rows
        .iter()
        .filter(|r| r.text("container_tag").as_deref() == Some("opencode_project_newnewnewn1"))
        .filter_map(|r| r.text("project_path"))
        .collect();
    assert_eq!(new_path, vec!["/new/project", "/new/project"]);
    assert!(
        rows.iter()
            .all(|r| r.text("container_tag").as_deref() != Some("opencode_project_oldoldoldolda1")),
        "old tag fully remapped"
    );
}

// ---------------------------------------------------------------------------
// 9/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_like_escape_shard_lookup_matches_underscored_and_percent_filenames() {
    let dir = common::temp_dir("golden-like-escape");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "metadata.db").await;
    batch_write(db.as_ref(), vec![sql::SHARDS_DDL]).await;

    assert_eq!(sql::GET_SHARD_BY_PATH, frozen::FROZEN_GET_SHARD_BY_PATH);

    let stored = [
        "projects/user_a_b_shard_0.db",
        "projects/user_a%b_shard_0.db",
        "projects/user_aXb_shard_0.db", // what an unescaped `_` would wrongly match
    ];
    for (index, path) in stored.iter().enumerate() {
        db.execute(
            sql::CREATE_SHARD,
            vec![
                text("project"),
                text(format!("a1b2c3d4e5f6000{index}")),
                Value::Integer(0),
                text(*path),
                Value::Integer(0),
            ],
        )
        .await
        .unwrap();
    }

    async fn lookup_by_path(db: &dyn Db, escaped: &str) -> Vec<mem_store::db::Row> {
        db.query(sql::GET_SHARD_BY_PATH, vec![text(escaped)])
            .await
            .expect("shard-by-path lookup")
    }
    // `_` escaped: only the literal underscore filename matches.
    let rows = lookup_by_path(db.as_ref(), r"user\_a\_b\_shard\_0.db").await;
    assert_eq!(rows.len(), 1, "exact match only for escaped pattern");
    assert_eq!(rows[0].text("db_path").as_deref(), Some(stored[0]));
    // `%` escaped: only the literal percent filename matches.
    let rows = lookup_by_path(db.as_ref(), r"user\_a\%b\_shard\_0.db").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text("db_path").as_deref(), Some(stored[1]));
}

// ---------------------------------------------------------------------------
// 10/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_shard_registry_crud_executes() {
    let dir = common::temp_dir("golden-shard-crud");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "metadata.db").await;
    batch_write(db.as_ref(), vec![sql::SHARDS_DDL, sql::SHARDS_ACTIVE_INDEX]).await;

    // Byte-equality pins for every shard-registry constant.
    assert_eq!(sql::GET_ACTIVE_SHARD, frozen::FROZEN_GET_ACTIVE_SHARD);
    assert_eq!(
        sql::GET_ALL_SHARDS_FOR_SCOPE,
        frozen::FROZEN_GET_ALL_SHARDS_FOR_SCOPE
    );
    assert_eq!(
        sql::GET_ALL_SHARDS_FOR_SCOPE_HASH,
        frozen::FROZEN_GET_ALL_SHARDS_FOR_SCOPE_HASH
    );
    assert_eq!(sql::CREATE_SHARD, frozen::FROZEN_CREATE_SHARD);
    assert_eq!(
        sql::FIND_SHARD_BY_SCOPE_INDEX,
        frozen::FROZEN_FIND_SHARD_BY_SCOPE_INDEX
    );
    assert_eq!(sql::FIND_SHARD_BY_KEY, frozen::FROZEN_FIND_SHARD_BY_KEY);
    assert_eq!(
        sql::REGISTER_EXISTING_SHARD,
        frozen::FROZEN_REGISTER_EXISTING_SHARD
    );
    assert_eq!(sql::GET_SHARD_BY_ID, frozen::FROZEN_GET_SHARD_BY_ID);
    assert_eq!(sql::GET_SHARD_BY_PATH, frozen::FROZEN_GET_SHARD_BY_PATH);
    assert_eq!(
        sql::REASSIGN_SHARD_SCOPE,
        frozen::FROZEN_REASSIGN_SHARD_SCOPE
    );
    assert_eq!(sql::MARK_SHARD_INACTIVE, frozen::FROZEN_MARK_SHARD_INACTIVE);
    assert_eq!(sql::SET_VECTOR_COUNT, frozen::FROZEN_SET_VECTOR_COUNT);
    assert_eq!(sql::DELETE_SHARD_ROW, frozen::FROZEN_DELETE_SHARD_ROW);
    assert_eq!(
        sql::SHARD_METADATA_INSERT_DIMENSIONS,
        frozen::FROZEN_SHARD_METADATA_INSERT_DIMENSIONS
    );
    assert_eq!(
        sql::SHARD_METADATA_INSERT_MODEL,
        frozen::FROZEN_SHARD_METADATA_INSERT_MODEL
    );
    assert_eq!(
        sql::SHARD_METADATA_SELECT_DIMENSIONS,
        frozen::FROZEN_SHARD_METADATA_SELECT_DIMENSIONS
    );
    assert_eq!(
        sql::SQLITE_MASTER_MEMORIES_TABLE,
        frozen::FROZEN_SQLITE_MASTER_MEMORIES
    );
    assert_eq!(
        sql::UPDATE_MEMORIES_PROJECT_ASSOCIATION,
        frozen::FROZEN_UPDATE_MEMORIES_PROJECT_ASSOCIATION
    );

    let find = sql::FIND_SHARD_BY_KEY;
    let result = db
        .execute(
            sql::CREATE_SHARD,
            vec![
                text("project"),
                text("a1b2c3d4e5f60009"),
                Value::Integer(0),
                text("projects/project_a1b2c3d4e5f60009_shard_0.db"),
                Value::Integer(1700000000000),
            ],
        )
        .await
        .unwrap();
    let shard_id = result.last_insert_rowid;
    assert!(shard_id > 0, "CREATE_SHARD must produce a rowid");

    let row = db
        .query(
            find,
            vec![text("project"), text("a1b2c3d4e5f60009"), Value::Integer(0)],
        )
        .await
        .unwrap();
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].i64("id"), Some(shard_id));

    db.execute(
        sql::REGISTER_EXISTING_SHARD,
        vec![
            text("project"),
            text("a1b2c3d4e5f60009"),
            Value::Integer(0),
            text("projects/project_a1b2c3d4e5f60009_shard_0.db"),
            Value::Integer(7),
            Value::Integer(1),
            Value::Integer(1700000000000),
        ],
    )
    .await
    .expect("upsert");
    let row = db
        .query(
            find,
            vec![text("project"), text("a1b2c3d4e5f60009"), Value::Integer(0)],
        )
        .await
        .unwrap();
    assert_eq!(row.len(), 1, "upsert does not duplicate");
    assert_eq!(row[0].i64("vector_count"), Some(7));

    db.execute(
        sql::REASSIGN_SHARD_SCOPE,
        vec![
            text("b2c3d4e5f6000009"),
            text("projects/project_b2c3d4e5f6000009_shard_0.db"),
            Value::Integer(9),
            Value::Integer(0),
            Value::Integer(shard_id),
        ],
    )
    .await
    .expect("reassign");
    let row = db
        .query(sql::GET_SHARD_BY_ID, vec![Value::Integer(shard_id)])
        .await
        .unwrap();
    assert_eq!(
        row[0].text("scope_hash").as_deref(),
        Some("b2c3d4e5f6000009")
    );
    assert_eq!(row[0].i64("is_active"), Some(0));

    db.execute(sql::MARK_SHARD_INACTIVE, vec![Value::Integer(shard_id)])
        .await
        .unwrap();
    db.execute(
        sql::SET_VECTOR_COUNT,
        vec![Value::Integer(11), Value::Integer(shard_id)],
    )
    .await
    .unwrap();
    let row = db
        .query(sql::GET_SHARD_BY_ID, vec![Value::Integer(shard_id)])
        .await
        .unwrap();
    assert_eq!(
        row[0].i64("vector_count"),
        Some(11),
        "SET_VECTOR_COUNT keeps its pin (W3 deleted the delta statements)"
    );
    assert_eq!(row[0].i64("is_active"), Some(0));

    db.execute(sql::DELETE_SHARD_ROW, vec![Value::Integer(shard_id)])
        .await
        .unwrap();
    let gone = db
        .query(
            find,
            vec![text("project"), text("b2c3d4e5f6000009"), Value::Integer(0)],
        )
        .await
        .unwrap();
    assert!(gone.is_empty(), "row deleted");
}

// ---------------------------------------------------------------------------
// 11/12
// ---------------------------------------------------------------------------

/// The `is_read_query` classification table (upstream `turso-db.ts` L24-26).
#[test]
fn test_is_read_query_classification_matches_upstream() {
    for read in [
        "SELECT * FROM memories",
        "  select x",
        "WITH t AS (SELECT 1) SELECT * FROM t",
        "PRAGMA user_version",
        "PRAGMA table_info(user_prompts)",
        "\n      SELECT * FROM memories\n      ORDER BY created_at DESC\n    ",
    ] {
        assert!(sql::is_read_query(read), "expected READ: {read:?}");
    }
    for write in [
        "PRAGMA foreign_keys = ON",
        "INSERT INTO memories VALUES (?)",
        "UPDATE memories SET is_pinned = 1",
        "DELETE FROM memories",
        "CREATE TABLE memories (id TEXT PRIMARY KEY)",
        "SELECTX",
        "PRAGMA user_version;",
    ] {
        assert!(!sql::is_read_query(write), "expected WRITE: {write:?}");
    }
}

// ---------------------------------------------------------------------------
// 12/12
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pragma_user_version_roundtrips_via_set_sql() {
    let dir = common::temp_dir("golden-user-version");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "version.db").await;

    assert_eq!(sql::PRAGMA_USER_VERSION, "PRAGMA user_version");
    let rows = db.query(sql::PRAGMA_USER_VERSION, vec![]).await.unwrap();
    assert_eq!(rows[0].i64("user_version"), Some(0), "fresh database");

    db.execute(&set_user_version_sql(3), vec![])
        .await
        .expect("set user_version");
    let rows = db.query(sql::PRAGMA_USER_VERSION, vec![]).await.unwrap();
    assert_eq!(rows[0].i64("user_version"), Some(3));
}

// ---------------------------------------------------------------------------
// 13/15
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_search_statements_match_library_and_execute() {
    let dir = common::temp_dir("golden-search-stmts");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(2)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    // Byte-equality pins for every W1.3 search/session/pin/reembed/metadata
    // constant against the frozen upstream literals.
    assert_eq!(
        sql::SEARCH_EXACT_VECTOR_ALL,
        frozen::FROZEN_SEARCH_EXACT_VECTOR_ALL
    );
    assert_eq!(
        sql::SEARCH_EXACT_VECTOR_TAGGED,
        frozen::FROZEN_SEARCH_EXACT_VECTOR_TAGGED
    );
    assert_eq!(
        sql::SEARCH_EXACT_TAGS_VECTOR_ALL,
        frozen::FROZEN_SEARCH_EXACT_TAGS_VECTOR_ALL
    );
    assert_eq!(
        sql::SEARCH_EXACT_TAGS_VECTOR_TAGGED,
        frozen::FROZEN_SEARCH_EXACT_TAGS_VECTOR_TAGGED
    );
    assert_eq!(sql::KEYWORD_SCORES_ALL, frozen::FROZEN_KEYWORD_SCORES_ALL);
    assert_eq!(
        sql::KEYWORD_SCORES_TAGGED,
        frozen::FROZEN_KEYWORD_SCORES_TAGGED
    );
    assert_eq!(
        sql::GET_MEMORIES_BY_SESSION_ID,
        frozen::FROZEN_GET_MEMORIES_BY_SESSION_ID
    );
    assert_eq!(sql::PIN_MEMORY, frozen::FROZEN_PIN_MEMORY);
    assert_eq!(sql::UNPIN_MEMORY, frozen::FROZEN_UNPIN_MEMORY);
    assert_eq!(
        sql::SELECT_REEMBED_SOURCE,
        frozen::FROZEN_SELECT_REEMBED_SOURCE
    );
    assert_eq!(
        sql::GET_MEMORIES_UNORDERED,
        frozen::FROZEN_GET_MEMORIES_UNORDERED
    );
    assert_eq!(
        sql::GET_SHARD_METADATA_VALUE,
        frozen::FROZEN_GET_SHARD_METADATA_VALUE
    );

    // Seed: mem_exact is the nearest to [1,0]; mem_half is second; the tagged
    // pair splits container tags so the filtered scan returns one row.
    async fn insert(db: &dyn Db, id: &str, vector: &str, tag: &str) {
        db.execute(
            sql::INSERT_WITHOUT_TAGS,
            vec![
                text(id),
                text(format!("content-{id}")),
                text(vector),
                text(tag),
                Value::Null,
                Value::Null,
                Value::Integer(1),
                Value::Integer(1),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ],
        )
        .await
        .unwrap();
    }
    insert(db.as_ref(), "mem_exact", "[1,0]", "opencode_exact").await;
    insert(db.as_ref(), "mem_half", "[1,1]", "opencode_exact").await;
    insert(db.as_ref(), "mem_orthogonal", "[0,1]", "opencode_other").await;

    // Unfiltered exact scan: nearest first, limit respected.
    let rows = db
        .query(
            sql::SEARCH_EXACT_VECTOR_ALL,
            vec![text("[1,0]"), Value::Integer(2)],
        )
        .await
        .expect("exact scan executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(ids, vec!["mem_exact", "mem_half"]);
    for row in &rows {
        assert!(row.get("dist").is_some(), "dist column returned: {row:?}");
    }

    // Filtered exact scan: container_tag predicate applies.
    let rows = db
        .query(
            sql::SEARCH_EXACT_VECTOR_TAGGED,
            vec![text("[1,0]"), text("opencode_exact"), Value::Integer(10)],
        )
        .await
        .expect("tagged exact scan executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(ids, vec!["mem_exact", "mem_half"]);
    let rows = db
        .query(
            sql::SEARCH_EXACT_VECTOR_TAGGED,
            vec![text("[1,0]"), text("opencode_other"), Value::Integer(10)],
        )
        .await
        .expect("tagged exact scan executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(ids, vec!["mem_orthogonal"]);
}

// ---------------------------------------------------------------------------
// 14/15
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_hydrate_candidates_sql_placeholders_and_execution() {
    // Placeholder count == id count; ids are BOUND PARAMS (never interpolated).
    let unfiltered = sql::hydrate_candidates_sql(2, false);
    assert!(
        unfiltered.contains("WHERE id IN (?, ?)"),
        "two placeholders: {unfiltered}"
    );
    assert!(!unfiltered.contains("container_tag = ?"));
    let expected = "SELECT id, content, tags, created_at, metadata, container_tag,\n             display_name, user_name, user_email, project_path, project_name,\n             git_repo_url, is_pinned, session_id,\n             vector_distance_cos(vector, vector32(?)) AS content_dist,\n             CASE WHEN tags_vector IS NOT NULL\n               THEN vector_distance_cos(tags_vector, vector32(?))\n               ELSE NULL END AS tags_dist\n      FROM memories\n      WHERE id IN (?, ?)\n    ";
    assert_eq!(unfiltered, expected);

    // Filtered variant appends the container_tag predicate.
    let filtered = sql::hydrate_candidates_sql(2, true);
    assert!(
        filtered.contains("WHERE id IN (?, ?) AND container_tag = ?"),
        "filtered suffix: {filtered}"
    );
    let expected_filtered = "SELECT id, content, tags, created_at, metadata, container_tag,\n             display_name, user_name, user_email, project_path, project_name,\n             git_repo_url, is_pinned, session_id,\n             vector_distance_cos(vector, vector32(?)) AS content_dist,\n             CASE WHEN tags_vector IS NOT NULL\n               THEN vector_distance_cos(tags_vector, vector32(?))\n               ELSE NULL END AS tags_dist\n      FROM memories\n      WHERE id IN (?, ?) AND container_tag = ?\n    ";
    assert_eq!(filtered, expected_filtered);

    // Id-count clamp: at most 1000 placeholders regardless of the request.
    let clamped = sql::hydrate_candidates_sql(5000, false);
    let in_clause = clamped
        .split("WHERE id IN (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("IN clause present");
    assert_eq!(in_clause.matches('?').count(), 1000);

    // Execution: 2 leading vector32(?) args, then the bound ids.
    let dir = common::temp_dir("golden-hydrate");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(2)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    async fn insert(db: &dyn Db, id: &str) {
        db.execute(
            sql::INSERT_WITHOUT_TAGS,
            vec![
                text(id),
                text(format!("content-{id}")),
                text("[1,0]"),
                text("opencode_hydrate"),
                Value::Null,
                Value::Null,
                Value::Integer(1),
                Value::Integer(1),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ],
        )
        .await
        .unwrap();
    }
    insert(db.as_ref(), "mem_h1").await;
    insert(db.as_ref(), "mem_h2").await;
    insert(db.as_ref(), "mem_h3").await;

    let rows = db
        .query(
            &sql::hydrate_candidates_sql(2, false),
            vec![text("[1,0]"), text("[1,0]"), text("mem_h1"), text("mem_h3")],
        )
        .await
        .expect("hydrate executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(ids, vec!["mem_h1", "mem_h3"]);
    for row in &rows {
        assert!(row.get("content_dist").is_some());
    }
    // Filtered execution binds the container tag AFTER the ids.
    let rows = db
        .query(
            &sql::hydrate_candidates_sql(1, true),
            vec![
                text("[1,0]"),
                text("[1,0]"),
                text("mem_h2"),
                text("opencode_hydrate"),
            ],
        )
        .await
        .expect("hydrate filtered executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(ids, vec!["mem_h2"]);
}

// ---------------------------------------------------------------------------
// 15/15
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frozen_keyword_like_escape_matches_literals() {
    let dir = common::temp_dir("golden-like-escape");
    let config = test_config(&dir, 2);
    let db = open_db(&config, "shard.db").await;
    batch_write(
        db.as_ref(),
        vec![sql::shard_metadata_ddl(), &sql::memories_ddl(2)],
    )
    .await;
    batch_write(
        db.as_ref(),
        vec![
            sql::MEMORY_SHARD_V2_ALTER,
            sql::MEMORY_SHARD_V2_INDEX,
            sql::MEMORY_SHARD_V2_BACKFILL,
        ],
    )
    .await;

    // One row with a LITERAL `%`/`_` in content; one row that only a bare
    // wildcard pattern would additionally match.
    async fn insert(db: &dyn Db, id: &str, content: &str) {
        db.execute(
            sql::INSERT_WITHOUT_TAGS,
            vec![
                text(id),
                text(content),
                text("[1,0]"),
                text("opencode_like"),
                Value::Null,
                Value::Null,
                Value::Integer(1),
                Value::Integer(1),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ],
        )
        .await
        .unwrap();
    }
    insert(db.as_ref(), "mem_literal", "progress 100%_done today").await;
    insert(db.as_ref(), "mem_wildcard", "progress 100Xdone today").await;

    // escape_like_pattern + ESCAPE '\': the literal pattern matches only the
    // row whose content holds the raw `%`/`_` characters.
    let escaped = escape_like_pattern("100%_done");
    assert_eq!(escaped, "100\\%\\_done");
    let escaped_pattern = format!("%{}%", escaped);
    let rows = db
        .query(
            sql::KEYWORD_SCORES_ALL,
            vec![
                text(&escaped_pattern),
                text(&escaped_pattern),
                Value::Integer(10),
            ],
        )
        .await
        .expect("keyword scan executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(
        ids,
        vec!["mem_literal"],
        "escaped wildcards match literally"
    );

    // A bare (unescaped) pattern widens the match via LIKE wildcards.
    let bare_pattern = "%100%_done%".to_string();
    let rows = db
        .query(
            sql::KEYWORD_SCORES_ALL,
            vec![text(&bare_pattern), text(&bare_pattern), Value::Integer(10)],
        )
        .await
        .expect("keyword scan executes");
    let ids: Vec<String> = rows.iter().map(|row| row.text("id").unwrap()).collect();
    assert_eq!(
        ids,
        vec!["mem_literal", "mem_wildcard"],
        "bare wildcards widen the match"
    );
}
