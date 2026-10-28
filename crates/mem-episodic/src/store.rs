//! `EpisodicStore` — owner of the dedicated `episodic.db` (W4.1; D-2).
//!
//! Storage is a single dedicated database (not a memory shard, not
//! `ai-sessions.db`): the migration set is applied on the first
//! [`ready`](EpisodicStore::ready) call and re-runs are no-ops returning the
//! current version.
//!
//! This file grows by tracer slice: Step 2 lands connection/migration
//! scaffolding, Step 4 the insert/get path, Step 5 list/delete/search and the
//! embedding read-back seam.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_embed::embedder::Embedder;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{
    BatchMode, ConnectFactory, Db, DbHandle, ExecResult, Row, Statement, Value, query_one,
    turso_connect_factory,
};
use mem_store::platform::Platform;
use mem_store::schema_migrations::{EPISODIC_MAX_KNOWN, MigrationOptions, apply_episodic_schema};
use mem_store::tokio::sync::OnceCell;
use mem_store::tracing::warn;
use mem_store::vector::vector_to_text;
use mem_store::vector_utils::parse_extracted_vector;

use crate::error::EpisodicError;
use crate::ids::now_millis;
use crate::raw_context::{RawContext, RawContextWrite};
use crate::record::{AgentSession, NewAgentSession};
use crate::segments::{MAX_SEGMENT_BYTES, RawSegment, split_segments};
use crate::timeline::{
    SessionSummary, TimelineFilter, summary_matches, timeline_limit, timeline_predicate_params,
    timeline_predicates,
};
use crate::validation::{MAX_ID_LEN, check_id, validate, validate_timeline_filter};
use mem_store::episodic_sql as sql;

/// The dedicated episodic database file name (D-2).
pub const EPISODIC_DB_NAME: &str = "episodic.db";

/// Migration label pinned by D-7; set-level since v2 (D-10 delta — the set now
/// spans v1+v2, and per-version identity lives in `SchemaMigration.description`).
pub const EPISODIC_MIGRATION_LABEL: &str = "episodic";

/// The producer of an agent-session row id (`SELECT uuid7_str()` by default).
pub type IdFactory = Arc<dyn Fn() -> String + Send + Sync>;

/// What [`EpisodicStore::upsert_session`](Self::upsert_session) decided
/// (W5.3): the recapture ladder's terminal decisions are observable so the
/// capture layer can truthfully report "no second tier-1 write".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// A fresh triple: the row was inserted.
    Inserted,
    /// A newer capture id updated the live row in place.
    Updated,
    /// The same capture id is already stored (no write).
    AlreadyApplied,
    /// An older capture id arrived; the row keeps the newer content.
    Superseded,
    /// The triple matches a tombstone; nothing was written.
    TombstoneUnchanged,
}

/// The injected time source (W4.3): a millisecond clock the store reads.
///
/// `None` means "use the wall clock"; the consolidation path always injects
/// one so the age threshold is testable without wall-clock waits.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The episodic store over one `episodic.db`.
pub struct EpisodicStore {
    storage_path: PathBuf,
    db_path: PathBuf,
    dimensions: u32,
    connections: ConnectionManager,
    handle: OnceCell<DbHandle>,
    id_factory: Option<IdFactory>,
    clock: Option<Clock>,
    factory: Arc<dyn ConnectFactory>,
}

impl EpisodicStore {
    /// Production constructor over the real turso factory.
    pub fn new(storage_path: PathBuf, store_config: StoreConfig) -> Self {
        Self::with_factory(
            storage_path,
            store_config,
            Arc::new(turso_connect_factory()),
        )
    }

    /// Constructor with an injected [`ConnectFactory`] (test seam).
    pub fn with_factory(
        storage_path: PathBuf,
        store_config: StoreConfig,
        factory: Arc<dyn ConnectFactory>,
    ) -> Self {
        let db_path = storage_path.join(EPISODIC_DB_NAME);
        let dimensions = store_config.embedding_dimensions;
        let connections = ConnectionManager::new(
            storage_path.clone(),
            store_config,
            Platform::current(),
            factory.clone(),
        );
        Self {
            storage_path,
            db_path,
            dimensions,
            connections,
            handle: OnceCell::new(),
            id_factory: None,
            clock: None,
            factory,
        }
    }

    /// Injects a deterministic row-id producer (test seam; production mints
    /// UUIDv7 in SQL).
    pub fn with_id_factory(mut self, factory: IdFactory) -> Self {
        self.id_factory = Some(factory);
        self
    }

    /// Injects a deterministic clock (test seam; production reads the wall
    /// clock through [`now_millis`]).
    ///
    /// Every stamp the store writes (`created_at`, `updated_at`,
    /// `deleted_at`) comes from this seam, so the W4.3 age threshold is
    /// reproducible without sleeping.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The current time in unix millis: the injected clock when present,
    /// else the wall clock.
    pub(crate) fn now(&self) -> i64 {
        match &self.clock {
            Some(clock) => clock(),
            None => now_millis(),
        }
    }

    /// The storage root.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// The on-disk `episodic.db` path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Embedding dimension pinned by migration v1.
    pub fn dimensions(&self) -> u32 {
        self.dimensions
    }

    /// Forgets the initialized handle so the next [`ready`](Self::ready)
    /// re-opens (the store keeps pointing at the same paths).
    pub fn reset(&mut self) {
        self.handle = OnceCell::new();
    }

    /// Re-points the store at another storage root and forgets the handle.
    ///
    /// F4: the injected [`ConnectFactory`] is preserved across the rebind
    /// (the previous hardcoded production factory silently dropped the seam).
    pub fn rebind(&mut self, storage_path: PathBuf, store_config: StoreConfig) {
        self.storage_path = storage_path.clone();
        self.db_path = storage_path.join(EPISODIC_DB_NAME);
        self.dimensions = store_config.embedding_dimensions;
        self.connections = ConnectionManager::new(
            storage_path,
            store_config,
            Platform::current(),
            self.factory.clone(),
        );
        self.handle = OnceCell::new();
    }

    /// Opens the database and applies the episodic migration set (idempotent:
    /// a re-run returns the current version with no error).
    pub async fn ready(&self) -> Result<DbHandle, EpisodicError> {
        let handle = self
            .handle
            .get_or_try_init(|| async { self.open_and_init().await })
            .await?;
        Ok(handle.clone())
    }

    async fn open_and_init(&self) -> Result<DbHandle, EpisodicError> {
        let db = self.connections.get_connection(&self.db_path).await?;
        let options = MigrationOptions {
            db_path: Some(self.db_path.clone()),
            label: EPISODIC_MIGRATION_LABEL,
            max_known: EPISODIC_MAX_KNOWN,
        };
        // W7.1: v1..v5 through the shared runner, then the convergent v6
        // retention schema. `episodic_migrations()` deliberately stays
        // v1..v5; v6 is applied ONLY here.
        apply_episodic_schema(db.as_ref(), self.dimensions, &options).await?;
        Ok(db)
    }

    /// Row count, optionally scoped to one project (`None` is diagnostics/
    /// tests only; every interactive read path is project-scoped).
    pub async fn count_sessions(&self, project_id: Option<&str>) -> Result<u64, EpisodicError> {
        let db = self.ready().await?;
        let (statement, params) = match project_id {
            Some(project_id) => (
                sql::COUNT_SESSIONS_BY_PROJECT,
                vec![Value::Text(project_id.to_string())],
            ),
            None => (sql::COUNT_SESSIONS, Vec::new()),
        };
        let row = query_one(db.as_ref(), statement, params).await?;
        Ok(row.and_then(|row| row.i64("count")).unwrap_or(0).max(0) as u64)
    }

    // -----------------------------------------------------------------------
    // Insert / get (Step 4)
    // -----------------------------------------------------------------------

    /// Validates `new`, mints the row id, embeds the composed document and
    /// inserts the row, returning the minted id.
    ///
    /// Ordering contract: validation → id mint → compose/embed → dimension
    /// check → insert. Nothing is written unless every earlier stage
    /// succeeds, so an invalid record or a failing embedder leaves zero rows.
    pub async fn insert_session(
        &self,
        new: NewAgentSession,
        embedder: &dyn Embedder,
    ) -> Result<String, EpisodicError> {
        let now = self.now();
        let mut session = new.into_session(now);
        validate(&session)?;

        // D-11 "both endpoints visible": the FK only proves *existence*,
        // never scope, so the parent is resolved (and scope-checked) before
        // anything is minted, embedded or written. Keeping this pre-check
        // ahead of the INSERT means the FK branch of `is_unique_violation`
        // stays defensive/unreachable through the public write surface.
        if let Some(parent_id) = session.parent_session_id.clone() {
            let db = self.ready().await?;
            self.ensure_parent_visible(db.as_ref(), &session, &parent_id)
                .await?;
        }

        let db = self.ready().await?;
        let id = self.mint_id(db.as_ref()).await?;
        session.id = id.clone();

        let embedding = crate::embedding::embed_document(embedder, &session).await?;
        if embedding.len() != self.dimensions as usize {
            return Err(EpisodicError::validation(
                "embedding",
                format!(
                    "expected {} dimensions, got {}",
                    self.dimensions,
                    embedding.len()
                ),
            ));
        }
        session.embedding = embedding;

        let metadata_json = session.to_metadata_json()?;
        let params = vec![
            Value::Text(session.id.clone()),
            session
                .capture_id
                .clone()
                .map(Value::Text)
                .unwrap_or(Value::Null),
            Value::Text(session.project_id.clone()),
            Value::Text(session.session_id.clone()),
            Value::Text(session.agent_id.clone()),
            session
                .parent_session_id
                .clone()
                .map(Value::Text)
                .unwrap_or(Value::Null),
            Value::Text(session.summary.clone()),
            Value::Text(metadata_json),
            Value::Text(vector_to_text(&session.embedding)?),
            Value::Integer(session.created_at),
            Value::Integer(session.updated_at),
            Value::Null,
            Value::Integer(0),
        ];
        // One local transaction (batch Write). W5.3: the INSERT ride the
        // atomic batch seam like every other episodic write.
        let statements = vec![Statement::new(sql::INSERT_SESSION, params)];
        match db.batch(statements, BatchMode::Write).await {
            Ok(_) => Ok(id),
            Err(error) if is_unique_violation(&error) => Err(EpisodicError::Conflict),
            Err(error) => Err(EpisodicError::Store(error)),
        }
    }

    /// W5.3 — the capture-aware upsert (one local transaction per call).
    ///
    /// Unique key stays `(project_id, session_id, agent_id)`. For a fresh
    /// triple this inserts; for an existing LIVE row the client-minted
    /// capture ids decide: the same `capture_id` is already applied (no-op),
    /// a lexicographically newer id updates the row in place, an older id
    /// is superseded (no write), and a tombstone is never mutated.
    /// The parent-visibility and embedding-dimension checks re-run on the
    /// update path like on the insert path; nothing is written unless every
    /// earlier stage succeeds.
    pub async fn upsert_session(
        &self,
        new: NewAgentSession,
        embedder: &dyn Embedder,
    ) -> Result<UpsertOutcome, EpisodicError> {
        let now = self.now();
        let mut session = new.into_session(now);
        validate(&session)?;

        if let Some(parent_id) = session.parent_session_id.clone() {
            let db = self.ready().await?;
            self.ensure_parent_visible(db.as_ref(), &session, &parent_id)
                .await?;
        }

        let embedding = crate::embedding::embed_document(embedder, &session).await?;
        if embedding.len() != self.dimensions as usize {
            return Err(EpisodicError::validation(
                "embedding",
                format!(
                    "expected {} dimensions, got {}",
                    self.dimensions,
                    embedding.len()
                ),
            ));
        }
        session.embedding = embedding;

        let db = self.ready().await?;
        let existing_row = query_one(
            db.as_ref(),
            sql::SELECT_SESSION_BY_TRIPLE,
            vec![
                Value::Text(session.project_id.clone()),
                Value::Text(session.session_id.clone()),
                Value::Text(session.agent_id.clone()),
            ],
        )
        .await
        .map_err(EpisodicError::Store)?;

        let Some(existing_row) = existing_row else {
            // Fresh triple: mint the row id and insert (one local Write
            // transaction).
            let id = self.mint_id(db.as_ref()).await?;
            session.id = id.clone();
            let metadata_json = session.to_metadata_json()?;
            let params = vec![
                Value::Text(session.id.clone()),
                session
                    .capture_id
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                Value::Text(session.project_id.clone()),
                Value::Text(session.session_id.clone()),
                Value::Text(session.agent_id.clone()),
                session
                    .parent_session_id
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                Value::Text(session.summary.clone()),
                Value::Text(metadata_json),
                Value::Text(vector_to_text(&session.embedding)?),
                Value::Integer(session.created_at),
                Value::Integer(session.updated_at),
                Value::Null,
                Value::Integer(0),
            ];
            let statements = vec![Statement::new(sql::INSERT_SESSION, params)];
            db.batch(statements, BatchMode::Write).await?;
            return Ok(UpsertOutcome::Inserted);
        };

        // Tombstone: never mutated.
        if !existing_row.is_null("deleted_at") {
            return Ok(UpsertOutcome::TombstoneUnchanged);
        }

        // The capture-id ladder: existing row's id vs the arriving id.
        let existing_capture = existing_row.text("capture_id");
        match session.capture_id.clone() {
            Some(_) if existing_capture.as_deref() == session.capture_id.as_deref() => {
                // Already applied: the exact same capture is stored.
                return Ok(UpsertOutcome::AlreadyApplied);
            }
            Some(arriving) if existing_capture.is_none_or(|old| arriving > old) => {}

            Some(_) => {
                // An older capture id is superseded; the row keeps the
                // newer content.
                return Ok(UpsertOutcome::Superseded);
            }
            None => {
                return Err(EpisodicError::validation(
                    "capture_id",
                    "upsert requires a client-minted capture_id",
                ));
            }
        }

        let metadata_json = session.to_metadata_json()?;
        let existing_id = existing_row.text("id").unwrap_or_default();
        let statements = vec![Statement::new(
            sql::UPDATE_SESSION_RECAPTURE,
            vec![
                Value::Text(session.summary.clone()),
                Value::Text(metadata_json),
                Value::Text(vector_to_text(&session.embedding)?),
                Value::Integer(now),
                Value::Text(session.capture_id.clone().unwrap_or_default()),
                Value::Text(existing_id),
                Value::Text(session.project_id.clone()),
            ],
        )];
        db.batch(statements, BatchMode::Write).await?;
        Ok(UpsertOutcome::Updated)
    }

    /// D-11 parent visibility pre-check (shared by `insert_session` and
    /// `upsert_session`): the parent must exist, be live and be in the
    /// SAME project as the child.
    async fn ensure_parent_visible(
        &self,
        db: &dyn Db,
        session: &AgentSession,
        parent_id: &str,
    ) -> Result<(), EpisodicError> {
        match visible_row_state(db, parent_id).await? {
            None => Err(EpisodicError::NotFound {
                entity: "parent_session".to_string(),
            }),
            Some(parent_row) => {
                if !parent_row.is_null("deleted_at") {
                    return Err(EpisodicError::NotFound {
                        entity: "parent_session".to_string(),
                    });
                }
                if parent_row.text("project_id").as_deref() != Some(session.project_id.as_str()) {
                    Err(EpisodicError::validation(
                        "parent_session_id",
                        "must be in the same project as the child session",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Mints the row id: the injected factory when present (`with_id_factory`
    /// test seam), else SQL-side `SELECT uuid7_str()` (D-1).
    ///
    /// `pub(crate)`: the W4.3 consolidator mints the milestone id through
    /// this same path, so every episodic row id is a UUIDv7 minted by the
    /// store (R4(a)).
    pub(crate) async fn mint_id(&self, db: &dyn Db) -> Result<String, EpisodicError> {
        if let Some(factory) = &self.id_factory {
            return Ok(factory());
        }
        let row = query_one(db, sql::SELECT_UUID7, Vec::new())
            .await?
            .ok_or_else(|| EpisodicError::Io {
                message: "uuid7_str() returned no row".to_string(),
            })?;
        row.text("id").ok_or_else(|| EpisodicError::Io {
            message: "uuid7_str() returned no id column".to_string(),
        })
    }

    /// Reads one session by row id.
    ///
    /// Fail-closed: corrupt/unknown-shape metadata is an
    /// [`EpisodicError::Json`] — a by-id lookup has nothing to skip.
    pub async fn get_session(&self, id: &str) -> Result<Option<AgentSession>, EpisodicError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_SESSION_BY_ID,
            vec![Value::Text(id.to_string())],
        )
        .await?;
        match row {
            Some(row) => Ok(Some(row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // List / delete / search / embedding read-back (Step 5)
    // -----------------------------------------------------------------------

    /// Project-scoped newest-first listing (`created_at DESC`), capped at
    /// `limit` (0 ⇒ empty).
    ///
    /// Corrupt metadata rows are skipped with a warning — the list path has
    /// other rows to serve, so it fails soft; [`get_session`](Self::get_session)
    /// is the fail-closed by-id lookup.
    pub async fn list_sessions(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AgentSession>, EpisodicError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::LIST_SESSIONS,
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Integer(clamp_limit(limit)),
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| match row_to_session(row) {
                Ok(session) => Some(session),
                Err(error) => {
                    warn!(
                        project_id,
                        error = %error,
                        "Skipping episodic row with unreadable metadata"
                    );
                    None
                }
            })
            .collect())
    }

    /// Deletes one row; `false` when no row matched.
    ///
    /// W4.2 (D-13): one `BatchMode::Write` batch of
    /// `MARK_CHILDREN_ORPHANED` and `DELETE_SESSION`, so a crash cannot leave
    /// children visible-but-unflagged. The signature is unchanged (W4.1
    /// suite); the FK `ON DELETE SET NULL` then nulls the children's stored
    /// link.
    pub async fn delete_session(&self, id: &str) -> Result<bool, EpisodicError> {
        let db = self.ready().await?;
        let now = self.now();
        let statements = vec![
            Statement::new(
                sql::MARK_CHILDREN_ORPHANED,
                vec![Value::Integer(now), Value::Text(id.to_string())],
            ),
            Statement::new(sql::DELETE_SESSION, vec![Value::Text(id.to_string())]),
        ];
        let results = db.batch(statements, BatchMode::Write).await?;
        Ok(results
            .last()
            .map(|result| result.changes > 0)
            .unwrap_or(false))
    }

    /// Project-scoped cosine search, nearest first.
    ///
    /// The distance is computed by SQL (`vector_distance_cos`); Rust only
    /// converts it to a clamped similarity for callers. Corrupt metadata rows
    /// are skipped with a warning, like [`list_sessions`](Self::list_sessions).
    pub async fn search_sessions(
        &self,
        project_id: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<SessionSearchHit>, EpisodicError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::SEARCH_SESSIONS,
                vec![
                    Value::Text(vector_to_text(query_vector)?),
                    Value::Text(project_id.to_string()),
                    Value::Integer(clamp_limit(limit)),
                ],
            )
            .await?;
        let mut hits = Vec::new();
        for row in &rows {
            match row_to_session(row) {
                Ok(session) => hits.push(SessionSearchHit {
                    session,
                    similarity: mem_store::vector_utils::distance_to_similarity(
                        row.f64("distance").unwrap_or(1.0),
                    ),
                }),
                Err(error) => {
                    warn!(
                        project_id,
                        error = %error,
                        "Skipping episodic row with unreadable metadata"
                    );
                }
            }
        }
        Ok(hits)
    }

    /// Diagnostic/test seam: the stored embedding of one row.
    pub async fn embedding_for(&self, id: &str) -> Result<Option<Vec<f32>>, EpisodicError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_EMBEDDING_BY_ID,
            vec![Value::Text(id.to_string())],
        )
        .await?;
        Ok(row
            .and_then(|row| row.get("embedding").cloned())
            .and_then(|value| parse_extracted_vector(&value)))
    }

    // -----------------------------------------------------------------------
    // Timeline (W4.2 Step 2)
    // -----------------------------------------------------------------------

    /// The project-scoped chronological/filtered timeline (newest first).
    ///
    /// The SQL statement carries every AND-only predicate as a bound
    /// parameter; the `touched_files` predicate is applied in Rust (D-14) so
    /// `last_n_sessions` truncates *after* every filter. Soft-deleted rows are
    /// invisible (D-11). Corrupt metadata rows are skipped with a warning, like
    /// [`list_sessions`](Self::list_sessions).
    pub async fn timeline(
        &self,
        project_id: &str,
        filter: &TimelineFilter,
    ) -> Result<Vec<SessionSummary>, EpisodicError> {
        check_id("project_id", project_id, MAX_ID_LEN)?;
        validate_timeline_filter(filter)?;
        let limit = timeline_limit(filter);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let mut params = vec![Value::Text(project_id.to_string())];
        params.extend(timeline_predicate_params(filter));
        let rows = db
            .query(
                &sql::timeline_sessions_sql(timeline_predicates(filter)),
                params,
            )
            .await?;
        let mut summaries = Vec::new();
        for row in &rows {
            match row_to_session(row) {
                Ok(session) => {
                    let summary = SessionSummary::from_session(&session);
                    if summary_matches(filter, &summary) {
                        summaries.push(summary);
                    }
                }
                Err(error) => {
                    warn!(
                        project_id,
                        error = %error,
                        "Skipping episodic row with unreadable metadata"
                    );
                }
            }
        }
        summaries.truncate(limit);
        Ok(summaries)
    }

    /// The project-scoped semantic timeline: the query is embedded with
    /// [`embed_query`](crate::embedding::embed_query) (D-15) and ranked by the
    /// exact `vector_distance_cos` scan (there is no vector index in this
    /// tier — W1.3 exact-fallback semantics).
    ///
    /// The dimension assertion mirrors the write path
    /// (`Validation { field: "embedding" }`). Soft-deleted rows are invisible
    /// (D-11); the `touched_files` predicate is applied in Rust (D-14) before
    /// `last_n_sessions` truncates; corrupt metadata rows fail soft.
    pub async fn semantic_timeline(
        &self,
        project_id: &str,
        query: &str,
        embedder: &dyn Embedder,
        filter: &TimelineFilter,
    ) -> Result<Vec<SessionSummary>, EpisodicError> {
        check_id("query", query, crate::validation::MAX_QUERY_BYTES)?;
        check_id("project_id", project_id, MAX_ID_LEN)?;
        validate_timeline_filter(filter)?;
        let limit = timeline_limit(filter);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let query_vector = crate::embedding::embed_query(embedder, query).await?;
        if query_vector.len() != self.dimensions as usize {
            return Err(EpisodicError::validation(
                "embedding",
                format!(
                    "expected {} dimensions, got {}",
                    self.dimensions,
                    query_vector.len()
                ),
            ));
        }
        let mut params = vec![
            Value::Text(vector_to_text(&query_vector)?),
            Value::Text(project_id.to_string()),
        ];
        params.extend(timeline_predicate_params(filter));
        let rows = db
            .query(
                &sql::semantic_timeline_sessions_sql(timeline_predicates(filter)),
                params,
            )
            .await?;
        let mut summaries = Vec::new();
        for row in &rows {
            match row_to_session(row) {
                Ok(session) => {
                    let mut summary = SessionSummary::from_session(&session);
                    summary.similarity = Some(mem_store::vector_utils::distance_to_similarity(
                        row.f64("distance").unwrap_or(1.0),
                    ));
                    if summary_matches(filter, &summary) {
                        summaries.push(summary);
                    }
                }
                Err(error) => {
                    warn!(
                        project_id,
                        error = %error,
                        "Skipping episodic row with unreadable metadata"
                    );
                }
            }
        }
        summaries.truncate(limit);
        Ok(summaries)
    }

    // -----------------------------------------------------------------------
    // Soft delete / visibility / detail (W4.2 Step 4)
    // -----------------------------------------------------------------------

    /// Scope-safe, idempotent soft delete of one row (D-13).
    ///
    /// Returns `false` (writing nothing) when the id is not visible in
    /// `project_id` — unknown, wrong-project or already soft-deleted never
    /// leaks existence. Otherwise one `BatchMode::Write` batch of
    /// `MARK_CHILDREN_ORPHANED` + `SOFT_DELETE_SESSION` (atomic).
    pub async fn soft_delete_session(
        &self,
        project_id: &str,
        id: &str,
    ) -> Result<bool, EpisodicError> {
        check_id("project_id", project_id, MAX_ID_LEN)?;
        check_id("id", id, MAX_ID_LEN)?;
        let db = self.ready().await?;
        let Some(row) = visible_row_state(db.as_ref(), id).await? else {
            return Ok(false);
        };
        if row.text("project_id").as_deref() != Some(project_id) || !row.is_null("deleted_at") {
            return Ok(false);
        }
        let now = self.now();
        let statements = vec![
            Statement::new(
                sql::MARK_CHILDREN_ORPHANED,
                vec![Value::Integer(now), Value::Text(id.to_string())],
            ),
            Statement::new(
                sql::SOFT_DELETE_SESSION,
                vec![
                    Value::Integer(now),
                    Value::Integer(now),
                    Value::Text(id.to_string()),
                    Value::Text(project_id.to_string()),
                ],
            ),
        ];
        let results = db.batch(statements, BatchMode::Write).await?;
        Ok(results
            .last()
            .map(|result| result.changes > 0)
            .unwrap_or(false))
    }

    /// Project-scoped full-record read; soft-deleted rows are invisible
    /// (D-11).
    ///
    /// Fail-closed: corrupt/unknown-shape metadata is an
    /// [`EpisodicError::Json`] — a by-id lookup has nothing to skip.
    pub async fn session_detail(
        &self,
        project_id: &str,
        id: &str,
    ) -> Result<Option<AgentSession>, EpisodicError> {
        check_id("project_id", project_id, MAX_ID_LEN)?;
        check_id("id", id, MAX_ID_LEN)?;
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_SESSION_DETAIL,
            vec![
                Value::Text(id.to_string()),
                Value::Text(project_id.to_string()),
            ],
        )
        .await?;
        match row {
            Some(row) => Ok(Some(row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // Parent/child linkage (W4.2 Step 5)
    // -----------------------------------------------------------------------

    /// Links `child_id` to `parent_id` inside `project_id`.
    ///
    /// Contract (D-11 "both endpoints visible"):
    /// - all three ids are validated; a self-link is a `Validation` error;
    /// - the **child** is resolved first and must be visible in `project_id`
    ///   (missing, wrong project or soft-deleted ⇒
    ///   `NotFound { entity: "child_session" }`);
    /// - the **parent** must be visible (`NotFound { entity:
    ///   "parent_session" }`) and in the same project (`Validation`);
    /// - the cycle check walks `parent_session_id` upward from the parent; a
    ///   `HashSet` of visited ids bounds the walk (loop invariant: every
    ///   visited id is distinct, so an already-cyclic database still
    ///   terminates).
    pub async fn link_parent(
        &self,
        project_id: &str,
        child_id: &str,
        parent_id: &str,
    ) -> Result<(), EpisodicError> {
        check_id("project_id", project_id, MAX_ID_LEN)?;
        check_id("child_id", child_id, MAX_ID_LEN)?;
        check_id("parent_id", parent_id, MAX_ID_LEN)?;
        if child_id == parent_id {
            return Err(EpisodicError::validation(
                "parent_session_id",
                "must not link a session to itself",
            ));
        }
        let db = self.ready().await?;

        // The child branch runs first: a child whose visibility fails never
        // proceeds to any parent resolution.
        match visible_row_state(db.as_ref(), child_id).await? {
            None => {
                return Err(EpisodicError::NotFound {
                    entity: "child_session".to_string(),
                });
            }
            Some(child_row) => {
                if !child_row.is_null("deleted_at")
                    || child_row.text("project_id").as_deref() != Some(project_id)
                {
                    return Err(EpisodicError::NotFound {
                        entity: "child_session".to_string(),
                    });
                }
            }
        }

        let Some(parent_row) = visible_row_state(db.as_ref(), parent_id).await? else {
            return Err(EpisodicError::NotFound {
                entity: "parent_session".to_string(),
            });
        };
        if !parent_row.is_null("deleted_at") {
            return Err(EpisodicError::NotFound {
                entity: "parent_session".to_string(),
            });
        }
        if parent_row.text("project_id").as_deref() != Some(project_id) {
            return Err(EpisodicError::validation(
                "parent_session_id",
                "must be in the same project as the child session",
            ));
        }

        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut current = parent_id.to_string();
        loop {
            if current == child_id {
                return Err(EpisodicError::validation(
                    "parent_session_id",
                    "would create a parent/child cycle",
                ));
            }
            if !visited.insert(current.clone()) {
                // The database already contains a cycle: terminate rather
                // than spin (the loop invariant is enforced by the set).
                break;
            }
            let Some(row) = query_one(
                db.as_ref(),
                sql::SELECT_PARENT_SESSION_ID,
                vec![Value::Text(current)],
            )
            .await?
            else {
                break;
            };
            match row.text("parent_session_id") {
                Some(next) if !next.is_empty() => current = next,
                _ => break,
            }
        }

        let now = self.now();
        let result = db
            .execute(
                sql::UPDATE_PARENT_LINK,
                vec![
                    Value::Text(parent_id.to_string()),
                    Value::Integer(now),
                    Value::Text(child_id.to_string()),
                    Value::Text(project_id.to_string()),
                ],
            )
            .await?;
        if result.changes == 0 {
            // Defensive: the child was visible a moment ago.
            return Err(EpisodicError::NotFound {
                entity: "child_session".to_string(),
            });
        }
        Ok(())
    }

    /// Clears `child_id`'s parent link and orphan flag.
    ///
    /// Idempotent and scope-safe: a soft-deleted (or otherwise invisible)
    /// child can never match the `deleted_at IS NULL` guard, so the answer is
    /// `false` and the stored row is untouched.
    pub async fn unlink_parent(
        &self,
        project_id: &str,
        child_id: &str,
    ) -> Result<bool, EpisodicError> {
        check_id("project_id", project_id, MAX_ID_LEN)?;
        check_id("child_id", child_id, MAX_ID_LEN)?;
        let db = self.ready().await?;
        let now = self.now();
        let result = db
            .execute(
                sql::CLEAR_PARENT_LINK,
                vec![
                    Value::Integer(now),
                    Value::Text(child_id.to_string()),
                    Value::Text(project_id.to_string()),
                ],
            )
            .await?;
        Ok(result.changes > 0)
    }

    // -----------------------------------------------------------------------
    // Consolidation accessors (W4.3)
    // -----------------------------------------------------------------------

    /// The distinct agents that own at least one live session older than
    /// `cutoff` (ascending `agent_id`).
    ///
    /// Read-only: the eligibility query never writes.
    pub async fn eligible_agents(
        &self,
        project_id: &str,
        cutoff: i64,
    ) -> Result<Vec<String>, EpisodicError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::DISTINCT_ELIGIBLE_AGENTS,
                vec![Value::Text(project_id.to_string()), Value::Integer(cutoff)],
            )
            .await?;
        Ok(rows.iter().filter_map(|row| row.text("agent_id")).collect())
    }

    /// Claims the live sessions of one agent that are older than `cutoff`
    /// (oldest first, capped at `limit`).
    ///
    /// Fail-closed: a row whose metadata cannot be decoded propagates as
    /// [`EpisodicError::Json`] — the caller classifies that as a per-agent
    /// skip and deletes nothing for that agent.
    pub async fn claim_sessions(
        &self,
        project_id: &str,
        agent_id: &str,
        cutoff: i64,
        limit: usize,
    ) -> Result<Vec<AgentSession>, EpisodicError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let rows = db
            .query(
                &sql::eligible_sessions_for_agent_sql(),
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Text(agent_id.to_string()),
                    Value::Integer(cutoff),
                    Value::Integer(clamp_limit(limit)),
                ],
            )
            .await?;
        let mut sessions = Vec::with_capacity(rows.len());
        for row in &rows {
            sessions.push(row_to_session(row)?);
        }
        Ok(sessions)
    }

    /// The project-scoped milestone listing (newest first).
    ///
    /// The embedding is NOT read back: the projection does not select it and
    /// [`crate::milestone::Milestone::embedding`] stays empty. Rows whose
    /// contributors JSON cannot be decoded are skipped with a warning — the
    /// list path fails soft, like [`list_sessions`](Self::list_sessions).
    pub async fn list_milestones(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::milestone::Milestone>, EpisodicError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::LIST_MILESTONES_BY_PROJECT,
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Integer(clamp_limit(limit)),
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| match row_to_milestone(row) {
                Ok(milestone) => Some(milestone),
                Err(error) => {
                    warn!(
                        project_id,
                        error = %error,
                        "Skipping milestone row with unreadable contributors"
                    );
                    None
                }
            })
            .collect())
    }

    /// Executes ONE atomic write batch (`BatchMode::Write`).
    ///
    /// This is the single seam the W4.3 consolidator uses for phase (f): the
    /// milestone INSERT, the orphan marks and the scoped DELETEs all ride in
    /// one `IMMEDIATE` transaction, so a crash can never leave a milestone
    /// without its source-row removals (or the other way round).
    pub async fn write_batch(
        &self,
        statements: Vec<Statement>,
    ) -> Result<Vec<ExecResult>, EpisodicError> {
        let db = self.ready().await?;
        Ok(db.batch(statements, BatchMode::Write).await?)
    }

    // -----------------------------------------------------------------------
    // W7.1 — raw context, segments and the agent role
    // -----------------------------------------------------------------------

    /// Stores the authoritative raw context for a triple and its derived
    /// segments in ONE write batch.
    ///
    /// First write wins: when the triple already has raw context the call
    /// returns [`RawContextWrite::AlreadyPresent`] and writes nothing — a
    /// recapture with different bytes is never an overwrite.
    ///
    /// **Single writer.** The read-then-write is safe because the only
    /// production writer is the capture drain, which is serialized by its own
    /// mutex and idempotent on `capture_id`; a second concurrent writer for
    /// the same triple would surface as a primary-key conflict rather than a
    /// silent overwrite.
    pub async fn insert_raw_context(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
        raw_context: &str,
    ) -> Result<RawContextWrite, EpisodicError> {
        crate::raw_context::validate_raw_context(raw_context)?;
        let db = self.ready().await?;
        let triple = vec![
            Value::Text(project_id.to_string()),
            Value::Text(session_id.to_string()),
            Value::Text(agent_id.to_string()),
        ];
        if query_one(db.as_ref(), sql::SELECT_RAW_CONTEXT_BY_TRIPLE, triple)
            .await?
            .is_some()
        {
            return Ok(RawContextWrite::AlreadyPresent);
        }

        let now = self.now();
        let segments = split_segments(raw_context, MAX_SEGMENT_BYTES);
        let mut statements = vec![Statement::new(
            sql::INSERT_RAW_CONTEXT,
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(session_id.to_string()),
                Value::Text(agent_id.to_string()),
                Value::Text(raw_context.to_string()),
                Value::Integer(raw_context.len() as i64),
                Value::Integer(now),
            ],
        )];
        for segment in &segments {
            statements.push(Statement::new(
                sql::INSERT_SEGMENT,
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Text(session_id.to_string()),
                    Value::Text(agent_id.to_string()),
                    Value::Integer(segment.segment_index),
                    Value::Text(segment.raw_content.clone()),
                    Value::Integer(segment.byte_offset),
                    Value::Integer(segment.byte_len),
                    Value::Integer(now),
                ],
            ));
        }
        db.batch(statements, BatchMode::Write).await?;
        Ok(RawContextWrite::Inserted {
            segments: segments.len(),
        })
    }

    /// The stored raw context of one triple (`None` when absent).
    pub async fn raw_context_for_session(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<RawContext>, EpisodicError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_RAW_CONTEXT_BY_TRIPLE,
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(session_id.to_string()),
                Value::Text(agent_id.to_string()),
            ],
        )
        .await?;
        Ok(row.map(|row| RawContext {
            project_id: row.text("project_id").unwrap_or_default(),
            session_id: row.text("session_id").unwrap_or_default(),
            agent_id: row.text("agent_id").unwrap_or_default(),
            raw_context: row.text("raw_context").unwrap_or_default(),
            byte_len: row.i64("byte_len").unwrap_or(0),
            captured_at: row.i64("captured_at").unwrap_or(0),
        }))
    }

    /// One triple's segments, ordered by `segment_index` ascending.
    pub async fn segments_for_session(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Vec<RawSegment>, EpisodicError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::SELECT_SEGMENTS_BY_TRIPLE,
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Text(session_id.to_string()),
                    Value::Text(agent_id.to_string()),
                ],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| RawSegment {
                segment_index: row.i64("segment_index").unwrap_or(0),
                raw_content: row.text("raw_content").unwrap_or_default(),
                byte_offset: row.i64("byte_offset").unwrap_or(0),
                byte_len: row.i64("byte_len").unwrap_or(0),
            })
            .collect())
    }

    /// The stored agent role of one triple (`None` when no session row
    /// exists). The v6 column defaults to `main`.
    pub async fn agent_role_for(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<String>, EpisodicError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_AGENT_ROLE_BY_TRIPLE,
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(session_id.to_string()),
                Value::Text(agent_id.to_string()),
            ],
        )
        .await?;
        Ok(row.and_then(|row| row.text("agent_role")))
    }

    /// Sets the agent role of one triple with a targeted `UPDATE`
    /// (no constructor/field churn).
    ///
    /// The role is validated against the closed ADR-0009 set BEFORE the
    /// write; the error never echoes the value. Returns `false` when no
    /// session row matched.
    pub async fn set_session_agent_role(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
        agent_role: &str,
    ) -> Result<bool, EpisodicError> {
        let role = mem_core::agent::AgentRole::parse(agent_role).map_err(|_| {
            EpisodicError::validation(
                "agent_role",
                "must be one of: main, explorer, reviewer, worker",
            )
        })?;
        let db = self.ready().await?;
        let result = db
            .execute(
                sql::UPDATE_SESSION_AGENT_ROLE,
                vec![
                    Value::Text(role.as_str().to_string()),
                    Value::Text(project_id.to_string()),
                    Value::Text(session_id.to_string()),
                    Value::Text(agent_id.to_string()),
                ],
            )
            .await?;
        Ok(result.changes > 0)
    }
}

/// One search hit: the record plus its cosine similarity (`1 - distance`,
/// clamped into `[0, 1]`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchHit {
    /// The matched record.
    pub session: AgentSession,
    /// Similarity in `[0, 1]` (higher is closer).
    pub similarity: f64,
}

/// `LIMIT ?` argument clamp (a `usize` beyond `i64` cannot be reached in
/// practice; the clamp keeps the cast total).
fn clamp_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// The raw visibility state of one row: `(project_id, deleted_at)`, or `None`
/// when no such row exists. Deliberately unfiltered — the caller applies the
/// D-11 rule (`soft_delete_session`).
async fn visible_row_state(db: &dyn Db, id: &str) -> Result<Option<Row>, EpisodicError> {
    Ok(query_one(
        db,
        sql::SELECT_ROW_VISIBILITY,
        vec![Value::Text(id.to_string())],
    )
    .await?)
}

/// Structured-first constraint classification (F3): `StoreError::Turso
/// { is_constraint: true, .. }` gates the branch, and only then does the
/// lowercased message select the *kind* (`"unique"`).
///
/// The message test is required, not optional: turso maps BOTH
/// `LimboError::Constraint` and `LimboError::ForeignKeyConstraint` to
/// `turso::Error::Constraint`, so `is_constraint` alone would report an FK
/// failure as [`EpisodicError::Conflict`]. The FK half is defensive and
/// unreachable through the public write surface while `insert_session`'s
/// parent pre-check stands.
fn is_unique_violation(error: &mem_store::error::StoreError) -> bool {
    let mem_store::error::StoreError::Turso {
        is_constraint: true,
        ..
    } = error
    else {
        // W5.3 divergence (documented): the `transactional_batch` path wraps
        // constraint errors WITHOUT the structured `is_constraint` flag
        // ("batch statement 0 failed: …"), so the batched INSERT classifies
        // via the message text alone — the message is the SQLite engine's
        // "UNIQUE constraint failed" literal, never interpolation.
        if let mem_store::error::StoreError::Turso { message, .. } = error {
            return message.to_ascii_lowercase().contains("unique");
        }
        return false;
    };
    error.message().to_ascii_lowercase().contains("unique")
}

/// Maps one `agent_sessions` row to a record (fail-closed on metadata).
fn row_to_session(row: &Row) -> Result<AgentSession, EpisodicError> {
    let metadata_text = row.text("metadata").unwrap_or_default();
    let metadata = crate::payload::SessionMetadata::from_json_str(&metadata_text)?;
    let embedding_value = row.get("embedding").cloned().unwrap_or(Value::Null);
    let embedding = parse_extracted_vector(&embedding_value).unwrap_or_default();
    Ok(AgentSession {
        id: row.text("id").unwrap_or_default(),
        project_id: row.text("project_id").unwrap_or_default(),
        session_id: row.text("session_id").unwrap_or_default(),
        agent_id: row.text("agent_id").unwrap_or_default(),
        parent_session_id: row.text("parent_session_id"),
        summary: row.text("summary").unwrap_or_default(),
        metadata,
        embedding,
        created_at: row.i64("created_at").unwrap_or(0),
        updated_at: row.i64("updated_at").unwrap_or(0),
        deleted_at: row.i64("deleted_at"),
        orphaned: row.i64("orphaned").unwrap_or(0) != 0,
        capture_id: row.text("capture_id"),
    })
}

/// Maps one `episodic_milestones` row to a record (fail-closed on the
/// contributors JSON).
///
/// The embedding is never read back: the projection does not select the
/// column, so [`crate::milestone::Milestone::embedding`] stays empty.
fn row_to_milestone(row: &Row) -> Result<crate::milestone::Milestone, EpisodicError> {
    let contributors_json = row.text("contributors").unwrap_or_default();
    let contributors = crate::milestone::decode_contributors(&contributors_json)?;
    Ok(crate::milestone::Milestone {
        id: row.text("id").unwrap_or_default(),
        project_id: row.text("project_id").unwrap_or_default(),
        agent_id: row.text("agent_id").unwrap_or_default(),
        summary: row.text("summary").unwrap_or_default(),
        contributors,
        source_count: row.i64("source_count").unwrap_or(0),
        window_start: row.i64("window_start").unwrap_or(0),
        window_end: row.i64("window_end").unwrap_or(0),
        embedding: Vec::new(),
        created_at: row.i64("created_at").unwrap_or(0),
    })
}
