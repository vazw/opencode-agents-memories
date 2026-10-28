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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

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
    /// This process owns the engine lock for as long as every handle lives.
    /// The `Arc` is the registry entry [`crate::delete_guard::ExclusiveRoot`]
    /// is built from: taking the lock by value would require `Arc::try_unwrap`,
    /// which frees the allocation the registered `Weak` points at.
    Acquired(Arc<EngineLock>),
    /// Another live engine owns it; `holder_pid` is the pid in the live lock.
    Lost {
        /// The pid recorded by the live holder.
        holder_pid: u32,
    },
}

/// A held engine lock. Dropping the last handle releases the lock (lifetime
/// scope) and withdraws the registered proof.
pub struct EngineLock {
    storage_path: PathBuf,
    probe: Arc<dyn ProcessProbe>,
    pid: u32,
    /// Per-lock idempotence for [`EngineLock::release`]: after a manual release
    /// the file may already belong to a newer holder, so `Drop` must not
    /// release (and thereby unlink) that newer holder's file.
    released: AtomicBool,
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

    /// The storage root this lock covers. [`crate::delete_guard::ExclusiveRoot::current`]
    /// matches it **exactly** (never by prefix).
    pub fn storage_root(&self) -> &Path {
        &self.storage_path
    }

    /// Removes the lock when this process still owns it.
    ///
    /// A lock re-taken by another process is never unlinked. Best-effort: a
    /// release failure must not turn a clean shutdown into an error.
    ///
    /// Idempotent per lock (`released`): once this lock has released its file,
    /// a later call is a no-op — the path may by then hold a **newer**
    /// registration's lock, which this lock must not unlink.
    pub fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        let live = read_live_lock(&self.storage_path, ENGINE_LOCK_FILE, self.probe.as_ref());
        if live.is_some_and(|current| current.pid == self.pid) {
            let _ = fs::remove_file(engine_lock_path(&self.storage_path));
        }
    }
}

impl Drop for EngineLock {
    fn drop(&mut self) {
        self.release();
        deregister(self);
    }
}

fn engine_lock_path(storage_path: &Path) -> PathBuf {
    storage_path.join(ENGINE_LOCK_FILE)
}

/// The process-wide registry of live engine locks — the seam
/// [`crate::delete_guard::ExclusiveRoot::current`] reads.
///
/// It publishes "this process holds the engine lock for this root" as a fact
/// the *lock* controls; consumers still convert the absence of an entry into a
/// typed refusal. `purge-session` is a legitimate second process with no lock,
/// so it simply sees `None`.
static LOCK_REGISTRY: Mutex<Vec<Weak<EngineLock>>> = Mutex::new(Vec::new());

/// Serialises the lib tests that assert on the process-global registry: a
/// concurrent [`registered_lock`] temporarily upgrades **every** live entry,
/// so a proof can legitimately outlive its owner's `drop` by exactly that
/// lookup's own clone. Assertions such as "the last drop withdraws the proof"
/// must not observe another test's transient clone. Poison recovered: the
/// guarded value is `()`.
#[cfg(all(test, target_os = "linux"))]
pub(crate) static REGISTRY_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Adds `lock` and prunes entries whose lock has already been dropped.
fn register(lock: &Arc<EngineLock>) {
    // Registry lock poison is recovered, never `unwrap`ed: the guarded value is
    // a list of weakly held handles, so a panicking writer cannot leave it
    // half-updated (ADR-0013 D-2).
    let mut registry = LOCK_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.retain(|entry| entry.strong_count() > 0);
    registry.push(Arc::downgrade(lock));
}

/// Withdraws `lock` **by identity**: only a dead entry or the entry whose
/// `Weak` points at `lock` is removed. A path-keyed removal is forbidden —
/// after `release()` another lock can be registered for the same root, and
/// this lock's drop must not erase that live entry.
fn deregister(lock: &EngineLock) {
    let mut registry = LOCK_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let identity = lock as *const EngineLock;
    registry.retain(|entry| entry.strong_count() > 0 && entry.as_ptr() != identity);
}

// Test-only interleaving seam (issue 46 defect 1): called immediately after
// each successful registry upgrade. The deadlock regression uses it to release
// the last *external* strong reference of a non-matching lock, so the upgraded
// handle is the final one when the non-match is rejected.
#[cfg(all(test, target_os = "linux"))]
type RegistryUpgradeHook = Box<dyn FnOnce(&Arc<EngineLock>)>;

#[cfg(all(test, target_os = "linux"))]
thread_local! {
    static REGISTRY_UPGRADE_HOOK: std::cell::RefCell<Option<RegistryUpgradeHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, target_os = "linux"))]
fn run_registry_upgrade_hook(lock: &Arc<EngineLock>) {
    if let Some(hook) = REGISTRY_UPGRADE_HOOK.with(|cell| cell.borrow_mut().take()) {
        hook(lock);
    }
}

/// The live lock registered for `root`, matched **exactly** (`/root/u` must
/// never satisfy `/root/users`). Poison recovered, never `unwrap`ed.
///
/// Upgrades are collected **under** the guard and selected **after** it drops:
/// a non-matching upgraded `Arc` may be the last strong reference, and its
/// `Drop` → `deregister` re-locks this same non-reentrant mutex. Dropping it
/// inside the guard is the issue-46 defect-1 same-thread deadlock.
pub(crate) fn registered_lock(root: &Path) -> Option<Arc<EngineLock>> {
    let locks: Vec<Arc<EngineLock>> = {
        let registry = LOCK_REGISTRY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut locks = Vec::with_capacity(registry.len());
        for weak in registry.iter() {
            let Some(lock) = weak.upgrade() else { continue };
            #[cfg(all(test, target_os = "linux"))]
            run_registry_upgrade_hook(&lock);
            locks.push(lock);
        }
        locks
    };
    locks.into_iter().find(|lock| lock.storage_root() == root)
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
            let lock = Arc::new(EngineLock {
                storage_path: storage_path.to_path_buf(),
                probe,
                pid,
                released: AtomicBool::new(false),
            });
            register(&lock);
            return Ok(EngineLockOutcome::Acquired(lock));
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
    // Read only by the `#[cfg(target_os = "linux")]` tests below; on any other
    // host this import would be dead and `-D warnings` would fail the build.
    #[cfg(target_os = "linux")]
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
    ///
    /// Only the `#[cfg(target_os = "linux")]` tests below build directories, so
    /// the helper is gated with them: on any other host it would be dead code
    /// and `-D warnings` would fail the build.
    #[cfg(target_os = "linux")]
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

    /// Acquires over a free root (test helper; panics on `Lost`).
    #[cfg(target_os = "linux")]
    fn acquire(root: &Path) -> Arc<EngineLock> {
        match acquire_engine_lock(root, Arc::new(crate::platform::SystemProcessProbe))
            .expect("acquire over a free root")
        {
            EngineLockOutcome::Acquired(lock) => lock,
            EngineLockOutcome::Lost { holder_pid } => {
                panic!("a free temp root must be acquired, holder pid {holder_pid}")
            }
        }
    }

    /// The registry-sensitive tests hold this for their whole body (see
    /// [`REGISTRY_TEST_LOCK`]).
    #[cfg(target_os = "linux")]
    fn registry_test_guard() -> std::sync::MutexGuard<'static, ()> {
        REGISTRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 46.2: the registry publishes the root proof while the lock lives and
    /// withdraws it on the last drop; a different root is never matched.
    ///
    /// Inverted world: skip `register` → the first `current` is `None`; match
    /// roots by prefix → the `other` assertion fails.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_engine_lock_publishes_and_withdraws_the_root_proof() {
        let _registry = registry_test_guard();
        let root = temp_dir("registry-publish");
        let other = temp_dir("registry-publish-other");
        let lock = acquire(&root);

        assert!(
            crate::delete_guard::ExclusiveRoot::current(&root).is_some(),
            "a live lock must be published"
        );
        assert!(
            crate::delete_guard::ExclusiveRoot::current(&other).is_none(),
            "the lookup matches the root exactly"
        );

        drop(lock);
        assert!(
            crate::delete_guard::ExclusiveRoot::current(&root).is_none(),
            "the last drop withdraws the proof"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&other);
    }

    /// 46.2: a drop after a manual release must not remove a **newer**
    /// registration for the same root.
    ///
    /// A releases (idempotent flag), B acquires and registers, A drops: the
    /// proof for B stays visible and B's lock file survives. A path-keyed
    /// deregistration would erase B; a non-idempotent `release` would unlink
    /// B's file.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_engine_lock_drop_does_not_remove_a_newer_registration() {
        let _registry = registry_test_guard();
        let root = temp_dir("registry-newer");
        let a = acquire(&root);
        a.release();
        assert!(
            !engine_lock_path(&root).exists(),
            "A's public early release removes its own file"
        );

        let b = acquire(&root);
        assert!(
            engine_lock_path(&root).exists(),
            "B now owns the root's lock file"
        );

        drop(a);
        assert!(
            crate::delete_guard::ExclusiveRoot::current(&root).is_some(),
            "A's drop must not deregister B's live entry"
        );
        assert!(
            engine_lock_path(&root).exists(),
            "A's drop must not unlink B's lock file"
        );

        drop(b);
        assert!(
            crate::delete_guard::ExclusiveRoot::current(&root).is_none(),
            "B's drop withdraws the proof"
        );
        assert!(!engine_lock_path(&root).exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// The deterministic RED child (issue 46 defect 1).
    ///
    /// Registers A then B. The hook drops A's only external `Arc` while
    /// `registered_lock` holds the registry mutex and has just upgraded A. On
    /// the pre-fix body that upgraded `Arc` becomes the last strong reference,
    /// so its `Drop` runs `deregister` under the same mutex — a same-thread
    /// deadlock. The parent kills the child after its deadline, which is why
    /// this must run in a separate process: a stuck thread cannot be unwound
    /// and would poison every later registry test.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_registry_upgrade_drop_child() {
        if std::env::var_os("MEM_STORE_REGISTRY_DROP_CHILD").is_none() {
            return;
        }
        let a_root = temp_dir("registry-drop-a");
        let b_root = temp_dir("registry-drop-b");
        let a = acquire(&a_root);
        let b = acquire(&b_root);
        REGISTRY_UPGRADE_HOOK.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move |upgraded: &Arc<EngineLock>| {
                assert!(
                    Arc::ptr_eq(upgraded, &a),
                    "the hook fires on A's upgrade (registry order A, B)"
                );
                drop(a);
            }));
        });
        let found = registered_lock(&b_root).expect("B is registered");
        assert!(Arc::ptr_eq(&found, &b));
        drop(found);
        drop(b);
        let _ = fs::remove_dir_all(&a_root);
        let _ = fs::remove_dir_all(&b_root);
    }

    /// The deadlock regression: the registry lookup must not drop a non-matching
    /// upgraded lock under its own non-reentrant guard.
    ///
    /// Inverted world: re-introduce the `filter_map(upgrade).find(..)` body (the
    /// upgraded non-match drops inside the guard) → the child's `Drop` re-locks
    /// `LOCK_REGISTRY` on the same thread and this parent fails after 10 s.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_registry_upgrade_drop_does_not_deadlock_under_the_guard() {
        let mut child = std::process::Command::new(std::env::current_exe().expect("current_exe"))
            .arg("--exact")
            .arg("engine_lock::tests::test_registry_upgrade_drop_child")
            .arg("--test-threads=1")
            .env("MEM_STORE_REGISTRY_DROP_CHILD", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the registry child");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                assert!(status.success(), "the child exited {status}");
                return;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("deadlock: the child did not finish within 10 s");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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
