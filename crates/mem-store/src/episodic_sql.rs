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

// ---------------------------------------------------------------------------
// v6 — the retention schema (W7.1). Net-new; no upstream oracle.
// ---------------------------------------------------------------------------
//
// EVERY omission below is the contract:
//
// - no `raw_context` column on `agent_sessions`: the retention table is the
//   ONE authoritative representation (M1);
// - no `FOREIGN KEY` / `ON DELETE` / trigger: consolidation deletes a session
//   ROW and must never destroy the raw context. There is no schema feature
//   whose job is to destroy it (ADR-0010 D-1);
// - no `CHECK` on `agent_role`: `AgentRole::parse` already enforces the
//   closed set, and a `CHECK` would cost a table rebuild to extend (Q6).

/// v6 additive column: the agent role on the session row. Existing rows
/// default to `main`; the capture path sets it with one targeted `UPDATE`.
pub const EPISODIC_V6_ALTERS: [&str; 1] =
    ["ALTER TABLE agent_sessions ADD COLUMN agent_role TEXT NOT NULL DEFAULT 'main'"];

/// v7 additive column (issue 110): the capture inbox's retry schedule.
///
/// Nullable and NO default on purpose — `NULL` means "no schedule", which is
/// exactly the pre-v7 behaviour, so every existing row stays immediately
/// eligible without a backfill. A `NOT NULL`/`DEFAULT` variant would rewrite
/// the meaning of those rows and REFUSES on a populated table.
pub const EPISODIC_V7_ALTERS: [&str; 1] =
    ["ALTER TABLE capture_inbox ADD COLUMN not_before INTEGER"];

/// v6 DDL: the authoritative raw-context table, keyed by the session's
/// logical identity — the same triple `agent_sessions` declares UNIQUE.
///
/// No FK: consolidation deletes a row, not an identity, so the triple
/// survives it and the raw bytes stay readable.
pub const RAW_CONTEXT_DDL: &str = "CREATE TABLE IF NOT EXISTS episodic_raw_context (\n\
 \x20 project_id TEXT NOT NULL,\n\
 \x20 session_id TEXT NOT NULL,\n\
 \x20 agent_id TEXT NOT NULL,\n\
 \x20 raw_context TEXT NOT NULL,\n\
 \x20 byte_len INTEGER NOT NULL,\n\
 \x20 captured_at INTEGER NOT NULL,\n\
 \x20 PRIMARY KEY (project_id, session_id, agent_id)\n\
 \x20)";

/// v6 DDL: the derived segment table. Regenerable from the raw context, so
/// it is safe to purge and safe to rebuild; offsets are contiguous and
/// `concat(raw_content) == raw_context` byte for byte.
pub const SEGMENTS_DDL: &str = "CREATE TABLE IF NOT EXISTS episodic_segments (\n\
 \x20 project_id TEXT NOT NULL,\n\
 \x20 session_id TEXT NOT NULL,\n\
 \x20 agent_id TEXT NOT NULL,\n\
 \x20 segment_index INTEGER NOT NULL,\n\
 \x20 raw_content TEXT NOT NULL,\n\
 \x20 byte_offset INTEGER NOT NULL,\n\
 \x20 byte_len INTEGER NOT NULL,\n\
 \x20 created_at INTEGER NOT NULL,\n\
 \x20 PRIMARY KEY (project_id, session_id, agent_id, segment_index)\n\
 \x20)";

/// The v6 segment index (ordered reads per session).
pub const SEGMENTS_INDEX: &str = "CREATE INDEX IF NOT EXISTS idx_episodic_segments_session ON episodic_segments(project_id, session_id, agent_id, segment_index)";

// ---------------------------------------------------------------------------
// v6 statements — raw context, segments, agent role (W7.1)
// ---------------------------------------------------------------------------

/// Insert the authoritative raw-context row (one per logical triple).
pub const INSERT_RAW_CONTEXT: &str = "INSERT INTO episodic_raw_context (project_id, session_id, agent_id, raw_context, byte_len, captured_at) VALUES (?, ?, ?, ?, ?, ?)";

/// Insert one derived segment.
pub const INSERT_SEGMENT: &str = "INSERT INTO episodic_segments (project_id, session_id, agent_id, segment_index, raw_content, byte_offset, byte_len, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

/// Read the authoritative raw context of one triple.
pub const SELECT_RAW_CONTEXT_BY_TRIPLE: &str = "SELECT project_id, session_id, agent_id, raw_context, byte_len, captured_at FROM episodic_raw_context WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// Read one triple's segments, ordered by `segment_index`.
pub const SELECT_SEGMENTS_BY_TRIPLE: &str = "SELECT project_id, session_id, agent_id, segment_index, raw_content, byte_offset, byte_len, created_at FROM episodic_segments WHERE project_id = ? AND session_id = ? AND agent_id = ? ORDER BY segment_index ASC";

/// The stored role of one triple (the v6 column; `main` by default).
pub const SELECT_AGENT_ROLE_BY_TRIPLE: &str = "SELECT agent_role FROM agent_sessions WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// The targeted role write (never a constructor change, never 29 literals).
pub const UPDATE_SESSION_AGENT_ROLE: &str = "UPDATE agent_sessions SET agent_role = ? WHERE project_id = ? AND session_id = ? AND agent_id = ?";

// ---------------------------------------------------------------------------
// W7.1 purge statements (D6). Every predicate is the BOUND triple or a bound
// row/milestone id — no interpolation, no FK, no cascade, no trigger.
// ---------------------------------------------------------------------------

/// The row ids of a triple, INCLUDING soft-deleted rows (the purge deletes
/// live and soft-deleted alike).
pub const SELECT_SESSION_IDS_BY_TRIPLE: &str =
    "SELECT id FROM agent_sessions WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// Delete every session row of the triple.
pub const PURGE_SESSIONS_BY_TRIPLE: &str =
    "DELETE FROM agent_sessions WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// Delete the authoritative raw context of the triple.
pub const PURGE_RAW_CONTEXT_BY_TRIPLE: &str =
    "DELETE FROM episodic_raw_context WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// Delete the derived segments of the triple.
pub const PURGE_SEGMENTS_BY_TRIPLE: &str =
    "DELETE FROM episodic_segments WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// Delete every inbox row of the triple (a queued payload IS raw content).
pub const PURGE_INBOX_BY_TRIPLE: &str =
    "DELETE FROM capture_inbox WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// The milestones a pre-scan must inspect: the project's milestones of the
/// scope's agent (the contributor JSON is decoded in Rust, no JSON SQL).
pub const LIST_MILESTONES_FOR_PURGE: &str =
    "SELECT id, contributors FROM episodic_milestones WHERE project_id = ? AND agent_id = ?";

/// v4 additive column (D-36): the client-minted `capture_id` on
/// `agent_sessions`. Nullable, no default — rows written before the W5.1
/// bridge carry NULL, and W5.1 inserts no value (W5.3 owns the write path).
pub const EPISODIC_V4_ALTERS: [&str; 1] = ["ALTER TABLE agent_sessions ADD COLUMN capture_id TEXT"];

/// v5 DDL: the W5.3 durable capture inbox (`episodic.db`). `payload` is the
/// raw request JSON as received; `status` is `queued`/`dead` (`done` rows
/// never persist — `mark_done` purges in the same write).
pub const CAPTURE_INBOX_DDL: &str = "CREATE TABLE IF NOT EXISTS capture_inbox (\n\
 \x20 capture_id TEXT PRIMARY KEY,\n\
 \x20 project_id TEXT NOT NULL,\n\
 \x20 session_id TEXT NOT NULL,\n\
 \x20 agent_id TEXT NOT NULL,\n\
 \x20 payload TEXT NOT NULL,\n\
 \x20 created_at INTEGER NOT NULL,\n\
 \x20 attempts INTEGER NOT NULL DEFAULT 0,\n\
 \x20 last_attempt_at INTEGER,\n\
 \x20 status TEXT NOT NULL DEFAULT 'queued'\n\
 \x20)";

/// v5 indexes: FIFO dequeue order and the project scan.
pub const CAPTURE_INBOX_V5_INDEXES: [&str; 2] = [
    "CREATE INDEX IF NOT EXISTS idx_capture_inbox_status_created ON capture_inbox(status, created_at)",
    "CREATE INDEX IF NOT EXISTS idx_capture_inbox_project ON capture_inbox(project_id)",
];

/// Idempotent, capacity-checked enqueue in ONE conditional statement:
/// inserts only when no row with the same `capture_id` exists AND fewer
/// than the capacity bound of rows are queued. `changes == 0` means
/// "duplicate or full" and the caller disambiguates with `SELECT_CAPTURE_INBOX_COUNT_FOR_ID`
/// (no count-then-insert race: the predicate is evaluated inside the
/// statement's own write transaction).
pub const INSERT_CAPTURE_INBOX_CONDITIONAL: &str = "INSERT INTO capture_inbox (capture_id, project_id, session_id, agent_id, payload, created_at, attempts, last_attempt_at, status) SELECT ?, ?, ?, ?, ?, ?, 0, NULL, 'queued' WHERE NOT EXISTS (SELECT 1 FROM capture_inbox WHERE capture_id = ?) AND (SELECT COUNT(*) FROM capture_inbox WHERE status = 'queued') < ?";

/// FIFO dequeue of the entries whose retry schedule has ELAPSED: oldest
/// queued first, capture_id tie-break.
///
/// `status = 'queued'` is the leading equality on
/// `idx_capture_inbox_status_created`, so `(not_before IS NULL OR not_before
/// <= ?)` is a residual filter on the rows that seek already returns — it
/// never changes the access path (D-110-4).
///
/// Bind order: now, limit.
pub const SELECT_CAPTURE_INBOX_BATCH: &str = "SELECT capture_id, project_id, session_id, agent_id, payload, created_at, attempts, last_attempt_at, status FROM capture_inbox WHERE status = 'queued' AND (not_before IS NULL OR not_before <= ?) ORDER BY created_at ASC, capture_id ASC LIMIT ?";

/// D-48 done path: the done/delete statements in one statement — the row
/// is purged (the inbox never stores terminal rows; the status column
/// exists for the dead-letter state).
pub const DELETE_CAPTURE_INBOX_BY_ID: &str = "DELETE FROM capture_inbox WHERE capture_id = ?";

/// Consume one attempt on a retryable failure.
pub const BUMP_CAPTURE_INBOX_ATTEMPTS: &str =
    "UPDATE capture_inbox SET attempts = attempts + 1, last_attempt_at = ? WHERE capture_id = ?";

/// Issue 110 — the retry schedule write: the row stays `queued` and becomes
/// eligible again at `not_before` (a bound absolute instant). Bind order:
/// not_before, last_attempt_at, capture_id.
pub const SCHEDULE_CAPTURE_INBOX_RETRY: &str =
    "UPDATE capture_inbox SET not_before = ?, last_attempt_at = ? WHERE capture_id = ?";

/// Issue 110 — the stored schedule of one row (`NULL` = immediately eligible).
pub const SELECT_CAPTURE_INBOX_NOT_BEFORE: &str =
    "SELECT not_before FROM capture_inbox WHERE capture_id = ?";

/// Dead-letter: the terminal episodic-only state.
pub const DEADLETTER_CAPTURE_INBOX: &str =
    "UPDATE capture_inbox SET status = 'dead', last_attempt_at = ? WHERE capture_id = ?";

/// Queued-row count (capacity accounting + tests).
pub const COUNT_CAPTURE_INBOX_QUEUED: &str =
    "SELECT COUNT(*) AS count FROM capture_inbox WHERE status = 'queued'";

/// Disambiguates a zero-change conditional insert: does the capture_id
/// already exist?
pub const SELECT_CAPTURE_INBOX_COUNT_FOR_ID: &str =
    "SELECT COUNT(*) AS count FROM capture_inbox WHERE capture_id = ?";

/// Distinct live project ids (the scheduler's bounded per-tick scan).
/// `ORDER BY project_id` keeps scan order deterministic across ticks.
pub const DISTINCT_LIVE_PROJECT_IDS: &str = "SELECT DISTINCT project_id FROM agent_sessions WHERE deleted_at IS NULL ORDER BY project_id LIMIT ?";

// ---------------------------------------------------------------------------
// Query statements (net-new, parameterized)
// ---------------------------------------------------------------------------

/// Column projection shared by every read path; `vector_extract` turns the
/// F32 blob back into JSON text for transport across the `Db` seam. The two
/// v2 columns ride along on every projection (D-11 visibility).
pub const SESSION_COLUMNS: &str = "id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned";

/// SQL-side UUIDv7 minting (turso_core 0.8.3 registers `uuid7_str()`); no
/// `uuid`/`rand` dependency edge.
pub const SELECT_UUID7: &str = "SELECT uuid7_str() AS id";

/// Insert one session row. The embedding is bound as JSON text through
/// `vector32(?)`; the two v2 columns are always bound explicitly (D-9
/// defaults are for rows that bypass this path).
pub const INSERT_SESSION: &str = "INSERT INTO agent_sessions (id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, embedding, created_at, updated_at, deleted_at, orphaned) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?, ?, ?, ?)";

/// Read-back by row id (the only id-only lookup; every other read path is
/// project-scoped). Soft-deleted rows are invisible (D-11).
pub const SELECT_SESSION_BY_ID: &str = "SELECT id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE id = ? AND deleted_at IS NULL";

/// Project-scoped by-id detail read (soft-deleted rows invisible, D-11).
pub const SELECT_SESSION_DETAIL: &str = "SELECT id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE id = ? AND project_id = ? AND deleted_at IS NULL";

/// Project-scoped newest-first listing (`created_at DESC`, id tie-break).
pub const LIST_SESSIONS: &str = "SELECT id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT ?";

/// Project-scoped cosine search, nearest first.
pub const SEARCH_SESSIONS: &str = "SELECT id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned, vector_distance_cos(embedding, vector32(?)) AS distance FROM agent_sessions WHERE project_id = ? AND deleted_at IS NULL ORDER BY distance ASC, id ASC LIMIT ?";

/// Delete by row id (the hard-delete purge path; `delete_session` prefixes
/// it with `MARK_CHILDREN_ORPHANED` in one write batch).
pub const DELETE_SESSION: &str = "DELETE FROM agent_sessions WHERE id = ?";

/// The raw lookup by the upsert key `(project_id, session_id, agent_id)`,
/// WITH soft-deleted rows: the tombstone branch must find the row to
/// refuse mutating it.
pub const SELECT_SESSION_BY_TRIPLE: &str = "SELECT id, capture_id, project_id, session_id, agent_id, parent_session_id, summary, metadata, vector_extract(embedding) AS embedding, created_at, updated_at, deleted_at, orphaned FROM agent_sessions WHERE project_id = ? AND session_id = ? AND agent_id = ?";

/// W5.3 recapture update: rewrites the live row's content in place
/// (summary, metadata, embedding, updated_at, capture_id). The guard keeps
/// the row live and its project scope; the unique triple is unchanged.
pub const UPDATE_SESSION_RECAPTURE: &str = "UPDATE agent_sessions SET summary = ?, metadata = ?, embedding = vector32(?), updated_at = ?, capture_id = ? WHERE id = ? AND project_id = ? AND deleted_at IS NULL";

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

/// W6.4 — the milestone delete used by the portability import's replace
/// mode (the DELETE that precedes the re-INSERT in the SAME atomic batch).
///
/// The delete is keyed by the milestone **id** alone: the import rewrites
/// the milestone's `project_id` to the TARGET's project, so a
/// project-scoped predicate would leave the source row behind (and a
/// milestone imported into a different project would never be replaced).
/// The milestone id is the row's primary key and is preserved across the
/// export/import round trip, so the id is the only stable handle.
pub const DELETE_MILESTONE: &str = "DELETE FROM episodic_milestones WHERE id = ?";

/// W6.4 — the milestone id existence probe used by the portability
/// import's pre-scan.
///
/// The milestone id is the row's **primary key**, so the collision probe
/// is GLOBAL and not project-scoped: a milestone of another project
/// collides just the same, so a project-scoped predicate would let the
/// import fail at the INSERT (after the fact tiers were already written).
/// The probe lets the import abort BEFORE the first write.
pub const SELECT_MILESTONE_ID_BY_ID: &str = "SELECT id FROM episodic_milestones WHERE id = ?";

/// Project-scoped milestone listing (newest first).
///
/// The projection deliberately EXCLUDES the embedding column: the read path
/// never reads the vector back (the embedding is write-only, `#[serde(skip)]`).
pub const LIST_MILESTONES_BY_PROJECT: &str = "SELECT id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, created_at FROM episodic_milestones WHERE project_id = ? ORDER BY created_at DESC, id DESC LIMIT ?";

/// The project roster (issue 116, D-6): the union of live session projects and
/// milestone projects, with one count per side and the greatest activity stamp.
///
/// The union is the point. Consolidation **hard**-deletes its source sessions
/// (`DELETE_SESSION_SCOPED`), so a sessions-only roster loses every
/// well-consolidated project; milestones alone have no history for a project
/// that was never consolidated.
///
/// `last_activity_at` is `COALESCE(session stamp, milestone stamp, 0)`: the
/// newest LIVE session wins when the project still has one, and the milestone
/// stamp is the fallback for a fully consolidated project (the consolidation
/// hard-deletes its sources, so that side is usually `NULL` exactly then).
/// The soft-delete filter lives in BOTH session subqueries, so a soft-deleted
/// session is neither counted nor dated.
///
/// Deterministic order: `last_activity_at DESC, project_id ASC`. Bind: limit.
pub const PROJECT_ROSTER: &str = "SELECT p.project_id AS project_id, \
        COALESCE(s.session_count, 0) AS session_count, \
        COALESCE(m.milestone_count, 0) AS milestone_count, \
        COALESCE(s.last_activity_at, m.last_activity_at, 0) AS last_activity_at \
        FROM (SELECT project_id FROM agent_sessions WHERE deleted_at IS NULL \
              UNION \
              SELECT project_id FROM episodic_milestones) AS p \
        LEFT JOIN (SELECT project_id, COUNT(*) AS session_count, MAX(created_at) AS last_activity_at \
                   FROM agent_sessions WHERE deleted_at IS NULL GROUP BY project_id) AS s \
               ON s.project_id = p.project_id \
        LEFT JOIN (SELECT project_id, COUNT(*) AS milestone_count, MAX(created_at) AS last_activity_at \
                   FROM episodic_milestones GROUP BY project_id) AS m \
               ON m.project_id = p.project_id \
        ORDER BY last_activity_at DESC, project_id ASC \
        LIMIT ?";

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
