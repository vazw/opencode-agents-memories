//! NET-NEW — no upstream oracle (R10); not part of the golden SQL locus in
//! `sql.rs`. The episodic schema is defined by the W4.1 acceptance tests, not
//! by a TypeScript service.
//!
//! Blueprint divergence note (W4.1): the DDL and index set are net-new
//! domain SQL, so they live here rather than in the byte-verbatim upstream
//! `sql.rs` and are never added to `golden_sql.rs`.
//!
//! The module also hosts the net-new query statements used by
//! `mem_episodic::store` so the crate keeps one SQL locus and the store
//! never interpolates values into SQL strings.

/// Baseline `agent_sessions` DDL (migration v1) with the embedding dimension
/// fixed at first migration.
///
/// `{dims}` is interpolated as a literal dimension count (`F32_BLOB(dims)`);
/// every value is a `u32` and never user text.
pub fn agent_sessions_ddl(dims: u32) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS agent_sessions (\n\
         \x20 id TEXT PRIMARY KEY,\n\
         \x20 project_id TEXT NOT NULL,\n\
         \x20 session_id TEXT NOT NULL,\n\
         \x20 agent_id TEXT NOT NULL,\n\
         \x20 parent_session_id TEXT,\n\
         \x20 summary TEXT NOT NULL,\n\
         \x20 metadata TEXT NOT NULL,\n\
         \x20 embedding F32_BLOB({dims}) NOT NULL,\n\
         \x20 created_at INTEGER NOT NULL,\n\
         \x20 updated_at INTEGER NOT NULL,\n\
         \x20 FOREIGN KEY (parent_session_id) REFERENCES agent_sessions(id) ON DELETE SET NULL,\n\
         \x20 UNIQUE (project_id, session_id, agent_id)\n\
         )"
    )
}

/// The six v1 indexes, in registration order.
pub const EPISODIC_INDEXES: [&str; 6] = [
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_project_id ON agent_sessions(project_id)",
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_session_id ON agent_sessions(session_id)",
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent_id ON agent_sessions(agent_id)",
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_parent_session_id ON agent_sessions(parent_session_id)",
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_created_at ON agent_sessions(created_at)",
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_project_created ON agent_sessions(project_id, created_at)",
];

/// v2 additive columns (D-9): `deleted_at` (soft delete) and `orphaned`
/// (severed-parent flag). The v1 DDL is never rewritten in place.
pub const EPISODIC_V2_ALTERS: [&str; 2] = [
    "ALTER TABLE agent_sessions ADD COLUMN deleted_at INTEGER",
    "ALTER TABLE agent_sessions ADD COLUMN orphaned INTEGER NOT NULL DEFAULT 0",
];

/// v2 index over the soft-delete visibility predicate.
pub const EPISODIC_V2_INDEXES: [&str; 1] = [
    "CREATE INDEX IF NOT EXISTS idx_agent_sessions_project_deleted_created ON agent_sessions(project_id, deleted_at, created_at)",
];

/// v3 DDL: the consolidated milestone records (W4.3).
///
/// `{dims}` is interpolated as a literal dimension count (`F32_BLOB(dims)`);
/// every value is a `u32` and never user text. The `contributors` column
/// holds the contributors JSON — including each source session's retained,
/// privacy-stripped summary text (D-27) — so no extra column is needed.
pub fn episodic_milestones_ddl(dims: u32) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS episodic_milestones (\n\
         \x20 id TEXT PRIMARY KEY,\n\
         \x20 project_id TEXT NOT NULL,\n\
         \x20 agent_id TEXT NOT NULL,\n\
         \x20 summary TEXT NOT NULL,\n\
         \x20 contributors TEXT NOT NULL,\n\
         \x20 source_count INTEGER NOT NULL,\n\
         \x20 window_start INTEGER NOT NULL,\n\
         \x20 window_end INTEGER NOT NULL,\n\
         \x20 embedding F32_BLOB({dims}) NOT NULL,\n\
         \x20 created_at INTEGER NOT NULL\n\
         )"
    )
}

/// The v3 index over `(project_id, agent_id, created_at)`.
pub const EPISODIC_V3_INDEXES: [&str; 1] = [
    "CREATE INDEX IF NOT EXISTS idx_episodic_milestones_project_agent_created ON episodic_milestones(project_id, agent_id, created_at)",
];

/// v4 additive column (D-36): the client-minted `capture_id` on
/// `agent_sessions`. Nullable, no default — rows written before the W5.1
/// bridge carry NULL, and W5.1 inserts no value (W5.3 owns the write path).
pub const EPISODIC_V4_ALTERS: [&str; 1] = ["ALTER TABLE agent_sessions ADD COLUMN capture_id TEXT"];

// ---------------------------------------------------------------------------
// Query statements (net-new, parameterized)
// ---------------------------------------------------------------------------

/// Column projection shared by every read path; `vector_extract` turns the
/// F32 blob back into JSON text for transport across the `Db` seam. The two
/// v2 columns ride along on every projection (D-11 visibility).
pub const SESSION_COLUMNS: &str = "id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned";

/// SQL-side UUIDv7 minting (turso_core 0.8.3 registers `uuid7_str()`); no
/// `uuid`/`rand` dependency edge.
pub const SELECT_UUID7: &str = "SELECT uuid7_str() AS id";

/// Insert one session row. The embedding is bound as JSON text through
/// `vector32(?)`; the two v2 columns are always bound explicitly (D-9
/// defaults are for rows that bypass this path).
pub const INSERT_SESSION: &str = "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, parent_session_id, summary, metadata, embedding, created_at, updated_at, deleted_at, orphaned) VALUES (?, ?, ?, ?, ?, ?, ?, vector32(?), ?, ?, ?, ?)";

/// Read-back by row id (the only id-only lookup; every other read path is
/// project-scoped). Soft-deleted rows are invisible (D-11).
pub const SELECT_SESSION_BY_ID: &str = "SELECT id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE id = ? AND deleted_at IS NULL";

/// Project-scoped by-id detail read (soft-deleted rows invisible, D-11).
pub const SELECT_SESSION_DETAIL: &str = "SELECT id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE id = ? AND project_id = ? AND deleted_at IS NULL";

/// Project-scoped newest-first listing (`created_at DESC`, id tie-break).
pub const LIST_SESSIONS: &str = "SELECT id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT ?";

/// Project-scoped cosine search, nearest first.
pub const SEARCH_SESSIONS: &str = "SELECT id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned, vector_distance_cos(embedding, vector32(?)) AS distance FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL ORDER BY distance ASC, id ASC LIMIT ?";

/// Delete by row id (the hard-delete purge path; `delete_session` prefixes
/// it with `MARK_CHILDREN_ORPHANED` in one write batch).
pub const DELETE_SESSION: &str = "DELETE FROM agent_sessions WHERE id = ?";

/// Diagnostic read-back of one embedding.
pub const SELECT_EMBEDDING_BY_ID: &str =
    "SELECT vector_extract(embedding) AS embedding FROM agent_sessions WHERE id = ?";

/// Global session count (diagnostics/tests only; deliberately includes
/// soft-deleted rows — D-11's second raw seam).
pub const COUNT_SESSIONS: &str = "SELECT COUNT(*) AS count FROM agent_sessions";

/// Project-scoped session count (soft-deleted rows invisible, D-11).
pub const COUNT_SESSIONS_BY_PROJECT: &str =
    "SELECT COUNT(*) AS count FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL";

/// Raw visibility state of one row: `(project_id, deleted_at)`. Never applies
/// the visibility predicate — the caller decides (`soft_delete_session`,
/// `link_parent`).
pub const SELECT_ROW_VISIBILITY: &str =
    "SELECT project_id, deleted_at FROM agent_sessions WHERE id = ?";

/// The stored parent link of one row (cycle walk, D-11 linkage validation).
pub const SELECT_PARENT_SESSION_ID: &str =
    "SELECT parent_session_id FROM agent_sessions WHERE id = ?";

/// Idempotent, scope-safe soft delete: stamps `deleted_at` on one live row.
pub const SOFT_DELETE_SESSION: &str = "UPDATE agent_sessions SET deleted_at = ?, updated_at = ? WHERE id = ? AND project_id = ? AND deleted_at IS NULL";

/// Flags every child of `parent_session_id = ?` as orphaned (D-12). Runs in
/// the same write batch as the parent delete.
pub const MARK_CHILDREN_ORPHANED: &str =
    "UPDATE agent_sessions SET orphaned = 1, updated_at = ? WHERE parent_session_id = ?";

/// Sets a child's parent link and clears the orphan flag (visible child only).
pub const UPDATE_PARENT_LINK: &str = "UPDATE agent_sessions SET parent_session_id = ?, orphaned = 0, updated_at = ? WHERE id = ? AND project_id = ? AND deleted_at IS NULL";

/// Clears a child's parent link and the orphan flag (visible child only).
pub const CLEAR_PARENT_LINK: &str = "UPDATE agent_sessions SET parent_session_id = NULL, orphaned = 0, updated_at = ? WHERE id = ? AND project_id = ? AND deleted_at IS NULL AND parent_session_id IS NOT NULL";

/// Which optional AND predicates a timeline statement carries. Values are
/// NEVER interpolated: the flags decide *which* `" AND col = ?"` clause
/// exists, the caller binds the matching params in
/// [`TimelinePredicates`] declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimelinePredicates {
    pub since: bool,
    pub until: bool,
    pub agent_id: bool,
    pub session_id: bool,
}

/// Chronological/filtered timeline statement.
///
/// Bind order: project_id, [since], [until], [agent_id], [session_id].
pub fn timeline_sessions_sql(predicates: TimelinePredicates) -> String {
    let mut sql = format!(
        "SELECT {SESSION_COLUMNS} FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL"
    );
    if predicates.since {
        sql.push_str(" AND created_at >= ?");
    }
    if predicates.until {
        sql.push_str(" AND created_at <= ?");
    }
    if predicates.agent_id {
        sql.push_str(" AND agent_id = ?");
    }
    if predicates.session_id {
        sql.push_str(" AND session_id = ?");
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC");
    sql
}

/// Semantic timeline statement: exact `vector_distance_cos` scan (no vector
/// index exists — W1.3 exact-fallback semantics).
///
/// Bind order: query vector, project_id, [since], [until], [agent_id],
/// [session_id].
pub fn semantic_timeline_sessions_sql(predicates: TimelinePredicates) -> String {
    let mut sql = format!(
        "SELECT {SESSION_COLUMNS}, vector_distance_cos(embedding, vector32(?)) AS distance FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL"
    );
    if predicates.since {
        sql.push_str(" AND created_at >= ?");
    }
    if predicates.until {
        sql.push_str(" AND created_at <= ?");
    }
    if predicates.agent_id {
        sql.push_str(" AND agent_id = ?");
    }
    if predicates.session_id {
        sql.push_str(" AND session_id = ?");
    }
    sql.push_str(" ORDER BY distance ASC, id ASC");
    sql
}

// ---------------------------------------------------------------------------
// W4.3 statements (consolidation): milestones, eligibility, scoped delete
// ---------------------------------------------------------------------------

/// Insert one consolidated milestone row.
///
/// The contributors JSON (with the retained per-source summary text, D-27)
/// and the milestone embedding are BOUND values; the embedding rides as
/// JSON text through `vector32(?)`.
pub const INSERT_MILESTONE: &str = "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)";

/// Project-scoped milestone listing (newest first).
///
/// The projection deliberately EXCLUDES the embedding column: the read path
/// never reads the vector back (the embedding is write-only, `#[serde(skip)]`).
pub const LIST_MILESTONES_BY_PROJECT: &str = "SELECT id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, created_at FROM episodic_milestones WHERE project_id = ? ORDER BY created_at DESC, id DESC LIMIT ?";

/// The distinct agents owning at least one live session older than the
/// cutoff (ascending `agent_id`).
pub const DISTINCT_ELIGIBLE_AGENTS: &str = "SELECT DISTINCT agent_id FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL AND created_at < ? ORDER BY agent_id";

/// The eligible sessions of one agent (oldest first, capped at `LIMIT ?`).
///
/// Built as a function because the projection is the shared
/// [`SESSION_COLUMNS`] list (the file's other projection-dependent
/// statements — `timeline_sessions_sql`,
/// `semantic_timeline_sessions_sql` — are builders for the same reason).
///
/// Bind order: project_id, agent_id, cutoff, limit.
pub fn eligible_sessions_for_agent_sql() -> String {
    format!(
        "SELECT {SESSION_COLUMNS} FROM agent_sessions WHERE project_id = ? AND agent_id = ? AND deleted_at IS NULL AND created_at < ? ORDER BY created_at ASC, id ASC LIMIT ?"
    )
}

/// Scoped hard delete of one live source row (W4.3 phase (f)).
///
/// The delete is per id and scoped to `(project_id, agent_id)` with the
/// soft-delete guard, so a row that was soft-deleted between the claim and
/// the batch matches zero rows (`changes = 0`) instead of being removed.
pub const DELETE_SESSION_SCOPED: &str = "DELETE FROM agent_sessions WHERE id = ? AND project_id = ? AND agent_id = ? AND deleted_at IS NULL";
