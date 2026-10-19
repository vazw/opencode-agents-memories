//! `StoreConfig` — the mem-store slice of `mem_core` config (blueprint W1.2).
//!
//! Only the fields the storage engine reads are mirrored here; `from_core`
//! copies them out of a resolved [`mem_core::config::Config`] so the store
//! never re-resolves user config. `validate` re-checks the upstream
//! `getValidatedEmbeddingDimensions` bounds at the store border.

use std::path::PathBuf;

/// The storage-engine's configuration view.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub storage_path: PathBuf,
    pub embedding_dimensions: u32,
    pub embedding_model: String,
    pub max_vectors_per_shard: u32,
    pub container_tag_prefix: String,
    pub database_encryption_enabled: bool,
    /// Rendered cipher name (upstream `CONFIG.databaseEncryptionCipher`
    /// string form, e.g. `aes256gcm`).
    pub database_encryption_cipher: String,
    /// The raw configured key; resolution/provisioning lives in
    /// `resolve_database_encryption` (explicit hex, `env://VAR`, `file://path`,
    /// a bare path, or the auto-created default key file when encryption is
    /// enabled without an explicit key).
    pub database_encryption_key: Option<String>,
}

pub use crate::platform::Platform;

/// Mirrors only the store-relevant fields (cipher rendered through
/// `DatabaseEncryptionCipher::as_str()` — the pinned runtime string, e.g.
/// `aes256gcm`).
pub fn from_core(config: &mem_core::config::Config) -> StoreConfig {
    StoreConfig {
        storage_path: config.storage_path.clone(),
        embedding_dimensions: config.embedding_dimensions,
        embedding_model: config.embedding_model.clone(),
        max_vectors_per_shard: config.max_vectors_per_shard,
        container_tag_prefix: config.container_tag_prefix.clone(),
        database_encryption_enabled: config.database_encryption_enabled,
        database_encryption_cipher: config.database_encryption_cipher.as_str().to_string(),
        database_encryption_key: config.database_encryption_key.clone(),
    }
}

impl StoreConfig {
    /// The `metadata.db` location (upstream `METADATA_DB_NAME`).
    pub fn metadata_db_path(&self) -> PathBuf {
        self.storage_path.join("metadata.db")
    }
}

/// Upstream `getValidatedEmbeddingDimensions` bound: 1..=65536.
pub fn validate(config: &StoreConfig) -> Result<(), StoreError> {
    let dimensions = i64::from(config.embedding_dimensions);
    if !(1..=65536).contains(&dimensions) {
        return Err(StoreError::InvalidEmbeddingDimensions { dimensions });
    }
    Ok(())
}

use crate::error::StoreError;
