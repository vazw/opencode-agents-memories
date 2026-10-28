//! `FactsStore` — the owner of the dedicated `facts.db` (W7.1).
//!
//! One file per storage root (Q1); rows are `project_id`-scoped and carry a
//! `tier` discriminator. The migration set is applied on the first
//! [`ready`](FactsStore::ready) and re-runs are no-ops.
//!
//! The durable facts DB is NOT `mem_portability::facts::FactStore` (the
//! tier-1 memory seam): different crate, different file, different concept
//! (M30).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_core::dedup::normalize_key;
use mem_core::tier::MemoryTier;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{
    BatchMode, ConnectFactory, DbHandle, Row, Statement, Value, query_one, turso_connect_factory,
};
use mem_store::error::StoreError;
use mem_store::platform::Platform;
use mem_store::schema_migrations::{MigrationOptions, apply_schema_migrations};
use mem_store::tokio::sync::OnceCell;

use crate::FactsError;
use crate::ids::now_millis;
use crate::schema as sql;

/// The migration label (tracing only).
pub const FACTS_MIGRATION_LABEL: &str = "facts";

/// One contributing session behind a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactSource {
    /// Project scope (must equal the fact's project).
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope.
    pub agent_id: String,
    /// The milestone whose consolidation produced the fact.
    pub milestone_id: String,
}

/// A fact to write.
#[derive(Debug, Clone, PartialEq)]
pub struct FactInput {
    /// Project scope.
    pub project_id: String,
    /// One of the three fact tiers (validated).
    pub tier: MemoryTier,
    /// The fact text.
    pub content: String,
    /// The explicit key; `None` means "the content, normalized".
    pub key: Option<String>,
    /// The milestone id (the `source_ref` chain).
    pub source_ref: Option<String>,
    /// Confidence (default 1.0).
    pub confidence: Option<f64>,
    /// Free-form metadata JSON.
    pub metadata: Option<String>,
    /// Optional embedding (no producer this wave).
    pub embedding: Option<Vec<f32>>,
    /// Optional embedding model id.
    pub model_id: Option<String>,
    /// At least one contributing session (validated).
    pub sources: Vec<FactSource>,
}

/// What an insert did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactWrite {
    /// The fact did not exist and was inserted at version 1.
    Created,
    /// The fact existed and was updated (version incremented).
    Updated,
}

/// One stored fact.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// Deterministic row id.
    pub id: String,
    /// Project scope.
    pub project_id: String,
    /// Fact tier.
    pub tier: MemoryTier,
    /// Fact text.
    pub content: String,
    /// The normalized dedup key.
    pub key_normalized: String,
    /// Embedding model id, when one was recorded.
    pub model_id: Option<String>,
    /// The model-staleness flag.
    pub stale: i64,
    /// Generation stamp, when recorded.
    pub generated_at: Option<i64>,
    /// The milestone id.
    pub source_ref: Option<String>,
    /// Confidence.
    pub confidence: f64,
    /// Optimistic version (bumped on every upsert).
    pub version: i64,
    /// Free-form metadata JSON.
    pub metadata: Option<String>,
    /// Creation stamp.
    pub created_at: i64,
    /// Last-write stamp.
    pub updated_at: i64,
    /// Soft-delete stamp (`None` = live).
    pub deleted_at: Option<i64>,
}

impl Fact {
    /// Whether the row is soft-deleted (kept, hidden from live reads).
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// The facts store over one `facts.db`.
pub struct FactsStore {
    storage_path: PathBuf,
    db_path: PathBuf,
    dimensions: u32,
    connections: ConnectionManager,
    handle: OnceCell<DbHandle>,
}

impl FactsStore {
    /// Production constructor over the real turso factory.
    pub fn new(store_config: StoreConfig) -> Self {
        Self::with_factory(store_config, Arc::new(turso_connect_factory()))
    }

    /// Constructor with an injected [`ConnectFactory`] (test seam).
    pub fn with_factory(store_config: StoreConfig, factory: Arc<dyn ConnectFactory>) -> Self {
        let storage_path = store_config.storage_path.clone();
        let db_path = storage_path.join(sql::FACTS_DB_NAME);
        let dimensions = store_config.embedding_dimensions;
        let connections = ConnectionManager::new(
            storage_path.clone(),
            store_config,
            Platform::current(),
            factory,
        );
        Self {
            storage_path,
            db_path,
            dimensions,
            connections,
            handle: OnceCell::new(),
        }
    }

    /// The storage root.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// The on-disk `facts.db` path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Opens the database and applies the v1 schema (idempotent).
    pub async fn ready(&self) -> Result<DbHandle, FactsError> {
        let handle = self
            .handle
            .get_or_try_init(|| async { self.open_and_init().await })
            .await?;
        Ok(handle.clone())
    }

    async fn open_and_init(&self) -> Result<DbHandle, FactsError> {
        let db = self.connections.get_connection(&self.db_path).await?;
        let options = MigrationOptions {
            db_path: Some(self.db_path.clone()),
            label: FACTS_MIGRATION_LABEL,
        };
        // The set is v1-only today (no backup); the proof and settle keep the
        // site honest for a future v2 (issue 46.4).
        let root = self.connections.exclusive_root();
        match apply_schema_migrations(
            db.as_ref(),
            &sql::facts_migrations(self.dimensions),
            &options,
            root.as_ref(),
        )
        .await
        {
            Ok(_) => Ok(db),
            Err(StoreError::MigrationRestorePending(pending)) => {
                let original = pending.original().clone();
                match pending.restore(&self.connections, db).await {
                    Ok(()) => Err(original.into()),
                    Err(restore_error) => Err(restore_error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The applied `facts.db` schema version (1 for this wave).
    pub async fn schema_version(&self) -> Result<i64, FactsError> {
        let db = self.ready().await?;
        let row = query_one(db.as_ref(), "PRAGMA user_version", Vec::new()).await?;
        Ok(row.and_then(|row| row.i64("user_version")).unwrap_or(0))
    }

    /// Inserts or upserts one fact, with its provenance rows, in ONE write
    /// batch.
    pub async fn insert(&self, input: FactInput) -> Result<FactWrite, FactsError> {
        validate(&input)?;
        let key_normalized = normalize_key(input.key.as_deref().unwrap_or(&input.content));
        let id = sql::fact_id(&input.project_id, input.tier, &key_normalized);
        let now = now_millis();
        let confidence = input.confidence.unwrap_or(1.0);
        let db = self.ready().await?;
        let existing = query_one(
            db.as_ref(),
            sql::SELECT_FACT_BY_ID,
            vec![Value::Text(id.clone())],
        )
        .await?;

        let mut statements = Vec::new();
        let write = match existing {
            Some(_) => {
                statements.push(Statement::new(
                    sql::UPDATE_FACT,
                    vec![
                        Value::Text(input.content.clone()),
                        Value::Text(key_normalized.clone()),
                        input
                            .source_ref
                            .clone()
                            .map(Value::Text)
                            .unwrap_or(Value::Null),
                        Value::Real(confidence),
                        input
                            .metadata
                            .clone()
                            .map(Value::Text)
                            .unwrap_or(Value::Null),
                        Value::Integer(now),
                        Value::Text(id.clone()),
                    ],
                ));
                FactWrite::Updated
            }
            None => {
                let mut params = vec![
                    Value::Text(id.clone()),
                    Value::Text(input.project_id.clone()),
                    Value::Text(input.tier.as_str().to_string()),
                    Value::Text(input.content.clone()),
                    Value::Text(key_normalized.clone()),
                ];
                let statement = match &input.embedding {
                    Some(embedding) => {
                        params.push(Value::Text(mem_store::vector::vector_to_text(embedding)?));
                        params.push(
                            input
                                .model_id
                                .clone()
                                .map(Value::Text)
                                .unwrap_or(Value::Null),
                        );
                        params.push(
                            input
                                .source_ref
                                .clone()
                                .map(Value::Text)
                                .unwrap_or(Value::Null),
                        );
                        params.push(Value::Real(confidence));
                        params.push(
                            input
                                .metadata
                                .clone()
                                .map(Value::Text)
                                .unwrap_or(Value::Null),
                        );
                        params.push(Value::Integer(now));
                        params.push(Value::Integer(now));
                        sql::INSERT_FACT_WITH_EMBEDDING
                    }
                    None => {
                        params.push(
                            input
                                .model_id
                                .clone()
                                .map(Value::Text)
                                .unwrap_or(Value::Null),
                        );
                        params.push(
                            input
                                .source_ref
                                .clone()
                                .map(Value::Text)
                                .unwrap_or(Value::Null),
                        );
                        params.push(Value::Real(confidence));
                        params.push(
                            input
                                .metadata
                                .clone()
                                .map(Value::Text)
                                .unwrap_or(Value::Null),
                        );
                        params.push(Value::Integer(now));
                        params.push(Value::Integer(now));
                        sql::INSERT_FACT_NO_EMBEDDING
                    }
                };
                statements.push(Statement::new(statement, params));
                FactWrite::Created
            }
        };

        for source in &input.sources {
            statements.push(Statement::new(
                sql::INSERT_PROVENANCE,
                vec![
                    Value::Text(id.clone()),
                    Value::Text(source.project_id.clone()),
                    Value::Text(source.session_id.clone()),
                    Value::Text(source.agent_id.clone()),
                    Value::Text(source.milestone_id.clone()),
                    Value::Integer(now),
                ],
            ));
        }
        db.batch(statements, BatchMode::Write).await?;
        Ok(write)
    }

    /// Reads one fact by id, INCLUDING a soft-deleted row (the row is kept;
    /// [`Fact::is_deleted`] tells the caller).
    pub async fn get(&self, id: &str) -> Result<Option<Fact>, FactsError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::SELECT_FACT_BY_ID,
            vec![Value::Text(id.to_string())],
        )
        .await?;
        row.as_ref().map(row_to_fact).transpose()
    }

    /// Reads the live fact for `(project, tier, key)`; `key` is normalized
    /// first, so whitespace/case variants address the same row.
    pub async fn get_by_key(
        &self,
        project_id: &str,
        tier: MemoryTier,
        key: &str,
    ) -> Result<Option<Fact>, FactsError> {
        if !tier.is_fact_tier() {
            return Err(FactsError::validation(
                "tier",
                "must be one of: semantic, procedural, resource",
            ));
        }
        let id = sql::fact_id(project_id, tier, key);
        let fact = self.get(&id).await?;
        Ok(fact.filter(|fact| !fact.is_deleted()))
    }

    /// Live facts of a project, `updated_at DESC, id DESC`.
    pub async fn list(&self, project_id: &str, limit: usize) -> Result<Vec<Fact>, FactsError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let rows = db
            .query(
                sql::LIST_FACTS,
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)),
                ],
            )
            .await?;
        rows.iter().map(row_to_fact).collect()
    }

    /// The live fact count of a project.
    pub async fn count(&self, project_id: &str) -> Result<u64, FactsError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            sql::COUNT_FACTS,
            vec![Value::Text(project_id.to_string())],
        )
        .await?;
        Ok(row.and_then(|row| row.i64("count")).unwrap_or(0).max(0) as u64)
    }

    /// Soft-deletes one fact: stamps `deleted_at`, keeps the row. Idempotent.
    pub async fn soft_delete(&self, id: &str) -> Result<bool, FactsError> {
        let db = self.ready().await?;
        let now = now_millis();
        let result = db
            .execute(
                sql::SOFT_DELETE_FACT,
                vec![
                    Value::Integer(now),
                    Value::Integer(now),
                    Value::Text(id.to_string()),
                ],
            )
            .await?;
        Ok(result.changes > 0)
    }
}

/// Input validation: fact tier, non-empty content, at least one source, and
/// every source in the fact's project.
fn validate(input: &FactInput) -> Result<(), FactsError> {
    if !input.tier.is_fact_tier() {
        return Err(FactsError::validation(
            "tier",
            "must be one of: semantic, procedural, resource",
        ));
    }
    if input.content.trim().is_empty() {
        return Err(FactsError::validation("content", "must not be empty"));
    }
    if input.sources.is_empty() {
        return Err(FactsError::validation(
            "sources",
            "a fact requires at least one contributing session",
        ));
    }
    if input
        .sources
        .iter()
        .any(|source| source.project_id != input.project_id)
    {
        return Err(FactsError::validation(
            "sources",
            "every source must belong to the fact's project",
        ));
    }
    Ok(())
}

/// Maps a row; a corrupt tier value is a fail-closed internal error.
fn row_to_fact(row: &Row) -> Result<Fact, FactsError> {
    let tier_text = row.text("tier").unwrap_or_default();
    let tier = MemoryTier::parse(&tier_text).ok_or_else(|| FactsError::Internal {
        message: "stored facts.tier is not a known tier".to_string(),
    })?;
    Ok(Fact {
        id: row.text("id").unwrap_or_default(),
        project_id: row.text("project_id").unwrap_or_default(),
        tier,
        content: row.text("content").unwrap_or_default(),
        key_normalized: row.text("key_normalized").unwrap_or_default(),
        model_id: row.text("model_id"),
        stale: row.i64("stale").unwrap_or(0),
        generated_at: row.i64("generated_at"),
        source_ref: row.text("source_ref"),
        confidence: row.f64("confidence").unwrap_or(1.0),
        version: row.i64("version").unwrap_or(1),
        metadata: row.text("metadata"),
        created_at: row.i64("created_at").unwrap_or(0),
        updated_at: row.i64("updated_at").unwrap_or(0),
        deleted_at: row.i64("deleted_at"),
    })
}
