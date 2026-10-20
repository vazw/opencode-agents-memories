//! `UserPromptManager` — port of upstream
//! `src/services/user-prompt/user-prompt-manager.ts` (W3.1 step 2).
//!
//! Storage is upstream's actual `user-prompts.db` (D-V1): a Turso database
//! with the claim state machine `captured ∈ {0, 2, 1}` plus `capture_attempts`
//! and `user_learning_captured`. Every SQL statement mirrors the upstream
//! template literal verbatim (the constants live in `mem_store::sql` where a
//! golden test pins them).
//!
//! DIVERGENCE (reviewer note): upstream reads `CONFIG.storagePath` through a
//! module-level singleton; this port takes an explicit `storage_path` +
//! [`StoreConfig`] per manager (the established W1.x/W2.x pattern), so tests
//! are hermetic and two managers can act as two peer processes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, DbHandle, Value, query_one, turso_connect_factory};
use mem_store::platform::Platform;
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, ensure_user_prompt_columns, user_prompts_migrations,
};

use crate::error::ProfileError;
use crate::ids::{next_prompt_id, now_millis};

// W2.3 reviewer-rework firewall: ONLY these three items may be imported from
// `mem_ai::privacy` (locked hash-3fc66017d7f3 surface) — no internal module
// paths.
use mem_ai::privacy::{REDACTED, is_fully_private, strip_private_content};

/// Upstream `USER_PROMPTS_DB_NAME`.
pub const USER_PROMPTS_DB_NAME: &str = "user-prompts.db";

/// One `user_prompts` row surface (upstream `UserPrompt` interface).
#[derive(Debug, Clone, PartialEq)]
pub struct UserPrompt {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub project_path: Option<String>,
    pub content: String,
    pub created_at: i64,
    pub captured: bool,
    pub user_learning_captured: bool,
    pub linked_memory_id: Option<String>,
    pub capture_attempts: i64,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

/// The prompt store over one `user-prompts.db`.
pub struct UserPromptManager {
    storage_path: PathBuf,
    db_path: PathBuf,
    store_config: StoreConfig,
    platform: Platform,
    connections: ConnectionManager,
    handle: tokio::sync::OnceCell<DbHandle>,
    max_retries: u32,
}

impl UserPromptManager {
    /// Production constructor over the real turso factory.
    pub fn new(storage_path: PathBuf, store_config: StoreConfig, max_retries: u32) -> Self {
        Self::with_factory(
            storage_path,
            store_config,
            Arc::new(turso_connect_factory()),
            max_retries,
        )
    }

    /// Constructor with an injected [`ConnectFactory`] (test seam).
    pub fn with_factory(
        storage_path: PathBuf,
        store_config: StoreConfig,
        factory: Arc<dyn ConnectFactory>,
        max_retries: u32,
    ) -> Self {
        let db_path = storage_path.join(USER_PROMPTS_DB_NAME);
        let platform = Platform::current();
        let connections = ConnectionManager::new(
            storage_path.clone(),
            store_config.clone(),
            platform,
            factory,
        );
        Self {
            storage_path,
            db_path,
            store_config,
            platform,
            connections,
            handle: tokio::sync::OnceCell::new(),
            max_retries,
        }
    }

    /// Upstream `CONFIG.autoCaptureMaxRetries ?? 3`.
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Upstream `reset()` — forget the initialized handle; the next `ready()`
    /// re-runs migrations + the stale-claim reset over the same storage path.
    pub fn reset(&mut self) {
        self.handle = tokio::sync::OnceCell::new();
    }

    /// Upstream `CONFIG.storagePath = <new>; reset()` — rebind this manager to
    /// a different storage root.
    pub fn rebind(&mut self, storage_path: PathBuf, store_config: StoreConfig) {
        self.storage_path = storage_path.clone();
        self.db_path = storage_path.join(USER_PROMPTS_DB_NAME);
        self.store_config = store_config.clone();
        self.connections = ConnectionManager::new(
            storage_path,
            store_config,
            self.platform,
            Arc::new(turso_connect_factory()),
        );
        self.handle = tokio::sync::OnceCell::new();
    }

    /// Upstream `ready()` — lazy init that must succeed before any statement.
    pub async fn ready(&self) -> Result<DbHandle, ProfileError> {
        let handle = self
            .handle
            .get_or_try_init(|| async { self.open_and_init().await })
            .await?;
        Ok(handle.clone())
    }

    /// Upstream `initDatabase()`: schema migrations, column upgrade, then the
    /// open-time stale-claim reset (`captured = 2 → 0`).
    async fn open_and_init(&self) -> Result<DbHandle, ProfileError> {
        let db = self.connections.get_connection(&self.db_path).await?;
        let options = MigrationOptions {
            db_path: Some(self.db_path.clone()),
            label: USER_PROMPTS_DB_NAME,
        };
        apply_schema_migrations(db.as_ref(), &user_prompts_migrations(), &options).await?;
        ensure_user_prompt_columns(db.as_ref()).await?;
        db.execute(
            "UPDATE user_prompts SET captured = 0 WHERE captured = 2",
            Vec::new(),
        )
        .await?;
        Ok(db)
    }

    // -----------------------------------------------------------------------
    // Writes
    // -----------------------------------------------------------------------

    /// Upstream `savePrompt`: INSERT … WHERE NOT EXISTS, else the oldest
    /// existing row id for the same (session_id, message_id) pair.
    ///
    /// D-V6 (issue directive): this write path is where fully-private prompt
    /// rejection lands. It uses ONLY the locked W2.3 privacy surface and keeps
    /// upstream's sentence `Private content blocked`; the persisted content is
    /// the stripped form (private spans never reach storage).
    pub async fn save_prompt(
        &self,
        session_id: &str,
        message_id: &str,
        project_path: &str,
        content: &str,
    ) -> Result<String, ProfileError> {
        let content = self.gate_private_content(content)?;
        self.persist_prompt(session_id, message_id, project_path, &content)
            .await
    }

    /// The D-V6 gate: reject fully-private content, otherwise return the
    /// stripped, persistable form.
    ///
    /// Two rejection classes, exactly as the blueprint pins them:
    /// 1. `is_fully_private(content)` — nothing but a private span / blank;
    /// 2. what remains after removing every `[REDACTED]` marker is blank —
    ///    e.g. two adjacent private spans strip to `[REDACTED][REDACTED]`,
    ///    which is not the single-marker form `is_fully_private` recognizes.
    fn gate_private_content(&self, content: &str) -> Result<String, ProfileError> {
        if is_fully_private(content) {
            return Err(ProfileError::PrivateContentBlocked);
        }
        let stripped = strip_private_content(content);
        let without_markers = stripped.replace(REDACTED, "");
        if without_markers.trim().is_empty() {
            return Err(ProfileError::PrivateContentBlocked);
        }
        Ok(stripped)
    }

    /// The raw persist path (no privacy gate) — upstream `savePrompt`'s SQL.
    async fn persist_prompt(
        &self,
        session_id: &str,
        message_id: &str,
        project_path: &str,
        content: &str,
    ) -> Result<String, ProfileError> {
        let db = self.ready().await?;
        let id = next_prompt_id();
        let now = now_millis();
        let inserted = db
            .execute(
                "
      INSERT INTO user_prompts (id, session_id, message_id, project_path, content, created_at, captured)
      SELECT ?, ?, ?, ?, ?, ?, 0
      WHERE NOT EXISTS (
        SELECT 1 FROM user_prompts WHERE session_id = ? AND message_id = ?
      )
    ",
                vec![
                    Value::Text(id.clone()),
                    Value::Text(session_id.to_string()),
                    Value::Text(message_id.to_string()),
                    Value::Text(project_path.to_string()),
                    Value::Text(content.to_string()),
                    Value::Integer(now),
                    Value::Text(session_id.to_string()),
                    Value::Text(message_id.to_string()),
                ],
            )
            .await?;
        if inserted.changes > 0 {
            return Ok(id);
        }

        let existing = query_one(
            db.as_ref(),
            "
       SELECT id FROM user_prompts
       WHERE session_id = ? AND message_id = ?
       ORDER BY created_at ASC
       LIMIT 1",
            vec![
                Value::Text(session_id.to_string()),
                Value::Text(message_id.to_string()),
            ],
        )
        .await?;
        if let Some(row) = existing
            && let Some(existing_id) = row.text("id")
        {
            return Ok(existing_id);
        }
        Err(ProfileError::Save(
            "Failed to save or locate user prompt".to_string(),
        ))
    }

    /// Upstream `setPromptModel`.
    pub async fn set_prompt_model(
        &self,
        message_id: &str,
        provider_id: &str,
        model_id: &str,
    ) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute(
            "UPDATE user_prompts SET provider_id = ?, model_id = ? WHERE message_id = ?",
            vec![
                Value::Text(provider_id.to_string()),
                Value::Text(model_id.to_string()),
                Value::Text(message_id.to_string()),
            ],
        )
        .await?;
        Ok(())
    }

    /// Upstream `deletePrompt`.
    pub async fn delete_prompt(&self, prompt_id: &str) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute(
            "DELETE FROM user_prompts WHERE id = ?",
            vec![Value::Text(prompt_id.to_string())],
        )
        .await?;
        Ok(())
    }

    /// Upstream `markAsCaptured` (`captured = 1`).
    pub async fn mark_as_captured(&self, prompt_id: &str) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute(
            "UPDATE user_prompts SET captured = 1 WHERE id = ?",
            vec![Value::Text(prompt_id.to_string())],
        )
        .await?;
        Ok(())
    }

    /// Upstream `claimPrompt`: `captured 0 → 2`, true iff one row changed.
    pub async fn claim_prompt(&self, prompt_id: &str) -> Result<bool, ProfileError> {
        let db = self.ready().await?;
        let result = db
            .execute(
                "UPDATE user_prompts SET captured = 2 WHERE id = ? AND captured = 0",
                vec![Value::Text(prompt_id.to_string())],
            )
            .await?;
        Ok(result.changes > 0)
    }

    /// Upstream `recordFailedAttempt` (unbounded increment; the read queries
    /// bound visibility by `capture_attempts < max_retries`).
    pub async fn record_failed_attempt(&self, prompt_id: &str) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute(
            "UPDATE user_prompts SET capture_attempts = capture_attempts + 1 WHERE id = ?",
            vec![Value::Text(prompt_id.to_string())],
        )
        .await?;
        Ok(())
    }

    /// Upstream `releaseClaim`: `captured 2 → 0`, true iff one row changed.
    pub async fn release_claim(&self, prompt_id: &str) -> Result<bool, ProfileError> {
        let db = self.ready().await?;
        let result = db
            .execute(
                "UPDATE user_prompts SET captured = 0 WHERE id = ? AND captured = 2",
                vec![Value::Text(prompt_id.to_string())],
            )
            .await?;
        Ok(result.changes > 0)
    }

    /// Upstream `markMultipleAsUserLearningCaptured` (empty list = no-op).
    pub async fn mark_multiple_as_captured(
        &self,
        prompt_ids: &[String],
    ) -> Result<(), ProfileError> {
        if prompt_ids.is_empty() {
            return Ok(());
        }
        let db = self.ready().await?;
        let placeholders = placeholders(prompt_ids.len());
        let sql = format!("UPDATE user_prompts SET captured = 1 WHERE id IN ({placeholders})");
        db.execute(sql.as_str(), text_values(prompt_ids)).await?;
        Ok(())
    }

    /// Upstream `linkMemoryToPrompt`.
    pub async fn link_memory_to_prompt(
        &self,
        prompt_id: &str,
        memory_id: &str,
    ) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute(
            "UPDATE user_prompts SET linked_memory_id = ? WHERE id = ?",
            vec![
                Value::Text(memory_id.to_string()),
                Value::Text(prompt_id.to_string()),
            ],
        )
        .await?;
        Ok(())
    }

    /// Upstream `vacuum`.
    pub async fn vacuum(&self) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute("VACUUM", Vec::new()).await?;
        Ok(())
    }

    /// Upstream `deleteOldPrompts`.
    pub async fn delete_old_prompts(&self, cutoff_time: i64) -> Result<u64, ProfileError> {
        let db = self.ready().await?;
        let result = db
            .execute(
                "DELETE FROM user_prompts WHERE created_at < ?",
                vec![Value::Integer(cutoff_time)],
            )
            .await?;
        Ok(result.changes)
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    /// Upstream `getLastUncapturedPrompt` (newest `captured = 0` under the
    /// retry bound).
    pub async fn get_last_uncaptured_prompt(
        &self,
        session_id: &str,
    ) -> Result<Option<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            "
      SELECT * FROM user_prompts
      WHERE session_id = ? AND captured = 0 AND capture_attempts < ?
      ORDER BY created_at DESC
      LIMIT 1
    ",
            vec![
                Value::Text(session_id.to_string()),
                Value::Integer(i64::from(self.max_retries)),
            ],
        )
        .await?;
        Ok(row.as_ref().map(row_to_prompt))
    }

    /// Upstream `getUncapturedPromptsForSession` (oldest first).
    pub async fn get_uncaptured_prompts_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                "
      SELECT * FROM user_prompts
      WHERE session_id = ? AND captured = 0 AND capture_attempts < ?
      ORDER BY created_at ASC
    ",
                vec![
                    Value::Text(session_id.to_string()),
                    Value::Integer(i64::from(self.max_retries)),
                ],
            )
            .await?;
        Ok(rows.iter().map(row_to_prompt).collect())
    }

    /// Upstream `countUncapturedPrompts`.
    pub async fn count_uncaptured_prompts(&self) -> Result<i64, ProfileError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            "SELECT COUNT(*) as count FROM user_prompts WHERE captured = 0 AND capture_attempts < ?",
            vec![Value::Integer(i64::from(self.max_retries))],
        )
        .await?;
        Ok(row.and_then(|row| row.i64("count")).unwrap_or(0))
    }

    /// Upstream `getUncapturedPrompts` (retry-bounded, least-retried first).
    pub async fn get_uncaptured_prompts(
        &self,
        limit: i64,
    ) -> Result<Vec<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                "
      SELECT * FROM user_prompts
      WHERE captured = 0 AND capture_attempts < ?
      ORDER BY capture_attempts ASC, created_at ASC
      LIMIT ?
    ",
                vec![
                    Value::Integer(i64::from(self.max_retries)),
                    Value::Integer(limit),
                ],
            )
            .await?;
        Ok(rows.iter().map(row_to_prompt).collect())
    }

    /// Upstream `getPromptById`.
    pub async fn get_prompt_by_id(
        &self,
        prompt_id: &str,
    ) -> Result<Option<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            "SELECT * FROM user_prompts WHERE id = ?",
            vec![Value::Text(prompt_id.to_string())],
        )
        .await?;
        Ok(row.as_ref().map(row_to_prompt))
    }

    /// Upstream `getPromptsByIds` (empty list = empty result, no SQL).
    pub async fn get_prompts_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<UserPrompt>, ProfileError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.ready().await?;
        let placeholders = placeholders(ids.len());
        let sql = format!("SELECT * FROM user_prompts WHERE id IN ({placeholders})");
        let rows = db.query(sql.as_str(), text_values(ids)).await?;
        Ok(rows.iter().map(row_to_prompt).collect())
    }

    /// Upstream `countUnanalyzedForUserLearning`.
    pub async fn count_unanalyzed_for_user_learning(&self) -> Result<i64, ProfileError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            "SELECT COUNT(*) as count FROM user_prompts WHERE user_learning_captured = 0",
            Vec::new(),
        )
        .await?;
        Ok(row.and_then(|row| row.i64("count")).unwrap_or(0))
    }

    /// Upstream `getPromptsForUserLearning(limit)` (oldest first).
    pub async fn get_prompts_for_user_learning(
        &self,
        limit: i64,
    ) -> Result<Vec<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                "SELECT * FROM user_prompts
      WHERE user_learning_captured = 0
      ORDER BY created_at ASC
      LIMIT ?",
                vec![Value::Integer(limit)],
            )
            .await?;
        Ok(rows.iter().map(row_to_prompt).collect())
    }

    /// Upstream `markMultipleAsUserLearningCaptured` (empty list = no-op).
    pub async fn mark_multiple_as_user_learning_captured(
        &self,
        prompt_ids: &[String],
    ) -> Result<(), ProfileError> {
        if prompt_ids.is_empty() {
            return Ok(());
        }
        let db = self.ready().await?;
        let placeholders = placeholders(prompt_ids.len());
        let sql = format!(
            "UPDATE user_prompts SET user_learning_captured = 1 WHERE id IN ({placeholders})"
        );
        db.execute(sql.as_str(), text_values(prompt_ids)).await?;
        Ok(())
    }

    /// Upstream `getCapturedPrompts(projectPath?)`: the SQL side normalizes
    /// stored separators, the argument side normalizes the query path.
    pub async fn get_captured_prompts(
        &self,
        project_path: Option<&str>,
    ) -> Result<Vec<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let rows = match project_path {
            Some(project_path) => {
                db.query(
                    "SELECT * FROM user_prompts
           WHERE captured = 1 AND REPLACE(project_path, '\\', '/') = ?
           ORDER BY created_at DESC",
                    vec![Value::Text(normalize_separators(project_path))],
                )
                .await?
            }
            None => {
                db.query(
                    "SELECT * FROM user_prompts WHERE captured = 1 ORDER BY created_at DESC",
                    Vec::new(),
                )
                .await?
            }
        };
        Ok(rows.iter().map(row_to_prompt).collect())
    }

    /// Upstream `searchPrompts(query, projectPath?, limit = 20)`.
    pub async fn search_prompts(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: i64,
    ) -> Result<Vec<UserPrompt>, ProfileError> {
        let db = self.ready().await?;
        let mut params = vec![Value::Text(format!("%{query}%"))];
        let mut sql =
            "SELECT * FROM user_prompts WHERE content LIKE ? AND captured = 1".to_string();
        if let Some(project_path) = project_path {
            sql.push_str(" AND REPLACE(project_path, '\\', '/') = ?");
            params.push(Value::Text(normalize_separators(project_path)));
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");
        params.push(Value::Integer(limit));
        let rows = db.query(sql.as_str(), params).await?;
        Ok(rows.iter().map(row_to_prompt).collect())
    }

    /// Upstream `updateProjectPath` (no-op on empty/equal paths).
    pub async fn update_project_path(
        &self,
        old_project_path: &str,
        new_project_path: &str,
    ) -> Result<u64, ProfileError> {
        if old_project_path.is_empty()
            || new_project_path.is_empty()
            || old_project_path == new_project_path
        {
            return Ok(0);
        }
        let db = self.ready().await?;
        let result = db
            .execute(
                "UPDATE user_prompts
       SET project_path = ?
       WHERE REPLACE(project_path, '\\', '/') = ?",
                vec![
                    Value::Text(new_project_path.to_string()),
                    Value::Text(normalize_separators(old_project_path)),
                ],
            )
            .await?;
        Ok(result.changes)
    }
}

/// Upstream `projectPath.replace(/\\/g, "/")`.
pub(crate) fn normalize_separators(path: &str) -> String {
    path.replace('\\', "/")
}

/// `?,?,?…` placeholder list.
fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn text_values(values: &[String]) -> Vec<Value> {
    values
        .iter()
        .map(|value| Value::Text(value.clone()))
        .collect()
}

/// Upstream `rowToPrompt` (null-tolerant: a missing column reads as the
/// documented default rather than a panic).
fn row_to_prompt(row: &mem_store::db::Row) -> UserPrompt {
    UserPrompt {
        id: row.text("id").unwrap_or_default(),
        session_id: row.text("session_id").unwrap_or_default(),
        message_id: row.text("message_id").unwrap_or_default(),
        project_path: row.text("project_path"),
        content: row.text("content").unwrap_or_default(),
        created_at: row.i64("created_at").unwrap_or_default(),
        captured: row.i64("captured") == Some(1),
        user_learning_captured: row.i64("user_learning_captured") == Some(1),
        linked_memory_id: row.text("linked_memory_id"),
        capture_attempts: row.i64("capture_attempts").unwrap_or(0),
        provider_id: row.text("provider_id"),
        model_id: row.text("model_id"),
    }
}
