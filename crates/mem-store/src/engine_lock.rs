//! Engine lifetime lock — W5.1 (ADR-0007 D-39, D-47).
//!
//! Exactly **one** engine process owns the storage root's databases. The lock
//! is a distinct file, `.engine.lock`:
//!
//! * `.turso-operation.lock` is **operation-scoped** (`acquire_turso_operation_lock`
//!   records the operation and releases it when the operation ends), and
//!   `assert_no_turso_migration_in_progress` fails on any live holder — so an
//!   engine holding it for its whole lifetime would make `get_write_shard`
//!   refuse the engine's own writes, and would collide with the W4.3
//!   consolidation run.
//! * `.turso-migrate.lock` is the legacy migration lock, also asserted.
//!
//! The body is a [`LockRecord`] — the [`LockState`] fields (`pid`, `timestamp`,
//! `operation`) plus the writer's start-time token — **not** a bare pid:
//! `read_live_lock` parses the record and unlinks anything it cannot parse, so
//! a plain-pid body would be read as corrupt and stolen — two DB owners. That
//! is the single most dangerous failure this module can have, and
//! `test_engine_lock_body_is_read_live_lock_parseable` (integration suite) pins
//! it. The token makes a recycled pid read as the *previous* owner, so a lock
//! left behind by an exited engine is reclaimed instead of blocking forever.
//!
//! The loser never opens the DB: it reports the **holder's** pid (read from the
//! live lock), writes the one-line status file, and exits with
//! [`ENGINE_LOCK_LOST`] (75). The status file is a report only — whether a
//! second host instance should degrade or be pointed elsewhere is an open
//! question for the user (ADR-0007 documented limitation (ii)); nothing here
//! answers it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::StoreError;
use crate::operation_lock::{LockRecord, LockState, now_rfc3339_millis, read_live_lock};
use crate::platform::ProcessProbe;

/// The lifetime lock file name.
pub const ENGINE_LOCK_FILE: &str = ".engine.lock";

/// The status file name inside the storage root.
pub const STATUS_FILE: &str = "status";

/// The `LockState.operation` value the engine records.
pub const ENGINE_LOCK_OPERATION: &str = "engine";

/// Exit code of the engine that lost the lock (D-39 / blueprint Step 3).
pub const ENGINE_LOCK_LOST: i32 = 75;

/// Status-file line prefix.
const STATUS_PREFIX: &str = "degraded: ";

/// The status line is bounded at 200 characters **plus** the newline.
const MAX_STATUS_CHARS: usize = 200;

/// The outcome of [`acquire_engine_lock`].
#[derive(Debug)]
pub enum EngineLockOutcome {
    /// This process owns the engine lock for as long as the handle lives.
    Acquired(EngineLock),
    /// Another live engine owns it; `holder_pid` is the pid in the live lock.
    Lost {
        /// The pid recorded by the live holder.
        holder_pid: u32,
    },
}

/// A held engine lock. Dropping it releases the lock (lifetime scope).
pub struct EngineLock {
    storage_path: PathBuf,
    probe: Arc<dyn ProcessProbe>,
    pid: u32,
}

impl std::fmt::Debug for EngineLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLock")
            .field("path", &engine_lock_path(&self.storage_path))
            .field("pid", &self.pid)
            .finish()
    }
}

impl EngineLock {
    /// The lock file path.
    pub fn path(&self) -> PathBuf {
        engine_lock_path(&self.storage_path)
    }

    /// Removes the lock when this process still owns it.
    ///
    /// A lock re-taken by another process is never unlinked. Best-effort: a
    /// release failure must not turn a clean shutdown into an error.
    pub fn release(&self) {
        let live = read_live_lock(&self.storage_path, ENGINE_LOCK_FILE, self.probe.as_ref());
        if live.is_some_and(|current| current.pid == self.pid) {
            let _ = fs::remove_file(engine_lock_path(&self.storage_path));
        }
    }
}

impl Drop for EngineLock {
    fn drop(&mut self) {
        self.release();
    }
}

fn engine_lock_path(storage_path: &Path) -> PathBuf {
    storage_path.join(ENGINE_LOCK_FILE)
}

/// Creates the lock file exclusively and writes the `LockRecord` body.
///
/// Returns `Ok(false)` when the file already exists.
fn try_create(storage_path: &Path, pid: u32) -> Result<bool, StoreError> {
    let path = engine_lock_path(storage_path);
    let record = LockRecord::for_this_process(pid, Some(ENGINE_LOCK_OPERATION.to_string()));
    let body = serde_json::to_string(&record).map_err(|error| StoreError::Json {
        message: error.to_string(),
    })?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(body.as_bytes())?;
            // Flush before the handle drops, so a crash cannot leave a
            // truncated body that reads as corrupt and gets stolen.
            let _ = file.sync_all();
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(StoreError::from(error)),
    }
}

fn current_holder(storage_path: &Path, probe: &dyn ProcessProbe) -> Option<LockState> {
    read_live_lock(storage_path, ENGINE_LOCK_FILE, probe)
}

/// Acquires the lifetime engine lock.
///
/// `create_new(true)` is the atomic acquire. An `AlreadyExists` result is not
/// final: [`read_live_lock`] removes a dead/corrupt lock, after which the
/// acquire is retried **once**. A still-live holder makes this call return
/// [`EngineLockOutcome::Lost`] with that holder's pid.
pub fn acquire_engine_lock(
    storage_path: &Path,
    probe: Arc<dyn ProcessProbe>,
) -> Result<EngineLockOutcome, StoreError> {
    fs::create_dir_all(storage_path)?;
    let pid = std::process::id();
    for _attempt in 0..2 {
        if try_create(storage_path, pid)? {
            return Ok(EngineLockOutcome::Acquired(EngineLock {
                storage_path: storage_path.to_path_buf(),
                probe,
                pid,
            }));
        }
        // `read_live_lock` unlinks a dead-owner or corrupt lock as a side
        // effect; a live holder is returned unchanged.
        if let Some(state) = current_holder(storage_path, probe.as_ref()) {
            return Ok(EngineLockOutcome::Lost {
                holder_pid: state.pid,
            });
        }
    }
    // Two rounds and still no lock, with nothing readable: report the holder we
    // can see (0 when the file vanished mid-race). The caller still degrades.
    Ok(EngineLockOutcome::Lost {
        holder_pid: current_holder(storage_path, probe.as_ref())
            .map(|state| state.pid)
            .unwrap_or(0),
    })
}

/// Renders the single status line:
/// `degraded: <short reason> (pid=<n>, ts=<UTC ISO-8601>)`.
///
/// The reason is sanitised (no newlines) and truncated on a character boundary
/// so the whole line, **including** the trailing newline, is ≤ 200 characters.
pub fn status_line(reason: &str, pid: u32) -> String {
    let timestamp = now_rfc3339_millis();
    let suffix = format!(" (pid={pid}, ts={timestamp})");
    let budget = MAX_STATUS_CHARS.saturating_sub(1 + STATUS_PREFIX.len() + suffix.len());
    let sanitised: String = reason
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let reason: String = sanitised.trim().chars().take(budget).collect();
    format!("{STATUS_PREFIX}{reason}{suffix}")
}

/// Writes `<storage_path>/status` (one line, overwriting any previous report).
///
/// Two writers exist: the lock loser (recording the **holder's** pid) and the
/// JS wrapper (recording its own). The owner never deletes a status it did not
/// write, so nothing in this module removes the file.
pub fn write_status_file(storage_path: &Path, reason: &str, pid: u32) -> Result<(), StoreError> {
    fs::create_dir_all(storage_path)?;
    let line = status_line(reason, pid);
    fs::write(storage_path.join(STATUS_FILE), format!("{line}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_lock::OPERATION_LOCK_FILE;
    use crate::platform::process_start_time;

    #[test]
    fn status_line_is_bounded_one_line_and_carries_pid_and_timestamp() {
        let long = "x".repeat(500);
        let line = status_line(&long, 4242);
        assert!(line.len() < MAX_STATUS_CHARS, "{}", line.len());
        assert!(!line.contains('\n'));
        assert!(line.starts_with("degraded: "));
        assert!(line.contains("pid=4242"));
        assert!(line.contains("ts="));
        assert!(line.ends_with(')'));
    }

    /// A fresh, uniquely named directory under `TMPDIR`.
    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mem-store-engine-lock-unit-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// The engine lock's body carries the owner's start-time token, and
    /// `read_live_lock` validates it against `/proc`.
    ///
    /// Inverted world: if `try_create` wrote a token-less `LockState` body, the
    /// `start_time` assertion would see `None` and the record would fall back to
    /// the conservative legacy path instead of defeating PID reuse.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_engine_lock_body_carries_the_owner_start_time() {
        let dir = temp_dir("body-token");
        let EngineLockOutcome::Acquired(lock) =
            acquire_engine_lock(&dir, Arc::new(crate::platform::SystemProcessProbe))
                .expect("acquire over a free directory")
        else {
            panic!("a free storage root must be acquired");
        };

        let body = fs::read_to_string(engine_lock_path(&dir)).expect("read engine lock body");
        let record: LockRecord = serde_json::from_str(&body).expect("body is a LockRecord");
        let me = std::process::id();
        assert_eq!(record.state.pid, me);
        assert_eq!(
            record.state.operation.as_deref(),
            Some(ENGINE_LOCK_OPERATION)
        );
        assert_eq!(record.start_time, process_start_time(me));
        assert!(record.start_time.is_some(), "Linux observes the token");

        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A record naming **this** pid with a stale token is a recycled pid, so the
    /// engine lock is stale and must be reclaimed rather than blocking this
    /// process forever.
    ///
    /// Inverted world: if the token were ignored, the pid would read alive and
    /// `acquire_engine_lock` would answer `Lost` — an engine locked out of its
    /// own storage root by a dead predecessor.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_engine_lock_reclaims_a_reused_pid_record() {
        let dir = temp_dir("reused-pid");
        let path = engine_lock_path(&dir);
        let me = std::process::id();
        let own = process_start_time(me).expect("own start-time token");
        let stale = LockRecord {
            state: LockState {
                pid: me,
                timestamp: "2026-10-20T00:00:00.000Z".to_string(),
                operation: Some(ENGINE_LOCK_OPERATION.to_string()),
            },
            start_time: Some(own.wrapping_add(1)),
        };
        fs::write(&path, serde_json::to_string(&stale).expect("serialize"))
            .expect("write stale engine lock");

        let outcome = acquire_engine_lock(&dir, Arc::new(crate::platform::SystemProcessProbe))
            .expect("acquire does not fail on io");
        let EngineLockOutcome::Acquired(lock) = outcome else {
            panic!("a recycled pid's lock must be reclaimed, got {outcome:?}");
        };

        let body = fs::read_to_string(&path).expect("read reclaimed body");
        let record: LockRecord = serde_json::from_str(&body).expect("reclaimed body parses");
        assert_eq!(
            record.start_time,
            Some(own),
            "the reclaimed record belongs to this process"
        );
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Sanity: the operation-lock constant is not the engine lock's file, so a
    /// reclaimed engine lock never touches another session's operation lock.
    ///
    /// Inverted world: if the two names collided, the assertion fails.
    #[test]
    fn test_engine_lock_file_is_distinct_from_the_operation_lock() {
        assert_ne!(ENGINE_LOCK_FILE, OPERATION_LOCK_FILE);
    }
}
