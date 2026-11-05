//! Sidecar `decay-sweep.json` stamp store for daily profile decay (Issue 102).
//!
//! # Downgrade and Schema Evolution
//! The sidecar file `decay-sweep.json` resides beside `user-profiles.db`. It is
//! purely additive: older builds of the engine ignore unknown files in the
//! storage root, while this and future builds degrade unknown or incompatible
//! schema versions fail-safe to due (`None`), safely running a decay sweep
//! rather than aborting engine startup or skipping sweeps permanently.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ProfileError;

/// Sidecar file name stored beside `user-profiles.db`.
pub const DECAY_SWEEP_FILE_NAME: &str = "decay-sweep.json";

fn default_version() -> u32 {
    1
}

/// On-disk schema for `decay-sweep.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecaySweepStamp {
    /// Schema version (defaults to 1 for backward compatibility).
    #[serde(default = "default_version")]
    pub version: u32,
    /// Unix timestamp in milliseconds of the last successful decay sweep pass.
    pub last_decay_sweep: i64,
}

/// Resolves the sidecar file path beside the profile storage directory or database file.
pub fn decay_sweep_path(storage_path: &Path) -> PathBuf {
    storage_path.join(DECAY_SWEEP_FILE_NAME)
}

/// Parses the raw JSON content of the stamp file, validating schema version.
pub fn parse_stamp(raw: &str, path: &Path) -> Result<i64, ProfileError> {
    let stamp: DecaySweepStamp = serde_json::from_str(raw).map_err(|error| ProfileError::Json {
        message: format!("decay sweep stamp at {}: {error}", path.display()),
        line: error.line(),
        column: error.column(),
    })?;
    if stamp.version != 1 {
        return Err(ProfileError::Save(format!(
            "decay sweep stamp at {}: unsupported version {} (expected 1)",
            path.display(),
            stamp.version
        )));
    }
    Ok(stamp.last_decay_sweep)
}

/// Loads the decay sweep timestamp from disk, returning `Ok(None)` if the file does not exist.
pub fn load_stamp(path: &Path) -> Result<Option<i64>, ProfileError> {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse_stamp(&raw, path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ProfileError::io_message(
            format!("failed to read decay sweep stamp from {}", path.display()),
            &error,
        )),
    }
}

/// Loads the decay sweep timestamp with fail-safe degradation: missing, corrupt,
/// or unreadable files log a single warning and evaluate to `None` ("due").
pub fn load_stamp_fail_safe(path: &Path) -> Option<i64> {
    match load_stamp(path) {
        Ok(stamp) => stamp,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "decay sweep stamp missing, corrupt or unreadable; degrading fail-safe to due"
            );
            None
        }
    }
}

/// Serializes the stamp to UTF-8 JSON bytes.
pub fn serialize_stamp(last_decay_sweep: i64) -> Result<Vec<u8>, ProfileError> {
    let stamp = DecaySweepStamp {
        version: 1,
        last_decay_sweep,
    };
    serde_json::to_vec(&stamp).map_err(|error| ProfileError::json(&error))
}

/// Atomically persists the decay sweep stamp to disk using a unique temporary file
/// and atomic rename, setting file permissions to 0600 on Unix.
pub fn save_stamp(path: &Path, last_decay_sweep: i64) -> Result<(), ProfileError> {
    let payload = serialize_stamp(last_decay_sweep)?;
    let tmp_path = PathBuf::from(format!(
        "{}.{}_{}.tmp",
        path.display(),
        std::process::id(),
        crate::ids::now_millis()
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = match options.open(&tmp_path) {
        Ok(file) => file,
        Err(error) => {
            return Err(ProfileError::io_message(
                format!("failed to create temp file {}", tmp_path.display()),
                &error,
            ));
        }
    };

    let outcome = (|| -> Result<(), std::io::Error> {
        file.write_all(&payload)?;
        file.sync_all()?;
        Ok(())
    })();

    drop(file);

    match outcome {
        Ok(()) => {
            if let Err(error) = std::fs::rename(&tmp_path, path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(ProfileError::io_message(
                    format!(
                        "failed to rename {} to {}",
                        tmp_path.display(),
                        path.display()
                    ),
                    &error,
                ));
            }
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(ProfileError::io_message(
                format!("failed to write stamp to {}", tmp_path.display()),
                &error,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(prefix: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "mem-profile-decay-sweep-{prefix}-{}-{counter}",
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
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_decay_sweep_path() {
        assert_eq!(
            decay_sweep_path(Path::new("/tmp")),
            PathBuf::from("/tmp/decay-sweep.json")
        );
    }

    #[test]
    fn test_save_and_load_stamp_round_trip() {
        let dir = TempDir::new("round-trip");
        let path = decay_sweep_path(dir.path());
        save_stamp(&path, 1730000000000).unwrap();
        let loaded = load_stamp(&path).unwrap();
        assert_eq!(loaded, Some(1730000000000));
        assert_eq!(load_stamp_fail_safe(&path), Some(1730000000000));
    }

    #[test]
    fn test_load_stamp_missing_file_returns_none() {
        let dir = TempDir::new("missing");
        let path = decay_sweep_path(dir.path()).join("nonexistent");
        assert_eq!(load_stamp(&path).unwrap(), None);
        assert_eq!(load_stamp_fail_safe(&path), None);
    }

    #[test]
    fn test_load_stamp_corrupt_file_returns_none_fail_safe() {
        let dir = TempDir::new("corrupt");
        let path = decay_sweep_path(dir.path());
        fs::write(&path, b"{corrupt json").unwrap();
        assert!(load_stamp(&path).is_err());
        assert_eq!(load_stamp_fail_safe(&path), None);
    }

    #[test]
    fn test_load_stamp_backward_compatible_version_default() {
        let dir = TempDir::new("backward-compat");
        let path = decay_sweep_path(dir.path());
        fs::write(&path, b"{\"last_decay_sweep\": 12345}").unwrap();
        assert_eq!(load_stamp(&path).unwrap(), Some(12345));
        assert_eq!(load_stamp_fail_safe(&path), Some(12345));
    }

    #[test]
    fn test_atomic_save_stamp_mode_and_cleanup() {
        let dir = TempDir::new("atomic-save");
        let path = decay_sweep_path(dir.path());
        save_stamp(&path, 1730000000000).unwrap();
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec![DECAY_SWEEP_FILE_NAME.to_string()]);
    }
}
