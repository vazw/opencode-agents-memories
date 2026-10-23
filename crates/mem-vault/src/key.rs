//! Key custody path math — the crate's private disk-decision surface
//! (ADR-0012 D-1/D-2).
//!
//! ```text
//! keys_dir    = storage_path / "vault-keys"
//! key_path    = keys_dir / "{scope}.key"
//! scope_dir   = storage_path / "vault" / "{scope}"
//! db_path     = scope_dir / "vault.db"
//! marker_path = scope_dir / ".vault-encrypted-v1"
//! ```
//!
//! Every builder validates the scope, joins, and re-checks containment under
//! the storage root. The containment call is **cheap defence-in-depth, not a
//! second independent guarantee**: after `is_valid_scope_hash` a 16-lowercase-
//! hex string cannot contain `/`, `.`, `\` or NUL, so traversal is already
//! unrepresentable. This module does not claim two guarantees.
//!
//! # Why the key file is minted atomically (D-4)
//!
//! `create_new(true)` followed by a write is not atomic enough: a kill between
//! create and write leaves a zero-length `.key`, read validation rejects it,
//! and the never-regenerate rule forbids repairing it — a self-inflicted
//! permanent lockout. The mint therefore writes a temporary file, `sync_all`s
//! it, and `hard_link`s it to the final name; the temporary file is unlinked on every path where
//! the process is alive to return. A `SIGKILL` or a power loss between
//! `create_new` and `hard_link` can leave a `.tmp-{pid}-{counter}` behind, and the
//! unlink itself is best-effort (a failure is `tracing::warn!`ed, never
//! returned as the caller's error). What keeps the lockout off the primary
//! path is the link, not the unlink: the hard link publishes the already
//! synced bytes under the final name in one step, so on the `HardLink` branch
//! the final key file is never short. The `Fallback` branch's guarantee is
//! weaker by construction (create + write + sync, with a best-effort unlink
//! on error) and is reached only where `hard_link` is unsupported
//! (`Unsupported`/`PermissionDenied`/`CrossesDevices`); the documented fallback is
//! `create_new` + write + `sync_all`, and the branch the only mint site reports
//! is pinned by a test (`MintPath`, `test_vault_mint_pins_the_shipped_link_path`).
//!
//! # Coexistence with `mem-store`'s key helper
//!
//! `mem_store::encryption_key::ensure_database_encryption_key_file` is a
//! *sibling with a different invariant*, never copied: its marker guard is
//! rooted at the storage root and would refuse a second project, and its
//! validation accepts 32-hex AES-128 keys and uppercase hex. The vault's rule
//! is deliberately stricter (exactly 64 lowercase hex).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static MINT_COUNTER: AtomicU64 = AtomicU64::new(0);

use mem_store::db::assert_path_inside_storage;

use crate::VaultError;
use crate::schema::{VAULT_DIR, VAULT_KEYS_DIR};

/// Validates a scope hash: exactly 16 lowercase hex characters (ADR-0012
/// D-2; `mem_core::scope::is_valid_scope_hash` is the declared `mem-core`
/// edge this crate consumes).
pub(crate) fn validate_scope(scope: &str) -> Result<(), VaultError> {
    if mem_core::scope::is_valid_scope_hash(scope) {
        Ok(())
    } else {
        Err(VaultError::InvalidScope)
    }
}

/// The key directory under the storage root (outside the data tree).
pub(crate) fn keys_dir(storage_path: &Path) -> PathBuf {
    storage_path.join(VAULT_KEYS_DIR)
}

/// Validates, joins and contains one derived path.
fn contained(storage_path: &Path, path: PathBuf) -> Result<PathBuf, VaultError> {
    assert_path_inside_storage(storage_path, &path)
        .map_err(|_| VaultError::PathEscape { path: path.clone() })?;
    Ok(path)
}

/// The per-scope key file: `{storage_root}/vault-keys/{scope}.key`.
pub(crate) fn key_path_for(storage_path: &Path, scope: &str) -> Result<PathBuf, VaultError> {
    validate_scope(scope)?;
    contained(
        storage_path,
        keys_dir(storage_path).join(format!("{scope}.key")),
    )
}

/// The per-scope data directory: `{storage_root}/vault/{scope}`.
pub(crate) fn vault_scope_dir(storage_path: &Path, scope: &str) -> Result<PathBuf, VaultError> {
    validate_scope(scope)?;
    contained(storage_path, storage_path.join(VAULT_DIR).join(scope))
}

/// The per-scope database: `{storage_root}/vault/{scope}/vault.db`.
pub(crate) fn db_path_for(storage_path: &Path, scope: &str) -> Result<PathBuf, VaultError> {
    let scope_dir = vault_scope_dir(storage_path, scope)?;
    contained(storage_path, scope_dir.join(crate::schema::VAULT_DB_NAME))
}

/// The per-scope encryption marker:
/// `{storage_root}/vault/{scope}/.vault-encrypted-v1`.
pub(crate) fn marker_path_for(storage_path: &Path, scope: &str) -> Result<PathBuf, VaultError> {
    let scope_dir = vault_scope_dir(storage_path, scope)?;
    contained(
        storage_path,
        scope_dir.join(crate::schema::VAULT_MARKER_NAME),
    )
}

// ---------------------------------------------------------------------------
// Key material (ADR-0012 D-4)
// ---------------------------------------------------------------------------

/// What [`ensure_key_material`] found or minted, and which branch reported it.
pub(crate) struct KeyMaterial {
    pub(crate) hex: KeyHex,
    pub(crate) path: MintPath,
}

/// 64 lowercase hex, with at most one trailing `\n` already stripped.
///
/// The field is private and [`KeyHex::parse`] is the only constructor, so an
/// empty or malformed key string **cannot be constructed** — and therefore
/// cannot reach `mem-store`'s key resolver, whose enabled-without-a-key branch
/// would otherwise read or create `$HOME/.config/opencode/opencode-mem-db.key`.
/// Derives `Clone` only, never `Copy`: key material must not be trivially
/// copied (secret-copy hygiene). Clones are explicit.
#[derive(Clone)]
pub(crate) struct KeyHex(String);

impl KeyHex {
    /// Strips at most one trailing newline, then requires exactly 64
    /// lowercase hex characters. The only constructor for the type.
    pub(crate) fn parse(raw: &str, path: &Path) -> Result<Self, VaultError> {
        let stripped = raw.strip_suffix('\n').unwrap_or(raw);
        let is_valid = stripped.len() == 64
            && stripped
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if is_valid {
            Ok(Self(stripped.to_string()))
        } else {
            Err(VaultError::InvalidKey {
                path: path.to_path_buf(),
            })
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for KeyHex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KeyHex(<redacted>)")
    }
}

impl PartialEq for KeyHex {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for KeyHex {}

/// Which branch the only mint site reported (ADR-0012 D-4 pins the shipped one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MintPath {
    /// The file existed and was read + validated. Not a mint.
    Reused,
    /// The `create_new` temp + `hard_link` route (the primary path).
    HardLink,
    /// The documented `Unsupported`/`PermissionDenied`/`CrossesDevices`
    /// fallback.
    Fallback,
}

/// The crate's entire disk-decision surface for key material.
///
/// The four branches (ADR-0012 D-4/D-10), described at the end of the wave:
///
/// 1. the file exists → read → validate (`KeyHex`) → `Reused`; an invalid file
///    is `InvalidKey` (path only) and the database is not opened;
/// 2. the file is missing while a marker or a database exists →
///    `KeyMissingForExistingData`; no mint, no repair, no delete;
/// 3. the file is missing and nothing else is present → mint atomically;
/// 4. the caller's `database_encryption_key` is never the source.
pub(crate) fn ensure_key_material(
    key_path: &Path,
    db_path: &Path,
    marker_path: &Path,
) -> Result<KeyMaterial, VaultError> {
    // Branch 1 — an existing key is read and validated, never regenerated.
    if key_path.exists() {
        let hex = read_key_file(key_path)?;
        return Ok(KeyMaterial {
            hex,
            path: MintPath::Reused,
        });
    }

    // Branch 2 — a lost key over live ciphertext (or its marker) is
    // unrecoverable by design: never mint, never repair, never delete.
    if marker_path.exists() || db_path.exists() {
        return Err(VaultError::KeyMissingForExistingData {
            path: key_path.to_path_buf(),
        });
    }

    // Branch 3 — nothing exists: mint atomically (delegated to mint_key_file).
    let (hex, path) = mint_key_file(key_path)?;
    Ok(KeyMaterial { hex, path })
}

/// Extracted mint site (W8a Step 6): atomically mints key material for a
/// scope that has no key file and no existing data. Reused by branch 3 of
/// [`ensure_key_material`]. The temp file name carries the pid AND a process-
/// wide counter so concurrent mints in one process cannot collide on the
/// same `{scope}.key.tmp-{pid}` name.
pub(crate) fn mint_key_file(key_path: &Path) -> Result<(KeyHex, MintPath), VaultError> {
    use std::io::ErrorKind;

    let keys_dir = key_path.parent().ok_or_else(|| VaultError::Io {
        path: key_path.to_path_buf(),
        source: std::io::Error::other("the key path has no parent directory"),
    })?;
    ensure_dir_0700(keys_dir)?;

    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| VaultError::Random)?;
    let rendered = format!("{}\n", mem_core::sha256::hex(&random));

    let file_name = key_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| VaultError::Io {
            path: key_path.to_path_buf(),
            source: std::io::Error::other("the key path has no file name"),
        })?;
    let counter = MINT_COUNTER.fetch_add(1, Ordering::SeqCst);
    let temp_path = key_path.with_file_name(format!(
        "{file_name}.tmp-{}-{}",
        std::process::id(),
        counter
    ));
    write_new_0600(&temp_path, &rendered)?;

    match std::fs::hard_link(&temp_path, key_path) {
        Ok(()) => {
            remove_created_file(&temp_path);
            Ok((KeyHex::parse(&rendered, key_path)?, MintPath::HardLink))
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            remove_created_file(&temp_path);
            Ok((read_key_file(key_path)?, MintPath::Reused))
        }
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::Unsupported | ErrorKind::PermissionDenied | ErrorKind::CrossesDevices
            ) =>
        {
            remove_created_file(&temp_path);
            match write_new_0600(key_path, &rendered) {
                Ok(()) => Ok((KeyHex::parse(&rendered, key_path)?, MintPath::Fallback)),
                Err(VaultError::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {
                    Ok((read_key_file(key_path)?, MintPath::Reused))
                }
                Err(other) => Err(other),
            }
        }
        Err(source) => {
            remove_created_file(&temp_path);
            Err(VaultError::Io {
                path: key_path.to_path_buf(),
                source,
            })
        }
    }
}

/// Reads and validates an existing key file (branches 1 and 3's race paths).
fn read_key_file(key_path: &Path) -> Result<KeyHex, VaultError> {
    let raw = std::fs::read_to_string(key_path).map_err(|source| VaultError::Io {
        path: key_path.to_path_buf(),
        source,
    })?;
    KeyHex::parse(&raw, key_path)
}

/// Creates a directory `0700` (the mode is best-effort where the platform
/// refuses, the same caveat ADR-0004 (d) records).
pub(crate) fn ensure_dir_0700(dir: &Path) -> Result<(), VaultError> {
    std::fs::create_dir_all(dir).map_err(|source| VaultError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Creates a new file with mode `0600` and `sync_all`s it. `AlreadyExists` is
/// reported through [`VaultError::Io`] so callers can distinguish the race.
///
/// A write or sync failure after the file was created unlinks that file before
/// returning — best-effort: a failed unlink is `tracing::warn!`ed and a short or
/// unsynced file can then survive the error, the D-4 lockout the unlink exists
/// to prevent. A failed `create_new` created nothing and leaves the existing
/// file untouched.
fn write_new_0600(path: &Path, contents: &str) -> Result<(), VaultError> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|source| VaultError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Err(source) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        // This call created the file; a short or unsynced file must not
        // survive the error, or a failed final-name write would strand a
        // key file that read validation rejects forever (ADR-0012 D-4).
        // The `AlreadyExists` case returns from `open` above, so this
        // never unlinks a file another caller created. The unlink is
        // best-effort: a failure is `tracing::warn!`ed, never returned as
        // the caller's error.
        remove_created_file(path);
        return Err(VaultError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

/// Unlinks a file this call created, reporting — never returning — a failure.
///
/// A failed unlink (a read-only directory, `EROFS`, `EACCES`) is not the
/// caller's error: the caller must still see the write or sync failure that
/// caused the cleanup. But it can strand a short key file — exactly the
/// permanent lockout ADR-0012 D-4's unlink exists to prevent — or a temporary
/// file holding key material, so it is never silent.
fn remove_created_file(path: &Path) {
    if let Err(source) = std::fs::remove_file(path) {
        tracing::warn!(
            path = %path.display(),
            error = %source,
            "cleanup unlink failed; the file remains on disk (ADR-0012 D-4)"
        );
    }
}

/// Creates the encryption marker when it is absent, and on every later open
/// reads it and compares the recorded cipher with the configured one
/// (ADR-0012 D-5). Always **before** the database is opened: this is what
/// turns an opaque codec error on an intact file into a named operator error.
///
/// The marker is the future commit point of a rekey (D-8); a v1 marker means
/// generation 1, and the rekey wave extends the format with a generation field.
pub(crate) fn ensure_marker(
    marker_path: &Path,
    scope_dir: &Path,
    configured_cipher: &str,
) -> Result<(), VaultError> {
    use std::io::ErrorKind;

    ensure_dir_0700(scope_dir)?;
    if !marker_path.exists() {
        match write_new_0600(marker_path, &format!("cipher={configured_cipher}\n")) {
            Ok(()) => return Ok(()),
            // A racing writer created it first: fall through to the read path.
            Err(VaultError::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }

    let raw = std::fs::read_to_string(marker_path).map_err(|source| VaultError::Io {
        path: marker_path.to_path_buf(),
        source,
    })?;
    // Exactly `cipher=<name>` plus at most one trailing newline.
    let stripped = raw.strip_suffix('\n').unwrap_or(&raw);
    let recorded = stripped
        .strip_prefix("cipher=")
        .filter(|name| !name.contains('\n'))
        .ok_or_else(|| VaultError::MarkerInvalid {
            path: marker_path.to_path_buf(),
        })?;
    if recorded != configured_cipher {
        return Err(VaultError::CipherMismatch {
            recorded: recorded.to_string(),
            configured: configured_cipher.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::schema::{VAULT_DB_NAME, VAULT_DIR, VAULT_KEYS_DIR, VAULT_MARKER_NAME};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    const SCOPE: &str = "0123456789abcdef";

    /// Measured on this platform at the first run; a mismatch is a finding,
    /// not a licence to edit.
    const SHIPPED_MINT_PATH: MintPath = MintPath::HardLink;

    fn scratch(label: &str) -> PathBuf {
        let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "mem-vault-{label}-{}-{counter}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    /// D-4's "the test must pin which path shipped" (`0012:139`).
    ///
    /// # What this pin can and cannot catch
    ///
    /// It catches an **honest branch change**: a rewrite that reports
    /// `Fallback` (or `Reused` where a mint was expected), or that drops the
    /// `MintPath` return, fails here. It does **not** catch a rewrite that
    /// keeps the `HardLink` label at the single mint site while changing the
    /// syscalls underneath — no return value can witness a syscall. The
    /// label's truth is a diff-review obligation, the same limit W7.1's
    /// `PragmaOutcome` has. The observable contract below narrows that gap from
    /// the outside; it does not close it.
    #[test]
    fn test_vault_mint_pins_the_shipped_link_path() {
        let dir = scratch("mint-path");
        let key_path = dir.join(VAULT_KEYS_DIR).join(format!("{SCOPE}.key"));
        let db_path = dir.join(VAULT_DIR).join(SCOPE).join(VAULT_DB_NAME);
        let marker_path = dir.join(VAULT_DIR).join(SCOPE).join(VAULT_MARKER_NAME);

        let first = ensure_key_material(&key_path, &db_path, &marker_path).expect("the mint");
        assert_eq!(
            first.path, SHIPPED_MINT_PATH,
            "the mint path this platform ships is pinned by ADR-0012 D-4; \
             changing it is a design change, not a test to relax"
        );

        // The observable contract, asserted independently of the branch label.
        let keys_dir = key_path.parent().expect("keys dir");
        let entries: Vec<String> = std::fs::read_dir(keys_dir)
            .expect("read the key directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec![format!("{SCOPE}.key")],
            "exactly one key file, no temp residue"
        );
        assert!(
            !entries.iter().any(|name| name.contains(".tmp-")),
            "the temporary file is always unlinked"
        );

        let bytes = std::fs::read(&key_path).expect("read the key file");
        assert_eq!(
            bytes.len(),
            65,
            "64 hex characters plus the trailing newline"
        );
        assert_eq!(bytes[64], b'\n');
        assert!(
            bytes[..64]
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "lowercase hex only"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(&key_path)
                .expect("stat the key file")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600, "the key file is owner-only");
            let dir_mode = std::fs::metadata(keys_dir)
                .expect("stat the key directory")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "the key directory is owner-only");
        }

        // A second call reuses the identical bytes: `Reused` is reachable, so
        // the total return type is not decoration.
        let second = ensure_key_material(&key_path, &db_path, &marker_path).expect("the reuse");
        assert_eq!(second.path, MintPath::Reused);
        assert!(
            second.hex == first.hex,
            "the reused key must be byte-identical"
        );
    }

    #[test]
    fn test_key_hex_is_clone_not_copy() {
        let path = std::path::Path::new("dummy");
        let hex = KeyHex::parse(&"0".repeat(64), path).expect("valid");
        let cloned = hex.clone();
        assert_eq!(hex, cloned);
        // KeyHex derives Clone only, never Copy: see the `#[derive(Clone)]`
        // attribute (no `Copy`). Copy absence is a compile-time property;
        // it cannot be asserted at runtime without a compile-fail doctest,
        // which cannot name this pub(crate) type (see item K).
    }

    #[test]
    fn test_vault_mint_temp_name_includes_pid_and_counter() {
        use std::sync::atomic::Ordering;
        let dir = scratch("mint-temp-name");
        let scope_a = "0123456789abcdef";
        let scope_b = "fedcba9876543210";
        let key_path_a = dir.join(VAULT_KEYS_DIR).join(format!("{scope_a}.key"));
        let key_path_b = dir.join(VAULT_KEYS_DIR).join(format!("{scope_b}.key"));
        let before = MINT_COUNTER.load(Ordering::SeqCst);
        let (hex_a, _path_a) = mint_key_file(&key_path_a).expect("mint a");
        let (hex_b, _path_b) = mint_key_file(&key_path_b).expect("mint b");
        let after = MINT_COUNTER.load(Ordering::SeqCst);
        assert!(after > before, "MINT_COUNTER advanced");
        let keys_dir = dir.join(VAULT_KEYS_DIR);
        for entry in std::fs::read_dir(&keys_dir).expect("read keys dir") {
            let name = entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .to_string();
            assert!(!name.contains(".tmp-"), "no temp residue: {name}");
        }
        assert_eq!(hex_a.as_str().len(), 64);
        assert_eq!(hex_b.as_str().len(), 64);
    }
}
