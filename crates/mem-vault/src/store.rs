//! `VaultStore` — the owner of one per-scope encrypted `vault.db`.
//!
//! W8a opens the content surface on top of the W7.2 open path: the fail-closed
//! open, per-scope key custody and v1 schema remain, and the crate now also
//! ships the content write/read path — `VaultStore::put` and
//! `VaultStore::get` — with a cached `KeyHex` per scope, content-addressed ids
//! derived from that key, and `VaultEntry`/`VaultRecord`. Rekey,
//! `recover_rekey`, the purge erasure leg and `VaultStore::erase` stay in W8b.
//!
//! The store takes **one** [`StoreConfig`] and every derived path descends
//! from its `storage_path`; there is no second root that could disagree
//! (ADR-0012 D-1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, DbHandle, query_one, turso_connect_factory};
use mem_store::platform::Platform;
use mem_store::schema_migrations::{MigrationOptions, apply_schema_migrations};
use mem_store::tokio::sync::OnceCell;

use crate::VaultError;
use crate::entry::{VaultEntry, VaultRecord};
use crate::key;

/// The migration label (tracing only).
pub(crate) const VAULT_MIGRATION_LABEL: &str = "vault";

/// One scope's lazy state: one manager (hence one key) and one open handle.
///
/// The manager is created **inside** its cell, so the key is minted exactly
/// once per scope even under concurrent opens, and a failure at a later step
/// is retried rather than cached (deviation 6).
#[derive(Default)]
struct ScopeState {
    manager: OnceCell<ConnectionManager>,
    handle: OnceCell<DbHandle>,
    key: std::sync::OnceLock<key::KeyHex>,
}

/// The vault store over one storage root.
///
/// # Why one manager per scope
///
/// A [`ConnectionManager`] owns one [`StoreConfig`], hence one key, and caches
/// handles by path. Sharing one across scopes does not fail loudly: it
/// encrypts a *fresh* file with the wrong scope's key — silent, unrecoverable
/// corruption. The map below is therefore load-bearing, not an optimisation.
pub struct VaultStore {
    config: StoreConfig,
    storage_path: PathBuf,
    factory: Arc<dyn ConnectFactory>,
    scopes: Mutex<HashMap<String, Arc<ScopeState>>>,
}

impl VaultStore {
    /// Production constructor over the real turso factory.
    pub fn new(config: StoreConfig) -> Self {
        Self::with_factory(config, Arc::new(turso_connect_factory()))
    }

    /// Constructor with an injected [`ConnectFactory`] (test seam, mirroring
    /// `mem-facts`).
    pub fn with_factory(config: StoreConfig, factory: Arc<dyn ConnectFactory>) -> Self {
        let storage_path = config.storage_path.clone();
        Self {
            config,
            storage_path,
            factory,
            scopes: Mutex::new(HashMap::new()),
        }
    }

    /// The storage root every vault path descends from.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// `{storage_root}/vault-keys/{scope}.key`.
    pub fn key_path(&self, scope: &str) -> Result<PathBuf, VaultError> {
        key::key_path_for(&self.storage_path, scope)
    }

    /// `{storage_root}/vault/{scope}/vault.db`.
    pub fn db_path(&self, scope: &str) -> Result<PathBuf, VaultError> {
        key::db_path_for(&self.storage_path, scope)
    }

    /// `{storage_root}/vault/{scope}/.vault-encrypted-v1`.
    pub fn marker_path(&self, scope: &str) -> Result<PathBuf, VaultError> {
        key::marker_path_for(&self.storage_path, scope)
    }

    /// Opens (and on a fresh scope creates) the scope's vault, applying the v1
    /// schema. Idempotent per scope.
    ///
    /// # The fail-closed order (ADR-0012 D-3) *is* the contract
    ///
    /// 1. validate the scope (no disk touched);
    /// 2. refuse when encryption is disabled — **before** any directory, key
    ///    read, connection or per-scope state, so a refused open leaves no
    ///    trace on disk and cannot be healed by a second call;
    /// 3. resolve the three paths (each validated and contained);
    /// 4. initialise the scope's key material and manager once;
    /// 5. ensure the marker, then open, then apply the v1 schema.
    pub async fn ready(&self, scope: &str) -> Result<DbHandle, VaultError> {
        key::validate_scope(scope)?;
        if !self.config.database_encryption_enabled {
            return Err(VaultError::EncryptionRequired);
        }
        let key_path = self.key_path(scope)?;
        let db_path = self.db_path(scope)?;
        let marker_path = self.marker_path(scope)?;
        let scope_dir = key::vault_scope_dir(&self.storage_path, scope)?;

        // The guard is released at the end of this block — never held across
        // an await, and never held while minting or opening.
        let state = {
            let mut scopes = self.scopes.lock().expect("vault scope map");
            Arc::clone(
                scopes
                    .entry(scope.to_owned())
                    .or_insert_with(|| Arc::new(ScopeState::default())),
            )
        };

        // Resolve key material first, cache it in the scope state (ignore a
        // set failure = already populated), then build the manager from the
        // cached hex. `ensure_key_material` is idempotent on an existing file
        // (Reused), so re-running it on a retry is safe.
        let material = key::ensure_key_material(&key_path, &db_path, &marker_path)?;
        let _ = state.key.set(material.hex.clone());
        tracing::debug!(mint_path = ?material.path, "vault key material resolved");

        let manager = state
            .manager
            .get_or_try_init(|| async {
                let cached = state
                    .key
                    .get()
                    .cloned()
                    .ok_or_else(|| VaultError::InvalidKeyHex)?;
                Ok::<ConnectionManager, VaultError>(self.build_manager(&cached))
            })
            .await?;

        let handle = state
            .handle
            .get_or_try_init(|| async {
                let cipher = self.config.database_encryption_cipher.clone();
                key::ensure_marker(&marker_path, &scope_dir, &cipher)?;
                let db = manager.get_connection(&db_path).await?;
                let options = MigrationOptions {
                    db_path: Some(db_path.clone()),
                    label: VAULT_MIGRATION_LABEL,
                };
                apply_schema_migrations(db.as_ref(), &crate::schema::vault_migrations(), &options)
                    .await?;
                Ok::<DbHandle, VaultError>(db)
            })
            .await?;
        Ok(handle.clone())
    }

    /// The applied schema version. Note: this **creates** a vault on a fresh
    /// scope — `ready` is the only open path, and a read-only variant could not
    /// work without the key anyway.
    pub async fn schema_version(&self, scope: &str) -> Result<i64, VaultError> {
        let db = self.ready(scope).await?;
        let row = query_one(db.as_ref(), "PRAGMA user_version", Vec::new()).await?;
        Ok(row.and_then(|row| row.i64("user_version")).unwrap_or(0))
    }

    pub async fn put(&self, scope: &str, entry: VaultEntry) -> Result<String, VaultError> {
        // Validate created_by (non-empty after trim).
        if entry.created_by.trim().is_empty() {
            return Err(VaultError::InvalidEntry {
                field: "created_by",
            });
        }
        // Validate source_ref unconditionally.
        let trimmed = entry.source_ref.trim();
        if trimmed.is_empty()
            || entry.source_ref.contains('\u{0}')
            || entry.source_ref.len() > 256
        {
            return Err(VaultError::InvalidEntry {
                field: "source_ref",
            });
        }

        let db = self.ready(scope).await?;

        // Read the cached KeyHex populated by ready().
        let state = {
            let scopes = self.scopes.lock().expect("vault scope map");
            scopes.get(scope).cloned()
        };
        let key = state
            .and_then(|state| state.key.get().cloned())
            .ok_or(VaultError::InvalidKeyHex)?;
        let content_hash = crate::hmac::keyed_content_hash(key.as_str(), &entry.content)?;

        // Normalize empty mime_type to NULL.
        let mime_type = match entry.mime_type {
            Some(value) if value.trim().is_empty() => None,
            other => other,
        };

        let created_at = crate::ids::now_millis();

        db.execute(
            "INSERT OR IGNORE INTO vault_entries (content_hash, content, mime_type, source_ref, created_at, created_by) VALUES (?,?,?,?,?,?)",
            vec![
                mem_store::db::Value::Text(content_hash.clone()),
                mem_store::db::Value::Blob(entry.content),
                mem_store::db::Value::from(mime_type),
                mem_store::db::Value::from(entry.source_ref),
                mem_store::db::Value::Integer(created_at),
                mem_store::db::Value::Text(entry.created_by),
            ],
        )
        .await?;

        Ok(content_hash)
    }

    pub async fn get(
        &self,
        scope: &str,
        content_hash: &str,
    ) -> Result<Option<VaultRecord>, VaultError> {
        let db = self.ready(scope).await?;
        let rows = db
            .query(
                "SELECT content_hash, content, mime_type, source_ref, created_at, created_by FROM vault_entries WHERE content_hash = ?",
                vec![mem_store::db::Value::Text(content_hash.to_string())],
            )
            .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let content = match row.get("content") {
            Some(mem_store::db::Value::Blob(bytes)) => bytes.clone(),
            _ => Vec::new(),
        };
        let record = VaultRecord {
            content_hash: row.text("content_hash").unwrap_or_default(),
            content,
            mime_type: row.text("mime_type"),
            source_ref: row.text("source_ref").unwrap_or_default(),
            created_at: row.i64("created_at").unwrap_or(0),
            created_by: row.text("created_by").unwrap_or_default(),
        };
        Ok(Some(record))
    }

    /// Builds the per-scope manager.
    ///
    /// The caller's `database_encryption_key` is **never forwarded**: the
    /// config clone carries only the validated `KeyHex`, so even `Some("")`
    /// from a caller cannot reach `mem-store`'s default-key branch.
    fn build_manager(&self, hex: &key::KeyHex) -> ConnectionManager {
        let mut config = self.config.clone();
        config.database_encryption_enabled = true;
        config.database_encryption_key = Some(hex.as_str().to_owned());
        ConnectionManager::new(
            self.storage_path.clone(),
            config,
            Platform::current(),
            std::sync::Arc::clone(&self.factory),
        )
    }
}
