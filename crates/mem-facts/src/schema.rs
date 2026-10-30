//! The `facts.db` v1 schema (fresh file: both tables are v1, no migration
//! history).
//!
//! # `fact_provenance` is a TABLE, not `metadata.sessions`
//!
//! Purge needs an EXACT indexed `DELETE … WHERE`; JSON stored in `TEXT`
//! cannot be indexed without a JSON1 extension this stack has not verified,
//! and `LIKE '%<session>%'` would match content. The table keeps `source_ref`
//! authoritative while making the session chain queryable.
//!
//! # Ids are keyed, not content-addressed
//!
//! `id = "fact-" + first 32 hex of sha256(project_id | tier | key)` — a
//! digest over the NORMALIZED KEY, never over `content`. A raw content digest
//! would be an offline confirmation oracle (the vault-keyed HMAC rule, Q2b,
//! is vault-only; this file must still not use content digests).

use mem_core::dedup::normalize_key;
use mem_core::tier::MemoryTier;
use mem_store::db::Statement;
use mem_store::schema_migrations::SchemaMigration;

/// The `facts.db` file name.
pub const FACTS_DB_NAME: &str = "facts.db";

/// The `facts` table DDL (`F32_BLOB({dims})`; `dims` is a `u32` literal, never
/// user text).
pub fn facts_ddl(dimensions: u32) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS facts (\n\
         \x20 id TEXT PRIMARY KEY,\n\
         \x20 project_id TEXT NOT NULL,\n\
         \x20 tier TEXT NOT NULL,\n\
         \x20 content TEXT NOT NULL,\n\
         \x20 key_normalized TEXT NOT NULL,\n\
         \x20 embedding F32_BLOB({dimensions}),\n\
         \x20 model_id TEXT,\n\
         \x20 stale INTEGER NOT NULL DEFAULT 0,\n\
         \x20 generated_at INTEGER,\n\
         \x20 source_ref TEXT,\n\
         \x20 confidence REAL NOT NULL DEFAULT 1.0,\n\
         \x20 version INTEGER NOT NULL DEFAULT 1,\n\
         \x20 metadata TEXT,\n\
         \x20 created_at INTEGER NOT NULL,\n\
         \x20 updated_at INTEGER NOT NULL,\n\
         \x20 deleted_at INTEGER,\n\
         \x20 UNIQUE (project_id, tier, key_normalized)\n\
         )"
    )
}

/// The `fact_provenance` table DDL.
///
/// `PRIMARY KEY (fact_id, project_id, session_id, agent_id)` makes one fact
/// record one row per contributing session, and re-promotion a no-op.
pub const FACT_PROVENANCE_DDL: &str = "CREATE TABLE IF NOT EXISTS fact_provenance (\n\
 \x20 fact_id TEXT NOT NULL,\n\
 \x20 project_id TEXT NOT NULL,\n\
 \x20 session_id TEXT NOT NULL,\n\
 \x20 agent_id TEXT NOT NULL,\n\
 \x20 milestone_id TEXT NOT NULL,\n\
 \x20 created_at INTEGER NOT NULL,\n\
 \x20 PRIMARY KEY (fact_id, project_id, session_id, agent_id)\n\
 \x20)";

/// The `facts` indexes: live listing, source lookup, stale/model sweep.
pub const FACTS_INDEXES: [&str; 3] = [
    "CREATE INDEX IF NOT EXISTS idx_facts_project_tier_deleted ON facts(project_id, tier, deleted_at)",
    "CREATE INDEX IF NOT EXISTS idx_facts_source_ref ON facts(source_ref)",
    "CREATE INDEX IF NOT EXISTS idx_facts_stale_model ON facts(stale, model_id)",
];

/// The `fact_provenance` indexes: the purge predicates.
pub const FACT_PROVENANCE_INDEXES: [&str; 2] = [
    "CREATE INDEX IF NOT EXISTS idx_fact_provenance_session ON fact_provenance(project_id, session_id, agent_id)",
    "CREATE INDEX IF NOT EXISTS idx_fact_provenance_milestone ON fact_provenance(milestone_id)",
];

/// The v1 migration set (both tables plus their indexes).
pub fn facts_migrations(dimensions: u32) -> Vec<SchemaMigration> {
    let mut statements = vec![
        Statement::new(facts_ddl(dimensions), Vec::new()),
        Statement::new(FACT_PROVENANCE_DDL, Vec::new()),
    ];
    for index in FACTS_INDEXES {
        statements.push(Statement::new(index, Vec::new()));
    }
    for index in FACT_PROVENANCE_INDEXES {
        statements.push(Statement::new(index, Vec::new()));
    }
    vec![SchemaMigration {
        version: 1,
        description: "facts-v1: facts + fact_provenance (W7.1)",
        statements,
    }]
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// Insert a fact without an embedding (this wave has no embedding producer).
pub const INSERT_FACT_NO_EMBEDDING: &str = "INSERT INTO facts (id, project_id, tier, content, key_normalized, embedding, model_id, stale, generated_at, source_ref, confidence, version, metadata, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, NULL, ?, 0, NULL, ?, ?, 1, ?, ?, ?, NULL)";

/// Insert a fact with an embedding (`vector32(?)`).
pub const INSERT_FACT_WITH_EMBEDDING: &str = "INSERT INTO facts (id, project_id, tier, content, key_normalized, embedding, model_id, stale, generated_at, source_ref, confidence, version, metadata, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, vector32(?), ?, 0, NULL, ?, ?, 1, ?, ?, ?, NULL)";

/// Upsert an existing fact: bump the version, clear `stale`/`deleted_at`.
pub const UPDATE_FACT: &str = "UPDATE facts SET content = ?, key_normalized = ?, stale = 0, source_ref = ?, confidence = ?, metadata = ?, version = version + 1, updated_at = ?, deleted_at = NULL WHERE id = ?";

/// One provenance row per contributing session; re-promotion is a no-op.
pub const INSERT_PROVENANCE: &str = "INSERT INTO fact_provenance (fact_id, project_id, session_id, agent_id, milestone_id, created_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING";

/// By-id read (includes soft-deleted rows; `soft_delete` keeps them).
pub const SELECT_FACT_BY_ID: &str = "SELECT id, project_id, tier, content, key_normalized, model_id, stale, generated_at, source_ref, confidence, version, metadata, created_at, updated_at, deleted_at FROM facts WHERE id = ?";

/// Live listing, newest first.
pub const LIST_FACTS: &str = "SELECT id, project_id, tier, content, key_normalized, model_id, stale, generated_at, source_ref, confidence, version, metadata, created_at, updated_at, deleted_at FROM facts WHERE project_id = ? AND deleted_at IS NULL ORDER BY updated_at DESC, id DESC LIMIT ?";

/// Live facts promoted by one milestone in one project, newest first.
///
/// The join key is `source_ref` (authoritative; see
/// `crates/mem-facts/CONTEXT.md`), not `fact_provenance`, whose
/// `ON CONFLICT DO NOTHING` rows name only the FIRST promoting milestone.
pub const LIST_FACTS_BY_MILESTONE: &str = "SELECT id, project_id, tier, content, key_normalized, model_id, stale, generated_at, source_ref, confidence, version, metadata, created_at, updated_at, deleted_at FROM facts WHERE project_id = ? AND source_ref = ? AND deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT ?";

/// Live count per project.
pub const COUNT_FACTS: &str =
    "SELECT COUNT(*) AS count FROM facts WHERE project_id = ? AND deleted_at IS NULL";

/// Soft delete: stamp `deleted_at` on one live row.
pub const SOFT_DELETE_FACT: &str =
    "UPDATE facts SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL";

// ---------------------------------------------------------------------------
// W7.1 purge statements (D6, M17, M21). Every value is BOUND; the only
// generated SQL is the placeholder count of an id list.
// ---------------------------------------------------------------------------

/// The fact ids whose provenance names the triple.
pub const SELECT_PROVENANCE_FACT_IDS_FOR_TRIPLE: &str =
    "SELECT fact_id FROM fact_provenance WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// The fact ids whose provenance names the milestone (project-scoped, so a
/// single-scope purge cannot touch another project's rows).
pub const SELECT_PROVENANCE_FACT_IDS_FOR_MILESTONE: &str =
    "SELECT fact_id FROM fact_provenance WHERE project_id = ? AND milestone_id = ?";

/// Delete one fact by id (the ONLY hard delete of a fact).
pub const DELETE_FACT_BY_ID: &str = "DELETE FROM facts WHERE id = ?";

/// Delete the provenance rows of the triple OR of one deleted fact.
pub const DELETE_PROVENANCE_BY_TRIPLE_OR_FACT: &str = "DELETE FROM fact_provenance WHERE (project_id = ? AND session_id = ? AND agent_id = ?) OR fact_id = ?";

/// Delete the triple's provenance rows (covers rows whose fact was already
/// gone).
pub const DELETE_PROVENANCE_BY_TRIPLE: &str =
    "DELETE FROM fact_provenance WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// The PROJECT-SCOPED orphan sweep (M21): never an unscoped cross-project
/// write.
pub const DELETE_ORPHAN_PROVENANCE_SCOPED: &str = "DELETE FROM fact_provenance WHERE project_id = ? AND fact_id NOT IN (SELECT id FROM facts WHERE project_id = ?)";

/// The deterministic fact id: `"fact-"` + first 32 hex of
/// `sha256(project_id | tier | normalized key)`.
pub fn fact_id(project_id: &str, tier: MemoryTier, key: &str) -> String {
    let normalized = normalize_key(key);
    let digest = mem_core::sha256::digest(
        format!("{project_id}\u{1}{}\u{1}{normalized}", tier.as_str()).as_bytes(),
    );
    format!("fact-{}", &mem_core::sha256::hex(&digest)[..32])
}
