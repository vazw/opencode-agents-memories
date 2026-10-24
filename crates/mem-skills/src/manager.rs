//! The `SkillsManager` (upstream `src/services/skills/skills-manager.ts`).
//!
//! The manager owns one `skills.db` and the `skills-content/` tree beside
//! it. Every mutation follows upstream's order exactly:
//!
//! 1. validate the input;
//! 2. write (or delete) the `SKILL.md` content file so the file exists
//!    **iff** the skill is enabled;
//! 3. notify the change listeners exactly once.
//!
//! The change-listener seam is upstream's `onSkillsChanged` — the
//! process-wide listener set that the OpenCode V2 bridge hooks its reload
//! into. It is a module-level registry (upstream keeps it in the module
//! scope too), so the guard returned by [`on_skills_changed`] removes the
//! listener on drop.

use mem_core::sync::lock_or_recover;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, DbHandle, Value, turso_connect_factory};
use mem_store::platform::Platform;

use crate::content::{format_markdown, skill_content_path};
use crate::schema::{
    SKILLS_DB_NAME, Skill, SkillInput, SkillPatch, SkillsError, is_valid_skill_id, skills_table_ddl,
};

/// The upstream "Invalid skill id" message (verbatim, so the REST layer can
/// surface it as an upstream business message).
pub const INVALID_SKILL_ID_MESSAGE: &str =
    "Invalid skill id: use lowercase kebab-case (a-z, 0-9, single hyphens), 1-64 characters";

/// The upstream "Skill name is required" message.
pub const SKILL_NAME_REQUIRED_MESSAGE: &str = "Skill name is required";

/// The upstream "Skill content is required" message.
pub const SKILL_CONTENT_REQUIRED_MESSAGE: &str = "Skill content is required";

/// The content file's name (upstream `SKILL.md`).
const SKILL_FILE_NAME: &str = "SKILL.md";

/// The path segment the `skill_path` fallback uses for an ILLEGAL id.
///
/// A constant, so the fallback can never carry an attacker-controlled
/// segment (a traversal id) into the path.
const INVALID_SKILL_PATH_SEGMENT: &str = "__invalid_skill_id__";

/// One entry of the OpenCode HTTP catalog (upstream `toHttpCatalog`).
///
/// The shape is upstream's `{name, version, files}` triple: `name` is the
/// skill **id** (OpenCode derives the skill name from the catalog entry
/// name), `version` is the `updated_at` stamp and `files` is the single
/// `{id}.md` file the host fetches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
}

/// A change listener (upstream `SkillChangeListener`).
pub type SkillsChangedListener = Arc<dyn Fn() + Send + Sync>;

/// One registered listener plus its identity (the id is what the guard
/// removes, so an out-of-order drop never unregisters a neighbour).
type RegisteredListener = (u64, SkillsChangedListener);

/// The process-wide listener registry (upstream's module-level
/// `skillChangeListeners` set).
fn listeners() -> &'static Mutex<Vec<RegisteredListener>> {
    static LISTENERS: OnceLock<Mutex<Vec<RegisteredListener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// The next listener id (monotonic; never reused).
fn next_listener_id() -> u64 {
    static NEXT_ID: OnceLock<AtomicU64> = OnceLock::new();
    NEXT_ID
        .get_or_init(|| AtomicU64::new(1))
        .fetch_add(1, Ordering::SeqCst)
}

/// Registers a change listener and returns the guard that removes it.
///
/// Upstream returns an unsubscribe closure; the port returns a guard with
/// the same lifetime semantics (dropping it unsubscribes).
pub fn on_skills_changed(listener: SkillsChangedListener) -> SkillsChangedGuard {
    let id = next_listener_id();
    // listener Vec<(u64, Arc<dyn Fn()>)>: single push; nothing persistent depends on it (ADR-0013 D-2 D2-R)
    let mut registry = lock_or_recover(listeners());
    registry.push((id, listener));
    SkillsChangedGuard { id: Some(id) }
}

/// Notifies every registered listener (upstream `notifySkillsChanged`).
///
/// A listener that panics must not stop the others: upstream catches the
/// error and logs it, and the port does the same — the notification is
/// best-effort.
pub fn notify_skills_changed() {
    let snapshot: Vec<SkillsChangedListener> = {
        // listener registry: snapshot clone; listeners run after the guard drops (ADR-0013 D-2 D2-R)
        let registry = lock_or_recover(listeners());
        registry
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect()
    };
    for listener in snapshot {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener()));
        if result.is_err() {
            tracing::warn!("skills change listener failed");
        }
    }
}

/// The unsubscribe guard returned by [`on_skills_changed`].
pub struct SkillsChangedGuard {
    id: Option<u64>,
}

impl SkillsChangedGuard {
    /// Unsubscribes eagerly (idempotent).
    ///
    /// The guard's whole job is to REMOVE the listener: `stop()` detaches
    /// the listener from the registry right away, exactly like dropping
    /// the guard does. (Detaching is idempotent, so the drop that follows
    /// a `stop()` is a no-op.)
    pub fn stop(mut self) {
        self.detach();
    }

    /// Removes the listener from the registry (idempotent).
    ///
    /// The id is TAKEN, so a second call (or the drop that follows a
    /// `stop()`) is a no-op: the listener is removed exactly once, and an
    /// out-of-order drop never unregisters a neighbour.
    fn detach(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        // listener registry: retain by id; the id-keyed guard makes duplicate removal harmless (ADR-0013 D-2 D2-R)
        let mut registry = lock_or_recover(listeners());
        registry.retain(|(registered, _)| *registered != id);
    }
}

impl Drop for SkillsChangedGuard {
    fn drop(&mut self) {
        self.detach();
    }
}

/// The `SkillsManager` — one `skills.db` plus its content tree.
pub struct SkillsManager {
    storage_path: PathBuf,
    db_path: PathBuf,
    connections: ConnectionManager,
    handle: tokio::sync::OnceCell<DbHandle>,
}

impl SkillsManager {
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
        let db_path = storage_path.join(SKILLS_DB_NAME);
        let connections = ConnectionManager::new(
            storage_path.clone(),
            store_config,
            Platform::current(),
            factory,
        );
        Self {
            storage_path,
            db_path,
            connections,
            handle: tokio::sync::OnceCell::new(),
        }
    }

    /// The storage root.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// The `skills.db` path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Upstream `reset()` — forget the cached handle so the next call
    /// re-opens (and re-creates) the database.
    pub fn reset(&mut self) {
        self.handle = tokio::sync::OnceCell::new();
    }

    /// Upstream `CONFIG.storagePath = <new>; reset()`.
    pub fn rebind(&mut self, storage_path: PathBuf, store_config: StoreConfig) {
        self.storage_path = storage_path.clone();
        self.db_path = storage_path.join(SKILLS_DB_NAME);
        self.connections = ConnectionManager::new(
            storage_path,
            store_config,
            Platform::current(),
            Arc::new(turso_connect_factory()),
        );
        self.handle = tokio::sync::OnceCell::new();
    }

    /// The absolute `SKILL.md` path of one skill (upstream `skillPath`).
    ///
    /// The path is always absolute: it is built from the absolute storage
    /// path, so the OpenCode host can hand it to the skill loader as-is.
    ///
    /// An ILLEGAL id has no content path. The fallback is a constant,
    /// non-existent path INSIDE the storage root — never the raw id
    /// joined onto the content directory, which would let a traversal id
    /// (`../../x`) escape the storage root.
    pub fn skill_path(&self, id: &str) -> PathBuf {
        skill_content_path(&self.storage_path, id).unwrap_or_else(|_| {
            self.storage_path
                .join(crate::schema::SKILLS_CONTENT_DIR)
                .join(INVALID_SKILL_PATH_SEGMENT)
                .join(SKILL_FILE_NAME)
        })
    }

    /// Upstream `formatMarkdown` — the OpenCode-compatible `SKILL.md` body.
    pub fn format_markdown(&self, skill: &Skill) -> String {
        format_markdown(skill)
    }

    /// Opens the database and applies the idempotent baseline DDL.
    async fn open_and_init(&self) -> Result<DbHandle, SkillsError> {
        let db = self.connections.get_connection(&self.db_path).await?;
        for statement in skills_table_ddl() {
            db.execute(&statement, Vec::new()).await?;
        }
        Ok(db)
    }

    /// The open database handle (opening it on first use).
    pub async fn ready(&self) -> Result<DbHandle, SkillsError> {
        let handle = self
            .handle
            .get_or_try_init(|| async { self.open_and_init().await })
            .await?;
        Ok(handle.clone())
    }

    /// Upstream `list` — the filtered, ordered skill list.
    ///
    /// `container_tag == None` is upstream's `undefined` (no filter). A
    /// `Some(tag)` filter keeps upstream's `(container_tag IS NULL OR
    /// container_tag = ?)` shape — a global skill stays visible under any
    /// tag filter.
    pub async fn list(
        &self,
        container_tag: Option<&str>,
        enabled_only: bool,
    ) -> Result<Vec<Skill>, SkillsError> {
        let db = self.ready().await?;
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        if enabled_only {
            clauses.push("enabled = 1".to_string());
        }
        if let Some(tag) = container_tag {
            clauses.push("(container_tag IS NULL OR container_tag = ?)".to_string());
            params.push(Value::Text(tag.to_string()));
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT id, name, description, content, autoinvoke, enabled, container_tag, created_at, updated_at FROM skills {where_clause} ORDER BY updated_at DESC, id ASC"
        );
        let rows = db.query(&sql, params).await?;
        Ok(rows.iter().map(row_to_skill).collect())
    }

    /// Upstream `get` — one skill by id.
    pub async fn get(&self, id: &str) -> Result<Option<Skill>, SkillsError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                "SELECT id, name, description, content, autoinvoke, enabled, container_tag, created_at, updated_at FROM skills WHERE id = ?",
                vec![Value::Text(id.to_string())],
            )
            .await?;
        Ok(rows.first().map(row_to_skill))
    }

    /// Upstream `create` — validate, insert, write the content file, notify.
    pub async fn create(&self, input: SkillInput) -> Result<Skill, SkillsError> {
        let id = input.id.trim();
        if !is_valid_skill_id(id) {
            return Err(SkillsError::InvalidId);
        }
        let name = input.name.trim();
        if name.is_empty() {
            return Err(SkillsError::NameRequired);
        }
        if input.content.trim().is_empty() {
            return Err(SkillsError::ContentRequired);
        }
        if self.get(id).await?.is_some() {
            return Err(SkillsError::AlreadyExists(id.to_string()));
        }

        let now = now_millis();
        let description = input.description.unwrap_or_default().trim().to_string();
        let autoinvoke = !matches!(input.autoinvoke, Some(false));
        let enabled = !matches!(input.enabled, Some(false));

        let db = self.ready().await?;
        db.execute(
            "INSERT INTO skills (id, name, description, content, autoinvoke, enabled, container_tag, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                Value::Text(id.to_string()),
                Value::Text(name.to_string()),
                Value::Text(description),
                Value::Text(input.content.clone()),
                Value::Integer(i64::from(autoinvoke)),
                Value::Integer(i64::from(enabled)),
                match &input.container_tag {
                    Some(tag) => Value::Text(tag.clone()),
                    None => Value::Null,
                },
                Value::Integer(now),
                Value::Integer(now),
            ],
        )
        .await?;

        let created = self
            .get(id)
            .await?
            .ok_or(SkillsError::MissingAfterWrite(id.to_string()))?;
        if created.enabled {
            self.write_skill_file(&created).await?;
        }
        notify_skills_changed();
        Ok(created)
    }

    /// Upstream `update` — patch, then sync the content file.
    pub async fn update(&self, id: &str, patch: SkillPatch) -> Result<Skill, SkillsError> {
        let Some(existing) = self.get(id).await? else {
            return Err(SkillsError::NotFound(id.to_string()));
        };

        let name = match &patch.name {
            Some(name) => name.trim().to_string(),
            None => existing.name.clone(),
        };
        if name.is_empty() {
            return Err(SkillsError::NameRequired);
        }
        let content = match &patch.content {
            Some(content) => content.clone(),
            None => existing.content.clone(),
        };
        if content.trim().is_empty() {
            return Err(SkillsError::ContentRequired);
        }
        let description = match &patch.description {
            Some(description) => description.trim().to_string(),
            None => existing.description.clone(),
        };
        let autoinvoke = patch.autoinvoke.unwrap_or(existing.autoinvoke);
        let enabled = patch.enabled.unwrap_or(existing.enabled);
        let container_tag = match &patch.container_tag {
            Some(tag) => Some(tag.clone()),
            None => existing.container_tag.clone(),
        };
        let now = now_millis();

        let db = self.ready().await?;
        db.execute(
            "UPDATE skills SET name = ?, description = ?, content = ?, autoinvoke = ?, enabled = ?, container_tag = ?, updated_at = ? WHERE id = ?",
            vec![
                Value::Text(name),
                Value::Text(description),
                Value::Text(content),
                Value::Integer(i64::from(autoinvoke)),
                Value::Integer(i64::from(enabled)),
                match container_tag {
                    Some(tag) => Value::Text(tag),
                    None => Value::Null,
                },
                Value::Integer(now),
                Value::Text(id.to_string()),
            ],
        )
        .await?;

        let updated = self
            .get(id)
            .await?
            .ok_or(SkillsError::MissingAfterWrite(id.to_string()))?;
        if updated.enabled {
            self.write_skill_file(&updated).await?;
        } else {
            self.remove_skill_file(id).await?;
        }
        notify_skills_changed();
        Ok(updated)
    }

    /// Upstream `delete` — remove the row and its content file, notify.
    pub async fn delete(&self, id: &str) -> Result<bool, SkillsError> {
        if self.get(id).await?.is_none() {
            return Ok(false);
        }
        let db = self.ready().await?;
        db.execute(
            "DELETE FROM skills WHERE id = ?",
            vec![Value::Text(id.to_string())],
        )
        .await?;
        self.remove_skill_file(id).await?;
        notify_skills_changed();
        Ok(true)
    }

    /// Upstream `toHttpCatalog` — the OpenCode HTTP catalog index.
    ///
    /// Only **enabled** skills are listed; `version` is the `updated_at`
    /// stamp and `files` is the single `{id}.md` file.
    pub async fn catalog_index(&self) -> Result<Vec<CatalogEntry>, SkillsError> {
        let enabled = self.list(None, true).await?;
        Ok(enabled
            .into_iter()
            .map(|skill| CatalogEntry {
                name: skill.id.clone(),
                version: skill.updated_at.to_string(),
                files: vec![format!("{}.md", skill.id)],
            })
            .collect())
    }

    /// Upstream `toCatalogMarkdown` — the catalog markdown of one skill.
    ///
    /// `None` for an unknown **or disabled** skill: the catalog only ever
    /// serves enabled skills.
    pub async fn catalog_markdown(&self, id: &str) -> Result<Option<String>, SkillsError> {
        let Some(skill) = self.get(id).await? else {
            return Ok(None);
        };
        if !skill.enabled {
            return Ok(None);
        }
        Ok(Some(format_markdown(&skill)))
    }

    /// Writes the `SKILL.md` content file (upstream `writeSkillFile`).
    async fn write_skill_file(&self, skill: &Skill) -> Result<(), SkillsError> {
        let path = skill_content_path(&self.storage_path, &skill.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| SkillsError::Io(error.to_string()))?;
        }
        std::fs::write(&path, format_markdown(skill))
            .map_err(|error| SkillsError::Io(error.to_string()))
    }

    /// Removes the skill's content directory (upstream `removeSkillFile`).
    async fn remove_skill_file(&self, id: &str) -> Result<(), SkillsError> {
        let path = skill_content_path(&self.storage_path, id)?;
        let Some(directory) = path.parent() else {
            return Ok(());
        };
        match std::fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            // A missing directory is already the wanted state.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SkillsError::Io(error.to_string())),
        }
    }
}

/// The current epoch in milliseconds (the store's timestamps are `i64` ms).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Maps one `skills` row to a [`Skill`] (upstream `rowToSkill`).
fn row_to_skill(row: &mem_store::db::Row) -> Skill {
    Skill {
        id: row.text("id").unwrap_or_default(),
        name: row.text("name").unwrap_or_default(),
        description: row.text("description").unwrap_or_default(),
        content: row.text("content").unwrap_or_default(),
        autoinvoke: row.i64("autoinvoke").unwrap_or(1) != 0,
        enabled: row.i64("enabled").unwrap_or(1) != 0,
        container_tag: row.text("container_tag"),
        created_at: row.i64("created_at").unwrap_or(0),
        updated_at: row.i64("updated_at").unwrap_or(0),
    }
}
