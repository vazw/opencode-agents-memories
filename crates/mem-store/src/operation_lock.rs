//! Turso operation/migration lock — port of upstream
//! `src/services/turso/operation-lock.ts` (blueprint W1.2 step 8).
//!
//! `CONFIG.storagePath` becomes the explicit `storage_path` argument of
//! every call (DIVERGENCE, recorded: upstream reads the global CONFIG —
//! the manager carries its own root). `process.kill(pid, 0)` becomes the
//! injectable [`ProcessProbe`]. The upstream returned release closure is
//! kept as [`TursoOperationLock::release`] plus a `Drop` fallback
//! (reviewer note: upstream NEEDS the explicit call; Rust tolerates a
//! forgotten one).

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::platform::ProcessProbe;

/// Operation lock file name.
pub const OPERATION_LOCK_FILE: &str = ".turso-operation.lock";
/// Legacy migration lock file name (still honored on asserts).
pub const LEGACY_MIGRATION_LOCK_FILE: &str = ".turso-migrate.lock";
/// Shards metadata db name.
pub const METADATA_DB_NAME: &str = "metadata.db";

/// `LockState` — TS-written locks parse (upstream `interface LockState` with
/// `operation?`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockState {
    pub pid: u32,
    pub timestamp: String,
    #[serde(default)]
    pub operation: Option<String>,
}

/// Upstream `isProcessAlive`.
fn is_process_alive(pid: u32, probe: &dyn ProcessProbe) -> bool {
    probe.is_alive(pid)
}

/// Filesystem lock reading + stale cleanup (missing -> `None`; corrupt /
/// non-positive / dead-pid locks are removed best-effort and dropped).
pub fn read_live_lock(
    storage_path: &Path,
    file: &str,
    probe: &dyn ProcessProbe,
) -> Option<LockState> {
    let path = storage_path.join(file);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => return None, // missing file (or unreadable) -> no lock
    };
    // W1.5 short-write guard: a file that exists but reads back
    // empty/whitespace-only is a truncated/partial write (crash between
    // `create_new` and `write_all`) and can never be a valid `LockState`.
    // Treat it exactly like corrupt JSON: unlink best-effort, report no lock.
    if contents.trim().is_empty() {
        let _ = fs::remove_file(&path);
        return None;
    }
    match serde_json::from_str::<LockState>(&contents) {
        Ok(state) if state.pid > 0 && is_process_alive(state.pid, probe) => return Some(state),
        _ => {
            // Corrupt JSON, non-positive pid, or dead owner: try to remove.
            // (A live-pid lock is never stolen — it returns above.)
        }
    }
    let _ = fs::remove_file(&path);
    None
}

fn operation_lock_path(storage_path: &Path) -> PathBuf {
    storage_path.join(OPERATION_LOCK_FILE)
}

/// Upstream `assertNoTursoMigrationInProgress`. Sync (`writelocks` are
/// filesystem-only); probe of the caller's choosing.
pub fn assert_no_turso_migration_in_progress(
    storage_path: &Path,
    probe: &dyn ProcessProbe,
) -> Result<(), StoreError> {
    for file in [OPERATION_LOCK_FILE, LEGACY_MIGRATION_LOCK_FILE] {
        if let Some(state) = read_live_lock(storage_path, file, probe) {
            return Err(StoreError::MigrationLocked {
                pid: state.pid,
                operation: state.operation,
            });
        }
    }
    Ok(())
}

/// Upstream `acquireTursoOperationLock` return value.
pub struct TursoOperationLock {
    storage_path: PathBuf,
    probe: Arc<dyn ProcessProbe>,
    pid: u32,
}

impl TursoOperationLock {
    /// Upstream release closure: removes the lock when still owned by this
    /// process.
    pub fn release(&self) {
        let path = operation_lock_path(&self.storage_path);
        let ours = read_live_lock(&self.storage_path, OPERATION_LOCK_FILE, self.probe.as_ref())
            .is_some_and(|current| current.pid == self.pid);
        if ours {
            let _ = fs::remove_file(&path);
        }
    }
}

impl Drop for TursoOperationLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Upstream `acquireTursoOperationLock`. The lock file is written with
/// `create_new(true)` (`wx`); `AlreadyExists` re-asserts (the existing lock
/// may be stale) then propagates the `Io` error.
pub fn acquire_turso_operation_lock(
    storage_path: &Path,
    probe: Arc<dyn ProcessProbe>,
    operation: &str,
) -> Result<TursoOperationLock, StoreError> {
    assert_no_turso_migration_in_progress(storage_path, probe.as_ref())?;
    let path = operation_lock_path(storage_path);
    let pid = std::process::id();
    let state = LockState {
        pid,
        timestamp: now_rfc3339_millis(),
        operation: Some(operation.to_string()),
    };
    let body = serde_json::to_string(&state).map_err(|error| StoreError::Json {
        message: error.to_string(),
    })?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(body.as_bytes())?;
            // W1.5 short-write hardening: flush the lock body to disk before
            // the handle is dropped so a crash cannot leave a truncated
            // (empty/partial) lock file. Best-effort — `sync_all` failures on
            // exotic filesystems must not fail an otherwise valid acquire.
            let _ = file.sync_all();
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            assert_no_turso_migration_in_progress(storage_path, probe.as_ref())?;
            return Err(StoreError::from(error));
        }
        Err(error) => return Err(StoreError::from(error)),
    }
    Ok(TursoOperationLock {
        storage_path: storage_path.to_path_buf(),
        probe,
        pid,
    })
}

// ---------------------------------------------------------------------------
// Legacy migration lock (blueprint W1.3 step 5 — port of upstream
// `legacy-migrator.ts` `acquireMigrationLock`/`releaseMigrationLock`, L107-156)
// ---------------------------------------------------------------------------

/// Upstream `MigrationLock` (`{ pid, timestamp }` — compact JSON, no
/// `operation` field; foreign writers may carry extra fields, which serde
/// ignores).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyLockState {
    pid: u32,
    timestamp: String,
}

fn legacy_lock_path(storage_path: &Path) -> PathBuf {
    storage_path.join(LEGACY_MIGRATION_LOCK_FILE)
}

/// Upstream `acquireMigrationLock` (boolean form). A live holder blocks
/// (no unlink — the foreign file is NEVER touched); a stale (dead/non-positive
/// pid) or corrupt lock is unlinked best-effort and the atomic `wx`
/// (`create_new`) create runs; a lost create race reports failure.
/// `process.kill(pid, 0)` becomes the injected [`ProcessProbe`].
pub fn acquire_legacy_migration_lock(storage_path: &Path, probe: &dyn ProcessProbe) -> bool {
    let path = legacy_lock_path(storage_path);
    if let Ok(contents) = fs::read_to_string(&path) {
        match serde_json::from_str::<LegacyLockState>(&contents) {
            Ok(state) if state.pid != 0 && is_process_alive(state.pid, probe) => return false,
            _ => {
                // stale or corrupt — try to remove
                let _ = fs::remove_file(&path);
            }
        }
    } else if path.exists() {
        // unreadable file: upstream catch also unlinks before re-creating
        let _ = fs::remove_file(&path);
    }
    let state = LegacyLockState {
        pid: std::process::id(),
        timestamp: now_rfc3339_millis(),
    };
    let Ok(body) = serde_json::to_string(&state) else {
        return false;
    };
    let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    else {
        return false;
    };
    file.write_all(body.as_bytes()).is_ok()
}

/// Release handle for the legacy lock: removal is PID-guarded (a lock
/// re-taken by another process is never unlinked); a corrupt lock file is
/// unlinked (upstream `catch { unlinkSync }`). `Drop` is the fallback, the
/// explicit [`Self::release`] mirrors upstream's `finally` call.
pub struct LegacyMigrationLock {
    storage_path: PathBuf,
    pid: u32,
}

impl LegacyMigrationLock {
    /// Upstream `releaseMigrationLock`.
    pub fn release(&self) {
        let path = legacy_lock_path(&self.storage_path);
        if !path.exists() {
            return;
        }
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<LegacyLockState>(&contents) {
                Ok(state) if state.pid == self.pid => {
                    let _ = fs::remove_file(&path);
                }
                _ => {}
            },
            Err(_) => {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

impl Drop for LegacyMigrationLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Upstream `acquireMigrationLockWithRetry` (`max_attempts` default 3).
/// DIVERGENCE (pinned by the legacy-lock test): upstream sleeps after EVERY
/// failed attempt including the last; this port sleeps only BETWEEN attempts
/// — exactly `[200ms, 350ms]` for 3 attempts — and never before the terminal
/// `locked by another process` error.
pub fn acquire_legacy_migration_lock_with_retry(
    storage_path: &Path,
    probe: Arc<dyn ProcessProbe>,
    sleep: &crate::platform::SleepFn,
    max_attempts: u32,
) -> Result<LegacyMigrationLock, StoreError> {
    for attempt in 0..max_attempts {
        if acquire_legacy_migration_lock(storage_path, probe.as_ref()) {
            return Ok(LegacyMigrationLock {
                storage_path: storage_path.to_path_buf(),
                pid: std::process::id(),
            });
        }
        if attempt + 1 < max_attempts {
            let delay = 200u64 + 150 * u64::from(attempt);
            sleep(std::time::Duration::from_millis(delay));
        }
    }
    Err(StoreError::internal(
        "Turso legacy migration locked by another process",
    ))
}

/// `new Date().toISOString()` — UTC, millisecond precision, trailing `Z`
/// (`Utc::now().to_rfc3339_opts(Millis, true)` chrono equivalent).
/// `pub(crate)`: W1.3 reuses it for the engine + legacy migration markers
/// (blueprint W1.3 step 4: "reuse/extract the existing RFC3339-millis
/// helper from `operation_lock.rs`").
pub(crate) fn now_rfc3339_millis() -> String {
    fn to_iso_millis(unix_millis: i64) -> String {
        let days = unix_millis.div_euclid(86_400_000);
        let millis_of_day = unix_millis.rem_euclid(86_400_000);
        // Civil-from-days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let (year, month, day) = ((y + i64::from(m <= 2)), m, d);
        let hour = millis_of_day / 3_600_000;
        let minute = (millis_of_day % 3_600_000) / 60_000;
        let second = (millis_of_day % 60_000) / 1000;
        let millis = millis_of_day % 1000;
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
    }
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    to_iso_millis(now_millis)
}
