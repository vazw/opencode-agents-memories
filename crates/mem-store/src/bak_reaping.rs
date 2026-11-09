//! Issue 43 W6c — the `.bak` reaper (ADR-0019 A11, D-19-6).
//!
//! Six backup families write full database copies (including sidecars) and no
//! other code path deletes them. The erasure sweep reaps them under the
//! exclusive root only:
//!
//! 1. `{db}.schema-v{n}-{millis}.bak` (`schema_migrations.rs`)
//! 2. `.legacy.bak` (`legacy_migrator.rs`)
//! 3. `.pre-reembed-{nonce}.bak` (`migration_service.rs`)
//! 4. `.pre-encrypt-{millis}.bak` (`encryption_migrator.rs`)
//! 5. `.pre-path-migrate-{nonce}.bak` (`shard_path_migration.rs`)
//! 6. `{db}.{reason}-{pid}-{millis}.bak` (shard quarantine, `shard_manager.rs`)
//!
//! Safety, in order:
//!
//! * The proof is [`ExclusiveRoot::current`] — this process holds the root's
//!   engine lock. No proof ⇒ [`StoreError::NoExclusiveRoot`], nothing removed.
//! * Every candidate is `symlink_metadata`'d; a symlink is SKIPPED (never
//!   followed out of the root), a directory is recursed, a regular file is
//!   claimed ([`ExclusiveRoot::claim`]) and probed before its first unlink.
//! * A `-tshm` is NEVER touched (ADR-0019 A14 / ADR-0023): the matched backup's
//!   `-wal` and `-shm` sidecars are reaped, `-tshm` is left byte-identical.

use std::path::{Path, PathBuf};

use crate::delete_guard::ExclusiveRoot;
use crate::error::StoreError;

/// What one reap removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BakReapReport {
    /// Regular files removed.
    pub reaped: u64,
    /// Their total size before removal.
    pub bytes: u64,
}

/// The six `.bak` families, matched by file name.
///
/// Inverted world: dropping any arm leaves that family's copies on disk after a
/// sweep, which the canary battery's `*.bak` grep detects.
#[must_use]
pub fn is_backup_name(name: &str) -> bool {
    if !name.ends_with(".bak") {
        return false;
    }
    if name.contains(".schema-v") || name.ends_with(".legacy.bak") {
        return true;
    }
    if name.contains(".pre-reembed-")
        || name.contains(".pre-encrypt-")
        || name.contains(".pre-path-migrate-")
    {
        return true;
    }
    // Quarantine: `{db}.{reason}-{pid}-{millis}.bak` (never dot-prefixed; the
    // two numeric tails are what distinguish it from an unrelated `.bak`).
    if name.starts_with('.') {
        return false;
    }
    let stem = &name[..name.len() - ".bak".len()];
    let Some((prefix, millis)) = stem.rsplit_once('-') else {
        return false;
    };
    if !millis.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some((_, pid)) = prefix.rsplit_once('-') else {
        return false;
    };
    pid.chars().all(|c| c.is_ascii_digit())
}

/// Reaps the six `.bak` families under `storage_root`.
///
/// Requires this process to hold the root's engine lock; the caller (the sweep)
/// acquires it before this call.
pub fn reap_backup_files(storage_root: &Path) -> Result<BakReapReport, StoreError> {
    let root = ExclusiveRoot::current(storage_root).ok_or_else(|| StoreError::NoExclusiveRoot {
        db_path: storage_root.to_path_buf(),
    })?;
    let mut report = BakReapReport::default();
    reap_directory(&root, storage_root, &mut report)?;
    Ok(report)
}

fn reap_directory(
    root: &ExclusiveRoot,
    directory: &Path,
    report: &mut BakReapReport,
) -> Result<(), StoreError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        // A directory that vanished mid-walk is nothing to reap, not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(StoreError::from(error)),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        // `symlink_metadata` never follows: a planted symlink is skipped, so the
        // reap cannot escape the root or delete a target outside it.
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(StoreError::from(error)),
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            reap_directory(root, &path, report)?;
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_backup_name(name) {
            continue;
        }
        reap_one(root, &path, &metadata, report)?;
        // The copy's WAL/SHM sidecars are full-copy debris; `-tshm` is never
        // touched (A14), and a missing sidecar is a no-op.
        for suffix in ["-wal", "-shm"] {
            let sidecar = sidecar_path(&path, suffix);
            let Ok(sidecar_meta) = std::fs::symlink_metadata(&sidecar) else {
                continue;
            };
            if sidecar_meta.file_type().is_symlink() || !sidecar_meta.is_file() {
                continue;
            }
            reap_one(root, &sidecar, &sidecar_meta, report)?;
        }
    }
    Ok(())
}

fn reap_one(
    root: &ExclusiveRoot,
    path: &Path,
    metadata: &std::fs::Metadata,
    report: &mut BakReapReport,
) -> Result<(), StoreError> {
    let claim = root.claim(path)?;
    // The proof (2) half: `Held`/`Unknown` refuse before this unlink.
    claim.assert_exclusive()?;
    std::fs::remove_file(path)?;
    report.reaped += 1;
    report.bytes += metadata.len();
    Ok(())
}

fn sidecar_path(base: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", base.display()))
}

#[cfg(test)]
mod tests {
    use super::is_backup_name;

    #[test]
    fn test_all_six_families_match() {
        for name in [
            "user-prompts.db.schema-v1-1700000000000.bak",
            "user-prompts.db.legacy.bak",
            "user-prompts.db.pre-reembed-42.bak",
            "user-prompts.db.pre-encrypt-1700000000000.bak",
            "project_ab_shard_0.db.pre-path-migrate-7.bak",
            "project_ab_shard_0.db.quarantine-1234-1700000000000.bak",
        ] {
            assert!(is_backup_name(name), "{name} is a backup family");
        }
    }

    #[test]
    fn test_unrelated_bak_and_tshm_do_not_match() {
        for name in [
            "notes.bak",
            "user-prompts.db-wal",
            "user-prompts.db-tshm",
            "user-prompts.db.pre-reembed-42.bak-wal",
        ] {
            assert!(!is_backup_name(name), "{name} is not a reaped family");
        }
    }
}
