//! Database encryption key management — port of upstream
//! `src/services/turso/encryption-key.ts` (blueprint W1.5 step 3, lane sE).
//!
//! DIVERGENCE (recorded): upstream reads the process-global `CONFIG` here
//! (`encryptionMarkerPath()` pulls `CONFIG.storagePath`; `ensure*` expands
//! `~` internally). This port threads the storage root explicitly, so
//! [`ensure_database_encryption_key_file`] takes `storage_path` for its
//! marker guard and `expand_path` is applied by the CALLER — the exported
//! `generate`/`ensure` take an already-resolved [`Path`].
//!
//! Key material is 32 random bytes rendered as lowercase hex (AES-256); the
//! file is written `0600` (`create_new`) inside a `0700` parent directory,
//! and the hexkey is NEVER logged — only paths are.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config::StoreConfig;
use crate::error::StoreError;

/// The encryption marker file name (`.tursodb-encrypted-v1`).
pub const ENCRYPTION_MARKER: &str = ".tursodb-encrypted-v1";

/// Upstream `isValidDatabaseEncryptionHexKey`: trimmed, all hex digits, and
/// exactly 32 (AES-128) or 64 (AES-256) characters.
pub fn is_valid_database_encryption_hex_key(value: &str) -> bool {
    let trimmed = value.trim();
    (trimmed.len() == 32 || trimmed.len() == 64) && trimmed.chars().all(|c| c.is_ascii_hexdigit())
}

/// Upstream `homedir()`: `HOME` (unix) → `USERPROFILE` → `HOMEDRIVE` +
/// `HOMEPATH`. `None` when nothing is set.
pub fn home_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE")
        && !profile.is_empty()
    {
        return Some(PathBuf::from(profile));
    }
    match (
        std::env::var("HOMEDRIVE").ok().filter(|v| !v.is_empty()),
        std::env::var("HOMEPATH").ok().filter(|v| !v.is_empty()),
    ) {
        (Some(drive), Some(path)) => Some(PathBuf::from(format!("{drive}{path}"))),
        _ => None,
    }
}

/// Upstream `DEFAULT_DATABASE_ENCRYPTION_KEY_PATH` (a function here: the home
/// directory is resolved at runtime, not const-evaluable).
pub fn default_database_encryption_key_path() -> PathBuf {
    home_dir()
        .map(|home| home.join(".config/opencode/opencode-mem-db.key"))
        .unwrap_or_else(|| PathBuf::from(".config/opencode/opencode-mem-db.key"))
}

/// Upstream `encryptionMarkerPath` rooted at the caller's storage path.
pub fn encryption_marker_path(storage_path: &Path) -> PathBuf {
    storage_path.join(ENCRYPTION_MARKER)
}

/// Upstream `expandPath`: `~/…` joins the home directory, `~` alone is the
/// home directory, everything else passes through unchanged.
fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    if path == "~"
        && let Some(home) = home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

/// 32 random bytes from `/dev/urandom`, lowercase hex. A read failure is an
/// error — this NEVER falls back to a weak PRNG (fail-closed).
fn random_hex_key() -> Result<String, StoreError> {
    let mut bytes = [0u8; 32];
    let mut source = std::fs::File::open("/dev/urandom").map_err(|error| {
        StoreError::internal(format!(
            "Failed to read random bytes for the database encryption key: {error}"
        ))
    })?;
    source.read_exact(&mut bytes).map_err(|error| {
        StoreError::internal(format!(
            "Failed to read random bytes for the database encryption key: {error}"
        ))
    })?;
    let mut hexkey = String::with_capacity(64);
    for byte in bytes {
        hexkey.push_str(&format!("{byte:02x}"));
    }
    Ok(hexkey)
}

/// Upstream `generateDatabaseEncryptionKeyFile`: fail-closed when encrypted
/// shards already exist, otherwise create a `0600` key file (`wx`).
pub fn generate_database_encryption_key_file(
    key_path: &Path,
    storage_path: &Path,
) -> Result<String, StoreError> {
    if encryption_marker_path(storage_path).exists() {
        return Err(StoreError::internal(format!(
            "Cannot generate a new database encryption key at {}: encrypted databases \
             already exist under {}. Restore the original key file or decrypt/migrate \
             with the old key first.",
            key_path.display(),
            storage_path.display(),
        )));
    }

    if let Some(parent) = key_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let hexkey = random_hex_key()?;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(key_path)?;
    file.write_all(format!("{hexkey}\n").as_bytes())?;
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort: the mode was already set on write where supported.
        let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
    }

    tracing::info!(
        "Generated database encryption key (path: {})",
        key_path.display()
    );
    Ok(hexkey)
}

/// Upstream `ensureDatabaseEncryptionKeyFile`: generate once when missing,
/// else read + validate the existing file (`0600` best-effort chmod).
pub fn ensure_database_encryption_key_file(
    key_path: &Path,
    storage_path: &Path,
) -> Result<String, StoreError> {
    if !key_path.exists() {
        return generate_database_encryption_key_file(key_path, storage_path);
    }

    let content = std::fs::read_to_string(key_path)?.trim().to_string();
    if !is_valid_database_encryption_hex_key(&content) {
        return Err(StoreError::internal(format!(
            "Database encryption key file {} is invalid. Expected a 32- or 64-character \
             hex string (AES-128 or AES-256).",
            key_path.display(),
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort chmod on exotic filesystems.
        let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(content)
}

/// Upstream `resolveOrCreateDatabaseEncryptionKey`, verbatim: explicit hex →
/// `env://VAR` → `file://path` → bare path → auto-created default file.
pub fn resolve_or_create_database_encryption_key(
    config: &StoreConfig,
) -> Result<Option<String>, StoreError> {
    let enabled = config.database_encryption_enabled;
    let raw = config
        .database_encryption_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if !enabled && raw.is_none() {
        return Ok(None);
    }

    if let Some(raw) = raw {
        // Explicit hex in config (discouraged, but supported).
        if is_valid_database_encryption_hex_key(raw) && !raw.contains("://") {
            return Ok(Some(raw.to_string()));
        }

        if let Some(variable) = raw.strip_prefix("env://") {
            let value = std::env::var(variable)
                .ok()
                .map(|value| value.trim().to_string());
            return match value {
                Some(value) if is_valid_database_encryption_hex_key(&value) => Ok(Some(value)),
                _ => Err(StoreError::internal(format!(
                    "databaseEncryptionKey {raw} must resolve to a 32- or 64-character hex string"
                ))),
            };
        }

        if let Some(path) = raw.strip_prefix("file://") {
            return ensure_database_encryption_key_file(&expand_path(path), &config.storage_path)
                .map(Some);
        }

        // Treat bare paths as file paths for convenience.
        if raw.contains('/') || raw.starts_with('~') {
            return ensure_database_encryption_key_file(&expand_path(raw), &config.storage_path)
                .map(Some);
        }

        return Err(StoreError::internal(
            "databaseEncryptionKey must be env://VAR, file://path, or a 32/64-char hex string",
        ));
    }

    // Enabled without an explicit key → the default file, auto-created once.
    ensure_database_encryption_key_file(
        &default_database_encryption_key_path(),
        &config.storage_path,
    )
    .map(Some)
}

/// Non-provisioning variant of [`resolve_or_create_database_encryption_key`]:
/// returns a key only if it is already resolvable WITHOUT minting a key file
/// or reading one into existence. Used by read-only probes (e.g.
/// `is_already_encrypted`) so scanning candidate shards never has the side
/// effect of creating a key file. Returns `None` when no key is available.
pub fn resolve_database_encryption_key_if_present(config: &StoreConfig) -> Option<String> {
    let enabled = config.database_encryption_enabled;
    let raw = config
        .database_encryption_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if !enabled && raw.is_none() {
        return None;
    }

    if let Some(raw) = raw {
        // Explicit hex in config.
        if is_valid_database_encryption_hex_key(raw) && !raw.contains("://") {
            return Some(raw.to_string());
        }

        if let Some(variable) = raw.strip_prefix("env://") {
            let value = std::env::var(variable)
                .ok()
                .map(|value| value.trim().to_string());
            return match value {
                Some(value) if is_valid_database_encryption_hex_key(&value) => Some(value),
                _ => None,
            };
        }

        if let Some(path) = raw.strip_prefix("file://") {
            let path = expand_path(path);
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                let content = content.trim().to_string();
                if is_valid_database_encryption_hex_key(&content) {
                    return Some(content);
                }
            }
            return None;
        }

        // Bare path.
        if raw.contains('/') || raw.starts_with('~') {
            let path = expand_path(raw);
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                let content = content.trim().to_string();
                if is_valid_database_encryption_hex_key(&content) {
                    return Some(content);
                }
            }
            return None;
        }

        return None;
    }

    // Enabled without an explicit key: only reuse an existing default key file.
    let default = default_database_encryption_key_path();
    if default.exists()
        && let Ok(content) = std::fs::read_to_string(&default)
    {
        let content = content.trim().to_string();
        if is_valid_database_encryption_hex_key(&content) {
            return Some(content);
        }
    }
    None
}
