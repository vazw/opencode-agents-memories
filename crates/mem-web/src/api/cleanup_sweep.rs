//! Sidecar `cleanup-sweep.json` stamp store for the capture-triggered retention
//! cleanup (issue 41).
//!
//! Deliberately parallel to mem_profile::decay_sweep — same shape, separate file,
//! separate error type. Do not merge: the two stamps must never be confusable.
//!
//! The stamp records when the automatic retention sweep last completed. It is
//! purely additive: older builds ignore unknown files in the storage root, and a
//! missing, corrupt or unreadable stamp degrades fail-safe to "due" (a sweep is
//! safer than a silently disabled one).
//!
//! Field name is `last_cleanup_sweep`, never `last_decay_sweep`: a shared field
//! name would let one sidecar be read as the other.

use std::io::{Error, ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Sidecar file name stored in the storage root.
pub const CLEANUP_SWEEP_FILE_NAME: &str = "cleanup-sweep.json";

fn default_version() -> u32 {
    1
}

/// On-disk schema for `cleanup-sweep.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupSweepStamp {
    /// Schema version (defaults to 1 for forward compatibility).
    #[serde(default = "default_version")]
    pub version: u32,
    /// Unix timestamp in milliseconds of the last completed automatic sweep.
    pub last_cleanup_sweep: i64,
}

/// Resolves the sidecar file path inside the storage root.
pub fn cleanup_sweep_path(storage_root: &Path) -> PathBuf {
    storage_root.join(CLEANUP_SWEEP_FILE_NAME)
}

fn invalid_data(path: &Path, message: String) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!("cleanup sweep stamp at {}: {message}", path.display()),
    )
}

fn parse_stamp(raw: &str, path: &Path) -> std::io::Result<i64> {
    let stamp: CleanupSweepStamp =
        serde_json::from_str(raw).map_err(|error| invalid_data(path, error.to_string()))?;
    if stamp.version != 1 {
        return Err(invalid_data(
            path,
            format!("unsupported version {} (expected 1)", stamp.version),
        ));
    }
    Ok(stamp.last_cleanup_sweep)
}

/// Loads the stamp, answering `Ok(None)` when the file does not exist and
/// `Err(InvalidData)` when it is corrupt or carries an unknown version.
pub fn load_stamp(path: &Path) -> std::io::Result<Option<i64>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse_stamp(&raw, path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::new(
            error.kind(),
            format!(
                "failed to read cleanup sweep stamp from {}: {error}",
                path.display()
            ),
        )),
    }
}

/// Loads the stamp with fail-safe degradation: missing, corrupt or unreadable
/// files log one warning and evaluate to `None` ("due").
pub fn load_stamp_fail_safe(path: &Path) -> Option<i64> {
    match load_stamp(path) {
        Ok(stamp) => stamp,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "cleanup sweep stamp missing, corrupt or unreadable; degrading fail-safe to due"
            );
            None
        }
    }
}

/// Atomically persists the stamp: unique temp file, write, `sync_all`, rename,
/// mode 0600. A crash leaves the previous stamp intact.
pub fn save_stamp(path: &Path, last_cleanup_sweep: i64) -> std::io::Result<()> {
    let payload = serde_json::to_vec(&CleanupSweepStamp {
        version: 1,
        last_cleanup_sweep,
    })
    .map_err(|error| invalid_data(path, error.to_string()))?;
    let tmp_path = PathBuf::from(format!(
        "{}.{}_{}.tmp",
        path.display(),
        std::process::id(),
        mem_profile::ids::now_millis()
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(&tmp_path).map_err(|error| {
        Error::new(
            error.kind(),
            format!("failed to create temp file {}: {error}", tmp_path.display()),
        )
    })?;

    let written = file.write_all(&payload).and_then(|()| file.sync_all());
    drop(file);

    if let Err(error) = written {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::new(
            error.kind(),
            format!("failed to write stamp to {}: {error}", tmp_path.display()),
        ));
    }

    if let Err(error) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::new(
            error.kind(),
            format!(
                "failed to rename {} to {}: {error}",
                tmp_path.display(),
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use mem_profile::decay::sweep_is_due;

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "mem-web-cleanup-sweep-{label}-{}-{counter}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_stamp_round_trip() {
        let dir = TempDir::new("round-trip");
        let path = cleanup_sweep_path(dir.path());
        assert_eq!(path, dir.path().join(CLEANUP_SWEEP_FILE_NAME));
        save_stamp(&path, 1_730_000_000_000).expect("the stamp is written");
        assert_eq!(load_stamp(&path).unwrap(), Some(1_730_000_000_000));
        assert_eq!(load_stamp_fail_safe(&path), Some(1_730_000_000_000));
        let raw = fs::read_to_string(&path).expect("the stamp is readable");
        assert!(
            raw.contains("last_cleanup_sweep"),
            "the sidecar carries its own field name, never last_decay_sweep: {raw}"
        );
        assert!(!raw.contains("last_decay_sweep"));
    }

    /// A fresh root has no stamp and must be due — mapping `NotFound` to an
    /// error would leave the sweep permanently disabled.
    #[test]
    fn test_missing_stamp_is_due() {
        let dir = TempDir::new("missing");
        let path = cleanup_sweep_path(dir.path());
        assert_eq!(load_stamp(&path).unwrap(), None);
        assert_eq!(load_stamp_fail_safe(&path), None);
        assert!(
            sweep_is_due(None, 1_730_000_000_000, 86_400_000),
            "no stamp must read as due"
        );
    }

    #[test]
    fn test_corrupt_and_wrong_version_stamps_degrade_to_due() {
        let dir = TempDir::new("corrupt");
        let corrupt = dir.path().join("corrupt.json");
        fs::write(&corrupt, "{corrupt").expect("write corrupt fixture");
        assert!(load_stamp(&corrupt).is_err());
        assert_eq!(load_stamp_fail_safe(&corrupt), None);

        let wrong_version = dir.path().join("v2.json");
        fs::write(&wrong_version, "{\"version\":2,\"last_cleanup_sweep\":1}")
            .expect("write version fixture");
        assert!(load_stamp(&wrong_version).is_err());
        assert_eq!(load_stamp_fail_safe(&wrong_version), None);
    }

    #[test]
    fn test_stamp_write_is_atomic_and_0600() {
        let dir = TempDir::new("atomic");
        let path = cleanup_sweep_path(dir.path());
        save_stamp(&path, 42).expect("the stamp is written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path)
                .expect("the stamp exists")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the stamp is owner-only");
        }

        let entries: Vec<String> = fs::read_dir(dir.path())
            .expect("the temp dir is readable")
            .map(|entry| {
                entry
                    .expect("a dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            entries,
            vec![CLEANUP_SWEEP_FILE_NAME.to_string()],
            "no temporary file survives a successful write"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let read_only = TempDir::new("read-only");
            fs::set_permissions(read_only.path(), fs::Permissions::from_mode(0o500))
                .expect("the directory is made read-only");
            let blocked = cleanup_sweep_path(read_only.path());
            assert!(
                save_stamp(&blocked, 7).is_err(),
                "an unwritable directory must answer Err, never silently succeed"
            );
            assert!(!blocked.exists(), "a failed write leaves no stamp behind");
        }
    }
}
