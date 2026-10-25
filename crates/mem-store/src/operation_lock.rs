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
//!
//! The record on disk is a [`LockRecord`]: the TS-compatible [`LockState`]
//! fields plus a Linux start-time token, so a recycled pid cannot validate the
//! previous owner's lock (`read_live_lock`). Where the token is unobservable
//! the verdict stays conservative — see `platform::decide_liveness`.

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
///
/// **No `deny_unknown_fields`** (verified) and `operation` is `#[serde(default)]`,
/// so serde ignores keys it does not know: a newer record that carries extra
/// fields (today: the on-disk [`LockRecord::start_time`]) still parses here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockState {
    pub pid: u32,
    pub timestamp: String,
    #[serde(default)]
    pub operation: Option<String>,
}

/// The on-disk lock record: [`LockState`] plus the start-time token that
/// defeats PID reuse.
///
/// The token is a **sibling** of the `LockState` fields in the JSON object
/// (`#[serde(flatten)]`), which is what keeps the wire format exactly
/// `{"pid":…,"timestamp":…,"operation":…,"start_time":…}` while leaving
/// `LockState`'s three-field shape alone — other crates' tests build
/// `LockState` with a struct literal, and a fourth field would be a
/// source-breaking change for files this fix must not touch.
///
/// Compatibility both directions rests on serde's default behaviour:
///
/// * old record (no `start_time`) read here -> `#[serde(default)]` makes it
///   `None` -> the liveness verdict stays conservative (`Alive`);
/// * new record read by an **old** binary -> neither `LockState` nor this type
///   denies unknown fields, so the extra key is ignored and the three known
///   fields parse.
///
/// The token is `None` off Linux (`/proc` is the only source) and is then
/// written as `null`, which reads back as `None` — the same conservative cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockRecord {
    #[serde(flatten)]
    pub state: LockState,
    #[serde(default)]
    pub start_time: Option<u64>,
}

impl LockRecord {
    /// This process's record for `operation` — the only writer in this crate.
    pub(crate) fn for_this_process(pid: u32, operation: Option<String>) -> Self {
        Self {
            state: LockState {
                pid,
                timestamp: now_rfc3339_millis(),
                operation,
            },
            start_time: crate::platform::process_start_time(pid),
        }
    }
}

/// Upstream `isProcessAlive`.
fn is_process_alive(pid: u32, probe: &dyn ProcessProbe) -> bool {
    probe.is_alive(pid)
}

/// Filesystem lock reading + stale cleanup (missing -> `None`; corrupt /
/// non-positive / dead-owner locks are removed best-effort and dropped).
///
/// "Dead owner" is the probe's token-aware verdict: the pid is gone, **or** the
/// pid exists but its start-time token differs from the one recorded when the
/// lock was taken (PID reuse). A record without a token, and any non-procfs
/// host, can never prove a reuse and therefore reads `Alive` — the owner's lock
/// is never stolen on a guess.
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
    if let Ok(record) = serde_json::from_str::<LockRecord>(&contents)
        && record.state.pid > 0
        && probe
            .liveness(record.state.pid, record.start_time)
            .is_alive()
    {
        return Some(record.state);
    }
    // Corrupt JSON, non-positive pid, or a dead owner (absent pid, or a
    // recycled pid per the start-time token): try to remove. A live-owner lock
    // is never stolen — it returns above.
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
    let record = LockRecord::for_this_process(pid, Some(operation.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Liveness, SystemProcessProbe, process_start_time};
    use std::sync::Mutex;

    /// A fresh, uniquely named directory under `TMPDIR` (the house temp-dir
    /// pattern from `crates/mem-store/tests/engine_lock.rs`).
    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mem-store-oplock-unit-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_lock(storage_path: &Path, body: &str) -> PathBuf {
        let path = storage_path.join(OPERATION_LOCK_FILE);
        fs::write(&path, body).expect("write lock body");
        path
    }

    /// A hand-built record body: `start_time` present only when asked for, so
    /// the "old record" fixture is byte-identical to what a pre-token writer
    /// (upstream TS) leaves behind.
    fn record_json(pid: u32, start_time: Option<u64>) -> String {
        let token = match start_time {
            Some(token) => format!(r#","start_time":{token}"#),
            None => String::new(),
        };
        format!(
            r#"{{"pid":{pid},"timestamp":"2026-10-20T00:00:00.000Z","operation":"test"{token}}}"#
        )
    }

    /// The pre-fix probe shape: a fixed existence answer, token-blind through
    /// the trait's default `liveness`.
    struct FixedProbe(bool);

    impl ProcessProbe for FixedProbe {
        fn is_alive(&self, _pid: u32) -> bool {
            self.0
        }
    }

    /// Answers alive and records the token `read_live_lock` handed over.
    #[derive(Default)]
    struct RecordingProbe {
        seen: Mutex<Vec<Option<u64>>>,
    }

    impl ProcessProbe for RecordingProbe {
        fn is_alive(&self, _pid: u32) -> bool {
            true
        }

        fn liveness(&self, _pid: u32, recorded_start: Option<u64>) -> Liveness {
            self.seen.lock().expect("probe mutex").push(recorded_start);
            Liveness::Alive
        }
    }

    /// A live holder's lock file must survive `read_live_lock`.
    ///
    /// Inverted world: if the live branch unlinked (or the verdict flipped to
    /// `Dead` for an alive pid), the file would be gone and a second process
    /// would acquire a lock another session still owns.
    #[test]
    fn test_live_holder_lock_is_not_unlinked() {
        let dir = temp_dir("live-holder");
        let path = write_lock(&dir, &record_json(4242, Some(1_000)));

        let state = read_live_lock(&dir, OPERATION_LOCK_FILE, &FixedProbe(true))
            .expect("a live holder's lock is returned");

        assert_eq!(state.pid, 4242);
        assert_eq!(state.operation.as_deref(), Some("test"));
        assert!(path.exists(), "a live holder's lock file must survive");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A dead holder's lock file is stale and must be unlinked.
    ///
    /// Inverted world: if the dead branch stopped unlinking, every later
    /// acquire in this directory fails with `MigrationLocked` forever.
    #[test]
    fn test_dead_holder_lock_is_unlinked() {
        let dir = temp_dir("dead-holder");
        let path = write_lock(&dir, &record_json(4242, Some(1_000)));

        let state = read_live_lock(&dir, OPERATION_LOCK_FILE, &FixedProbe(false));

        assert!(state.is_none(), "a dead holder's lock reads as no lock");
        assert!(!path.exists(), "a dead holder's lock file is removed");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The recorded token reaches the probe, present and absent.
    ///
    /// Inverted world: if `read_live_lock` parsed `LockState` (no token) or
    /// passed `None`, the first cell would record `None` and the PID-reuse
    /// defence would be dead code that no test notices.
    #[test]
    fn test_recorded_start_time_reaches_the_probe() {
        let dir = temp_dir("token-passthrough");
        let probe = RecordingProbe::default();

        write_lock(&dir, &record_json(4242, Some(777)));
        assert!(read_live_lock(&dir, OPERATION_LOCK_FILE, &probe).is_some());

        write_lock(&dir, &record_json(4242, None));
        assert!(read_live_lock(&dir, OPERATION_LOCK_FILE, &probe).is_some());

        let seen = probe.seen.lock().expect("probe mutex").clone();
        assert_eq!(
            seen,
            vec![Some(777), None],
            "the on-disk token is handed to the probe, and its absence is None"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// An old record (no `start_time`) still parses and stays conservative: the
    /// real probe reports this process alive, so the lock must be kept.
    ///
    /// Inverted world: if an absent token were read as a mismatch (or the old
    /// shape failed to parse), the file would be unlinked — the backward
    /// compatibility break that would steal a live pre-upgrade holder's lock.
    #[test]
    fn test_legacy_record_without_token_is_kept() {
        let dir = temp_dir("legacy-record");
        let me = std::process::id();
        let path = write_lock(&dir, &record_json(me, None));

        let state = read_live_lock(&dir, OPERATION_LOCK_FILE, &SystemProcessProbe)
            .expect("a token-less record from a live pid parses and reads alive");

        assert_eq!(state.pid, me);
        assert!(
            path.exists(),
            "the legacy record is never stolen on a guess"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A record carrying a field this binary does not know still parses: the
    /// forward-compatibility half of the compatibility contract (no
    /// `deny_unknown_fields` on `LockState`/`LockRecord`).
    ///
    /// Inverted world: with `deny_unknown_fields`, the body would fail to parse,
    /// `read_live_lock` would unlink it and report `None` — the new binary would
    /// steal a live holder's lock written by a future version.
    #[test]
    fn test_newer_record_with_unknown_field_still_parses() {
        let dir = temp_dir("unknown-field");
        let me = std::process::id();
        let path = write_lock(
            &dir,
            &format!(
                r#"{{"pid":{me},"timestamp":"2026-10-20T00:00:00.000Z","operation":"test","future_field":1}}"#
            ),
        );

        let state = read_live_lock(&dir, OPERATION_LOCK_FILE, &SystemProcessProbe)
            .expect("unknown fields are ignored, the known ones parse");

        assert_eq!(state.pid, me);
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// **The blast radius of the audit finding**: a record whose pid is reused
    /// by another process must be reclaimed, because the start-time token no
    /// longer matches. The real probe is used, so the whole chain
    /// `/proc` -> token -> verdict -> unlink is exercised.
    ///
    /// Inverted world: without the token check the pid is "alive", the lock is
    /// returned, the file survives, and the storage root is locked out forever
    /// by a process that long since exited.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_reused_pid_record_is_unlinked() {
        let dir = temp_dir("pid-reuse");
        let me = std::process::id();
        let own = process_start_time(me).expect("own start-time token");
        let path = write_lock(&dir, &record_json(me, Some(own.wrapping_add(1))));

        let state = read_live_lock(&dir, OPERATION_LOCK_FILE, &SystemProcessProbe);

        assert!(
            state.is_none(),
            "a pid whose start time changed is a different process"
        );
        assert!(
            !path.exists(),
            "the recycled-pid lock is stale and is unlinked"
        );

        // Same pid, correct token: the very same file is now a live lock.
        let path = write_lock(&dir, &record_json(me, Some(own)));
        assert!(
            read_live_lock(&dir, OPERATION_LOCK_FILE, &SystemProcessProbe).is_some(),
            "the matching token validates the holder"
        );
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Acquisition records its own start-time token, so the record it leaves
    /// behind is the one the token check can validate.
    ///
    /// Inverted world: if the writer omitted the token, `start_time` would be
    /// `None` and every fresh lock would rely on the legacy conservative path
    /// instead of defeating PID reuse.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_acquired_record_carries_the_owner_start_time() {
        let dir = temp_dir("record-token");
        let lock = acquire_turso_operation_lock(&dir, Arc::new(SystemProcessProbe), "unit-op")
            .expect("acquire over a free directory");

        let body = fs::read_to_string(dir.join(OPERATION_LOCK_FILE)).expect("read lock body");
        let record: LockRecord = serde_json::from_str(&body).expect("body is a LockRecord");
        let me = std::process::id();
        assert_eq!(record.state.pid, me);
        assert_eq!(record.state.operation.as_deref(), Some("unit-op"));
        assert_eq!(
            record.start_time,
            process_start_time(me),
            "the writer records its own start-time token"
        );
        assert!(record.start_time.is_some(), "Linux always observes it");

        drop(lock);
        assert!(
            !dir.join(OPERATION_LOCK_FILE).exists(),
            "release still unlinks the lock it owns"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
