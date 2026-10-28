//! SQLite file sidecar ops + Windows-lock retry battery (blueprint W1.2
//! steps 3 + 5; part 1 here, the Drop-observation battery in step 5).
//!
//! Port of upstream `src/services/turso/sqlite-handle-release.ts` and the
//! test-utils cleanup helper.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mem_store::platform::{Platform, SleepFn};
use mem_store::sqlite_files::{FILE_LOCK_RETRY_DELAYS_MS, FileLockCode};

// ---------------------------------------------------------------------------
// retry machinery helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LockError(FileLockCode);

impl mem_store::sqlite_files::FileLockError for LockError {
    fn lock_code(&self) -> Option<FileLockCode> {
        Some(self.0)
    }
}

#[derive(Debug)]
struct PlainError;

impl mem_store::sqlite_files::FileLockError for PlainError {
    fn lock_code(&self) -> Option<FileLockCode> {
        None
    }
}

/// Recording test context: counts sleeps, never sleeps.
fn recording_ctx(
    platform: Platform,
    delays: Arc<Mutex<Vec<u64>>>,
) -> mem_store::sqlite_files::RetryContext {
    let sleep: SleepFn = Arc::new(move |duration| {
        delays.lock().unwrap().push(duration.as_millis() as u64);
    });
    mem_store::sqlite_files::RetryContext {
        platform,
        sleep,
        on_retry: None,
    }
}

fn failing() -> Result<(), LockError> {
    Err(LockError(FileLockCode::Busy))
}

// ---------------------------------------------------------------------------
// 1/10
// ---------------------------------------------------------------------------

#[test]
fn test_retries_exactly_max_retries_plus_one_times_on_persistent_lock() {
    let ctx = recording_ctx(Platform::Windows, Arc::new(Mutex::new(Vec::new())));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let result = mem_store::sqlite_files::with_sqlite_file_lock_retry(
        || {
            observed.fetch_add(1, Ordering::SeqCst);
            failing()
        },
        2,
        &ctx,
    );
    assert!(result.is_err(), "persistent lock error must propagate");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "initial attempt + 2 retries"
    );
}

// ---------------------------------------------------------------------------
// 2/10
// ---------------------------------------------------------------------------

#[test]
fn test_runs_once_when_max_retries_is_zero() {
    let ctx = recording_ctx(Platform::Windows, Arc::new(Mutex::new(Vec::new())));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let result = mem_store::sqlite_files::with_sqlite_file_lock_retry(
        || {
            observed.fetch_add(1, Ordering::SeqCst);
            failing()
        },
        0,
        &ctx,
    );
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 3/10
// ---------------------------------------------------------------------------

#[test]
fn test_does_not_retry_when_platform_is_not_windows() {
    let ctx = recording_ctx(Platform::Unix, Arc::new(Mutex::new(Vec::new())));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let result = mem_store::sqlite_files::with_sqlite_file_lock_retry(
        || {
            observed.fetch_add(1, Ordering::SeqCst);
            failing()
        },
        FILE_LOCK_RETRY_DELAYS_MS.len(),
        &ctx,
    );
    assert!(result.is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "non-Windows: no retry at all"
    );
}

// ---------------------------------------------------------------------------
// 4/10
// ---------------------------------------------------------------------------

#[test]
fn test_does_not_retry_non_lock_errors() {
    let ctx = recording_ctx(Platform::Windows, Arc::new(Mutex::new(Vec::new())));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let result: Result<(), PlainError> = mem_store::sqlite_files::with_sqlite_file_lock_retry(
        || {
            observed.fetch_add(1, Ordering::SeqCst);
            Err(PlainError)
        },
        FILE_LOCK_RETRY_DELAYS_MS.len(),
        &ctx,
    );
    assert!(result.is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "code-less errors fail immediately"
    );
}

// ---------------------------------------------------------------------------
// 5/10
// ---------------------------------------------------------------------------

#[test]
fn test_returns_value_when_operation_eventually_succeeds() {
    let ctx = recording_ctx(Platform::Windows, Arc::new(Mutex::new(Vec::new())));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let result = mem_store::sqlite_files::with_sqlite_file_lock_retry(
        || -> Result<u8, LockError> {
            let attempt = observed.fetch_add(1, Ordering::SeqCst);
            if attempt < 3 {
                Err(LockError(FileLockCode::Permission))
            } else {
                Ok(42)
            }
        },
        FILE_LOCK_RETRY_DELAYS_MS.len(),
        &ctx,
    );
    assert_eq!(result.expect("eventual success"), 42);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

// ---------------------------------------------------------------------------
// 6/10
// ---------------------------------------------------------------------------

#[test]
fn test_retry_delays_follow_upstream_table() {
    let delays = Arc::new(Mutex::new(Vec::new()));
    let ctx = recording_ctx(Platform::Windows, delays.clone());
    let _ = mem_store::sqlite_files::with_sqlite_file_lock_retry(failing, 2, &ctx);
    let recorded = delays.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![25, 50],
        "first two entries of the upstream table for max 2"
    );
    assert_eq!(FILE_LOCK_RETRY_DELAYS_MS[0], 25);
    assert_eq!(FILE_LOCK_RETRY_DELAYS_MS[7], 3200);
}

// ---------------------------------------------------------------------------
// 7/10 — rename sidecar moves
// ---------------------------------------------------------------------------

fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn test_rename_moves_database_and_sidecars_and_drops_stale_target_sidecars() {
    let dir = common::temp_dir("sqlite-rename");
    let from = dir.join("old.db");
    let to = dir.join("sub/renamed.db");
    write_file(&from, b"main");
    write_file(&from.with_extension("db-wal"), b"wal");
    write_file(&from.with_extension("db-shm"), b"shm");
    write_file(&from.with_extension("db-tshm"), b"tshm");
    // stale sidecars at the target must not survive the swap
    write_file(&to.with_extension("db-wal"), b"stale");

    let proof = common::exclusive_root(&dir);
    let from_claim = proof.claim(&from).expect("the source is inside the root");
    let to_claim = proof.claim(&to).expect("the target is inside the root");
    mem_store::sqlite_files::rename_sqlite_database(from_claim, to_claim).unwrap();

    assert!(!from.exists(), "main file moved");
    assert_eq!(std::fs::read(&to).unwrap(), b"main");
    for suffix in ["-wal", "-shm", "-tshm"] {
        let moved = PathBuf::from(format!("{}{}", to.display(), suffix));
        let source = PathBuf::from(format!("{}{}", from.display(), suffix));
        assert!(moved.exists(), "{suffix} moved to target");
        assert!(!source.exists(), "{suffix} gone from source");
    }
    assert_eq!(
        std::fs::read(PathBuf::from(format!("{}{}", to.display(), "-wal"))).unwrap(),
        b"wal",
        "no stale -wal survived at the target"
    );
}

// ---------------------------------------------------------------------------
// 8/10 — copy sidecar moves
// ---------------------------------------------------------------------------

#[test]
fn test_copy_moves_database_and_sidecars_and_drops_stale_target_sidecars() {
    let dir = common::temp_dir("sqlite-copy");
    let from = dir.join("backup-src.db");
    let to = dir.join("sub/copy.db");
    write_file(&from, b"main");
    write_file(&from.with_file_name("backup-src.db-wal"), b"wal");
    write_file(&to.with_file_name("copy.db-shm"), b"stale-shm");

    let proof = common::exclusive_root(&dir);
    let to_claim = proof.claim(&to).expect("the target is inside the root");
    mem_store::sqlite_files::copy_sqlite_database(&from, to_claim).unwrap();

    assert!(from.exists(), "copy keeps the source");
    assert_eq!(std::fs::read(&to).unwrap(), b"main");
    let target_wal = PathBuf::from(format!("{}{}", to.display(), "-wal"));
    let target_shm = PathBuf::from(format!("{}{}", to.display(), "-shm"));
    assert_eq!(std::fs::read(&target_wal).unwrap(), b"wal");
    assert!(
        !target_shm.exists(),
        "stale target sidecar with no source counterpart is dropped, not resurrected"
    );
}

// ---------------------------------------------------------------------------
// 9/10 — remove
// ---------------------------------------------------------------------------

#[test]
fn test_remove_deletes_database_and_sidecars() {
    let dir = common::temp_dir("sqlite-remove");
    let main = dir.join("gone.db");
    write_file(&main, b"main");
    for suffix in ["-wal", "-shm", "-tshm"] {
        write_file(
            &PathBuf::from(format!("{}{}", main.display(), suffix)),
            suffix.as_bytes(),
        );
    }
    let proof = common::exclusive_root(&dir);
    let claim = proof.claim(&main).expect("the database is inside the root");
    mem_store::sqlite_files::remove_sqlite_database(claim).expect("a free database is removed");
    assert!(!main.exists());
    for suffix in ["-wal", "-shm", "-tshm"] {
        assert!(!PathBuf::from(format!("{}{}", main.display(), suffix)).exists());
    }
}

// ---------------------------------------------------------------------------
// 10/10 — cleanup helper
// ---------------------------------------------------------------------------

#[test]
fn test_cleanup_helper_rethrows_non_lock_errors() {
    // A NUL byte in the path is a plain io::Error (InvalidInput) — never a
    // lock error — so the helper must propagate it (upstream `rethrow` path).
    let bogus = PathBuf::from("/tmp/mem-store-w12-nul\u{0}-bytes.db");
    let error = common::cleanup_temp_dir(&bogus).expect_err("NUL path cannot be removed");
    assert!(
        error.raw_os_error().is_some() || matches!(error.kind(), std::io::ErrorKind::InvalidInput),
        "non-lock io error propagated: {error}"
    );
}

// ===========================================================================
// Step 5 — handle-release determinism battery (blueprint; extends part 1)
// ===========================================================================
//
// Upstream `collectReleasedSqliteHandles` (libsql 0.5.x GC pass on win32) is
// N-A in Rust: `TursoDb` owns BOTH the `turso::Database` and its
// `Connection`, `Db` exposes no `close()`, and release happens when the last
// handle drops. The battery below proves that determinism: Drop flag flips,
// the file is free for rename/reopen, fds return to baseline, and manager
// close/remove paths serialize correctly.

mod step5 {
    use super::*;

    use mem_store::config::StoreConfig;
    use mem_store::connection::ConnectionManager;
    use mem_store::db::{ConnectFactory, ConnectOptions, DbHandle, TursoConnectFactory, Value};
    use mem_store::platform::Platform;
    use mem_store::sql;

    use std::sync::Arc;

    use crate::common;

    fn step5_config(prefix: &str) -> StoreConfig {
        common::store_config(prefix)
    }

    async fn open_after_drop(config: &StoreConfig, name: &str) -> DbHandle {
        // no GC passes, no timer waits: straight factory reopen
        let opts = ConnectOptions::for_config(config, Platform::current());
        TursoConnectFactory
            .connect(config.storage_path.join(name), opts)
            .await
            .expect("reopen")
    }

    // -----------------------------------------------------------------------
    // 1/5
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_fake_db_drop_flips_flag_when_last_handle_released() {
        let fake = common::FakeDb::new();
        let drop_flag = fake.on_drop.clone();
        let handle: DbHandle = Arc::new(fake);
        let alias: DbHandle = handle.clone();

        drop(alias);
        tokio::task::yield_now().await; // scheduler settle only; not a wait
        assert!(
            !drop_flag.load(Ordering::SeqCst),
            "a cloned handle keeps the db alive"
        );

        drop(handle);
        tokio::task::yield_now().await;
        assert!(
            drop_flag.load(Ordering::SeqCst),
            "releasing the LAST handle flips the drop flag"
        );
    }

    // -----------------------------------------------------------------------
    // 2/5
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_turso_db_drop_releases_file_for_rename_and_reopen() {
        let config = step5_config("release-file");
        let db = open_after_drop(&config, "cycle.db").await;
        db.batch(
            vec![
                mem_store::db::Statement::new(sql::shard_metadata_ddl(), Vec::new()),
                mem_store::db::Statement::new(sql::memories_ddl(2), Vec::new()),
            ],
            mem_store::db::BatchMode::Write,
        )
        .await
        .expect("schema");
        db.batch(
            vec![
                mem_store::db::Statement::new(sql::MEMORY_SHARD_V2_ALTER, Vec::new()),
                mem_store::db::Statement::new(sql::MEMORY_SHARD_V2_INDEX, Vec::new()),
                mem_store::db::Statement::new(sql::MEMORY_SHARD_V2_BACKFILL, Vec::new()),
            ],
            mem_store::db::BatchMode::Write,
        )
        .await
        .expect("v2 chain (INSERT binds session_id)");
        db.execute(
            sql::INSERT_WITHOUT_TAGS,
            vec![
                Value::Text("persisted".to_string()),
                Value::Text("release battery".to_string()),
                Value::Text("[1.0,0.0]".to_string()),
                Value::Text("opencode_project_a1b2c3d4e5f60001".to_string()),
                Value::Null,
                Value::Null,
                Value::Integer(1),
                Value::Integer(1),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ],
        )
        .await
        .expect("seed insert");

        drop(db); // THE release mechanism: no GC pass, no timer
        std::fs::File::open(config.storage_path.join("cycle.db")).expect("file still exists");

        // The file is immediately free for exclusive renames (Windows-shaped
        // invariant; harmless on Unix).
        let moved = config.storage_path.join("cycle-moved.db");
        let cycle_db = config.storage_path.join("cycle.db");
        let proof = common::exclusive_root(&config.storage_path);
        let from_claim = proof
            .claim(&cycle_db)
            .expect("the source is inside the root");
        let to_claim = proof.claim(&moved).expect("the target is inside the root");
        mem_store::sqlite_files::rename_sqlite_database(from_claim, to_claim)
            .expect("file released for rename after drop");

        // Reopen reads the committed row.
        let reopened = open_after_drop(&config, "cycle-moved.db").await;
        let rows = reopened
            .query("SELECT id FROM memories", Vec::new())
            .await
            .expect("reopen query");
        assert_eq!(rows.len(), 1);
    }

    // -----------------------------------------------------------------------
    // 3/5
    // -----------------------------------------------------------------------

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_connection_cycle_returns_fd_count_to_baseline() {
        let config = step5_config("release-fd");
        let manager = ConnectionManager::new(
            config.storage_path.clone(),
            config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory) as Arc<dyn ConnectFactory>,
        );
        let path = config.storage_path.join("fds.db");
        // warm-up cycle (allocator + registry entry points warm up here)
        let warm = manager.get_connection(&path).await.expect("warm-up");
        manager
            .close_connection(&path)
            .await
            .expect("connection closes");
        drop(warm);

        fn fd_count() -> usize {
            std::fs::read_dir("/proc/self/fd")
                .expect("procfs available")
                .count()
        }
        let baseline = fd_count();
        for _ in 0..3 {
            let handle = manager.get_connection(&path).await.expect("cycle open");
            manager
                .close_connection(&path)
                .await
                .expect("connection closes");
            drop(handle);
        }
        let after = fd_count();
        // Tolerance: transient arc/registry bookkeeping fds; the invariant is
        // "no monotonic growth per cycle".
        assert!(after <= baseline + 2, "fds leaked: {baseline} -> {after}");
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_connection_cycle_returns_fd_count_to_baseline() {
        // N-A note (registry): procfs-based fd counting is Linux-only;
        // other platforms cannot observe fd tables portably.
        let config = step5_config("na-fd");
        let manager = ConnectionManager::new(
            config.storage_path.clone(),
            config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory) as Arc<dyn ConnectFactory>,
        );
        let path = config.storage_path.join("na.db");
        let _ = manager.get_connection(&path).await.expect("open");
        manager
            .close_connection(&path)
            .await
            .expect("connection closes");
    }

    // -----------------------------------------------------------------------
    // 4/5
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_manager_close_connection_releases_handle_and_path_recovers() {
        let config = step5_config("release-manager");
        let manager = ConnectionManager::new(
            config.storage_path.clone(),
            config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory) as Arc<dyn ConnectFactory>,
        );
        let path = config.storage_path.join("recover.db");
        let handle = manager.get_connection(&path).await.expect("open");
        assert!(manager.has_open_connection(&path).expect("has open"));
        manager
            .close_connection(&path)
            .await
            .expect("connection closes");
        drop(handle);
        assert!(
            !manager.has_open_connection(&path).expect("has open"),
            "released"
        );
        let reopened = manager.get_connection(&path).await.expect("reopen");
        assert!(manager.has_open_connection(&path).expect("has open"));
        manager
            .close_connection(&path)
            .await
            .expect("connection closes");
        manager
            .close_connection(&path)
            .await
            .expect("connection closes"); // second close: no-op
        drop(reopened);
        assert!(!manager.has_open_connection(&path).expect("has open"));
    }

    // -----------------------------------------------------------------------
    // 5/5
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_close_all_is_serialized_with_in_flight_opens() {
        let config = step5_config("release-close-all");
        let factory = Arc::new(common::FakeConnectFactory::default());
        factory.arm_open().await;
        let manager = ConnectionManager::new(
            config.storage_path.clone(),
            config.clone(),
            Platform::current(),
            factory.clone() as Arc<dyn ConnectFactory>,
        );
        let path = config.storage_path.join("serialized.db");

        let open_task = tokio::spawn({
            let manager = manager.clone();
            let path = path.clone();
            async move { manager.get_connection(&path).await }
        });
        while !factory
            .event_log()
            .iter()
            .any(|event| event == "open0:start")
        {
            tokio::task::yield_now().await;
        }
        let close_task = tokio::spawn({
            let manager = manager.clone();
            async move { manager.close_all().await.expect("close all") }
        });
        factory.release_next();
        let handle = open_task.await.expect("open join").expect("open");
        close_task.await.expect("close_all join");
        drop(handle);

        assert!(
            !manager.has_open_connection(&path).expect("has open"),
            "close_all drained the manager"
        );
        manager.get_connection(&path).await.expect("fresh open");
        assert_eq!(
            factory.opens.load(Ordering::SeqCst),
            2,
            "exactly one more open after close_all"
        );
    }
}
