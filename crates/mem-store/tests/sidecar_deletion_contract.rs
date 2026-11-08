//! The sidecar-deletion contract (issue 46).
//!
//! G1/G2 measurements plus the contract oracles: a deletion is legal only when
//! this process holds the storage root's engine lock and the database-file probe
//! answers `Free`/`Missing`; the schema runner never unlinks a file it has open;
//! a recovery path consults the operation lock before its first unlink.
//!
//! Step 1 owns the G1 measurement only: the pre-fix sequence that loses a write
//! through a still-open handle. Step 5 declares it replaced by
//! `test_self_holder_restore_is_pending_and_the_runner_does_not_unlink`.

mod common;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use mem_store::config::StoreConfig;
use mem_store::db::{
    BatchMode, ConnectFactory, ConnectOptions, Statement, Value, turso_connect_factory,
};
use mem_store::error::StoreError;
use mem_store::operation_lock::OPERATION_LOCK_FILE;
use mem_store::platform::Platform;
use mem_store::schema_migrations::{
    MigrationOptions, SchemaMigration, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::shard_path_migration::{PathMigrateSwapState, ShardMove};
use mem_store::sql;

/// The v1-era insert shape (the 16 pre-v2 columns), matching
/// `schema_migrations_runner::PRE_V2_INSERT`.
const PRE_V2_INSERT: &str = r#"
  INSERT INTO memories (
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
    metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url
  ) VALUES (?, ?, vector32(?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

fn migration(version: i64, text: &str) -> SchemaMigration {
    SchemaMigration {
        version,
        description: "test",
        statements: vec![Statement::new(text, Vec::new())],
    }
}

fn pre_v2_row(id: &str, content: &str) -> Vec<Value> {
    vec![
        Value::Text(id.to_string()),
        Value::Text(content.to_string()),
        Value::Text("[1.0,0.0]".to_string()),
        Value::Null,
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
    ]
}

/// Seeds the v1 schema + one row through an already-open handle.
async fn seed_v1(db: &dyn mem_store::db::Db) {
    let v1 = &memory_shard_migrations(2)[0];
    db.batch(v1.statements.clone(), BatchMode::Write)
        .await
        .expect("v1 schema");
    db.execute(&sql::set_user_version_sql(1), Vec::new())
        .await
        .expect("seed version");
    db.execute(PRE_V2_INSERT, pre_v2_row("row1", "keep"))
        .await
        .expect("seed row");
}

fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).expect("stat").ino()
}

/// The four files a refusal must leave byte-identical (`None` = absent).
fn file_snapshot(db_path: &Path) -> Vec<(&'static str, Option<Vec<u8>>)> {
    ["", "-wal", "-shm", "-tshm"]
        .into_iter()
        .map(|suffix| (suffix, std::fs::read(sidecar(db_path, suffix)).ok()))
        .collect()
}

async fn user_version(db: &dyn mem_store::db::Db) -> i64 {
    mem_store::db::query_one(db, sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version probe")
        .and_then(|row| row.i64("user_version"))
        .unwrap_or(-1)
}

/// 46.4 — the runner returns the pending; it does not unlink the file it holds.
///
/// This test **replaces** the Step-1 measurement
/// `test_g1_self_holder_write_loss_is_silent`, whose pre-fix observation was:
///
/// ```text
/// [G1 OBSERVED] write_error=none user_version_after_reopen=1 row_ids_after_reopen=["row1"] restored_inode=2082964
/// ```
///
/// After the fix the pending is returned instead: the main `db_path` keeps its
/// inode (nothing was unlinked; deliberately **no** probe-detection claim, R2),
/// and a write through the still-open handle is visible on reopen — the exact
/// inversion of the lost write above.
#[cfg(unix)]
#[tokio::test]
async fn test_self_holder_restore_is_pending_and_the_runner_does_not_unlink() {
    let config = common::store_config("self-holder-pending");
    let (_config, connections, _shards) = common::real_components(config.clone()).await;
    let db_path = config.storage_path.join("shard.db");
    let db = connections
        .get_connection(&db_path)
        .await
        .expect("open through the manager");
    seed_v1(db.as_ref()).await;
    let inode_before = inode(&db_path);
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "test",
        max_known: 2,
    };
    let proof = common::exclusive_root(&config.storage_path);

    let error = apply_schema_migrations(
        db.as_ref(),
        &[migration(2, "SELECTX certainly not sql")],
        &options,
        Some(&proof),
    )
    .await
    .expect_err("the failing v2 migration must return the pending");
    let pending = match error {
        StoreError::MigrationRestorePending(pending) => pending,
        other => panic!("expected MigrationRestorePending, got {other:?}"),
    };
    assert!(
        pending.backup_path().exists(),
        "the backup must be kept until the caller settles"
    );
    assert_eq!(
        inode(&db_path),
        inode_before,
        "the runner must not unlink or replace the file it holds open"
    );

    // The G1 inversion: the write through the still-open handle survives.
    db.execute(PRE_V2_INSERT, pre_v2_row("row2", "kept-write"))
        .await
        .expect("write through the still-open handle");
    drop(db);
    let reopened = common::open_real_db(&config, "shard.db")
        .await
        .expect("reopen");
    let ids: Vec<String> = reopened
        .query("SELECT id FROM memories", Vec::new())
        .await
        .expect("rows probe")
        .iter()
        .filter_map(|row| row.text("id"))
        .collect();
    println!(
        "[G1-INVERSION] row_ids_after_reopen={ids:?} inode_unchanged={}",
        inode(&db_path) == inode_before
    );
    assert!(
        ids.iter().any(|id| id == "row2"),
        "the write through the still-open handle must now be visible; ids: {ids:?}"
    );
    drop(reopened);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// 46.4 — `db_path: Some` + `root: None` refuses **before the checkpoint and
/// the copy**.
///
/// Fixture order (the Plan Gate's N2): checkpoint once, then a final write that
/// leaves a non-empty `-wal`, then the baseline. Moving the refusal after the
/// checkpoint changes the `-wal` bytes, so the mutation cannot pass silently.
/// The oracle is byte-for-byte file equality — strictly stronger than the
/// blueprint's SHA-256, and this workspace has no hash dependency to add.
#[cfg(unix)]
#[tokio::test]
async fn test_destructive_migration_without_root_proof_refuses_before_the_copy() {
    let config = common::store_config("no-root-refusal");
    let (_config, connections, _shards) = common::real_components(config.clone()).await;
    let db_path = config.storage_path.join("shard.db");
    let db = connections
        .get_connection(&db_path)
        .await
        .expect("open through the manager");
    seed_v1(db.as_ref()).await;
    // Quiesce once...
    db.query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await
        .expect("quiescing checkpoint");
    // ...then a final write leaves coordination bytes behind...
    db.execute(PRE_V2_INSERT, pre_v2_row("row2", "wal-payload"))
        .await
        .expect("final write");
    let wal_len = std::fs::metadata(sidecar(&db_path, "-wal"))
        .expect("the final write leaves a -wal")
        .len();
    assert!(
        wal_len > 0,
        "the fixture must leave a non-empty -wal, or the checkpoint mutation proves nothing"
    );
    // ...and only then the baseline.
    let baseline = file_snapshot(&db_path);
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "test",
        max_known: mem_store::schema_migrations::MEMORY_SHARD_MAX_KNOWN,
    };

    let error = apply_schema_migrations(db.as_ref(), &memory_shard_migrations(2), &options, None)
        .await
        .expect_err("a destructive upgrade needs the root proof");
    assert!(
        matches!(error, StoreError::NoExclusiveRoot { .. }),
        "the refusal must be the typed no-root error, got {error:?}"
    );

    let backups: Vec<PathBuf> = std::fs::read_dir(&config.storage_path)
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.to_string_lossy().ends_with(".bak"))
        .collect();
    assert!(backups.is_empty(), "no backup may be created: {backups:?}");
    assert_eq!(
        file_snapshot(&db_path),
        baseline,
        "db, -wal, -shm and -tshm must be byte-identical: the refusal precedes its own checkpoint"
    );
    assert_eq!(user_version(db.as_ref()).await, 1, "user_version unchanged");
    let rows = db
        .query("SELECT id FROM memories", Vec::new())
        .await
        .expect("rows probe");
    assert_eq!(
        rows.len(),
        2,
        "row count unchanged (row1 + the final write)"
    );

    drop(db);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// 46.4 — the v6 backup site is reachable on a v5 database and refuses with no
/// proof, before its copy.
#[cfg(unix)]
#[tokio::test]
async fn test_v6_backup_without_root_proof_refuses_before_its_copy() {
    const DIMS: u32 = 2;
    let config = common::store_config("v6-no-root");
    let (_config, connections, _shards) = common::real_components(config.clone()).await;
    let db_path = config.storage_path.join("episodic.db");
    let db = connections
        .get_connection(&db_path)
        .await
        .expect("open through the manager");
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "episodic",
        max_known: mem_store::schema_migrations::EPISODIC_MAX_KNOWN,
    };
    // A genuine v5 database: a version-0 apply takes no backup, so no proof.
    let version = apply_schema_migrations(
        db.as_ref(),
        &mem_store::schema_migrations::episodic_migrations(DIMS),
        &options,
        None,
    )
    .await
    .expect("apply v1..v5");
    assert_eq!(version, 5);
    let baseline = file_snapshot(&db_path);

    let error =
        mem_store::schema_migrations::apply_episodic_schema(db.as_ref(), DIMS, &options, None)
            .await
            .expect_err("the v6 backup needs a proof");
    assert!(
        matches!(error, StoreError::NoExclusiveRoot { .. }),
        "the refusal must be the typed no-root error, got {error:?}"
    );
    let backups: Vec<PathBuf> = std::fs::read_dir(&config.storage_path)
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.to_string_lossy().ends_with(".bak"))
        .collect();
    assert!(backups.is_empty(), "the v6 copy must not run: {backups:?}");
    assert_eq!(
        file_snapshot(&db_path),
        baseline,
        "the v6 refusal must leave every file byte-identical"
    );
    assert_eq!(user_version(db.as_ref()).await, 5, "still at v5");

    drop(db);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// 46.4 — a restore with no live engine lock refuses and keeps the backup.
///
/// The memoising `common::exclusive_root` keeps its lock alive and can never
/// observe `None`, so this test acquires a manual `Arc<EngineLock>` + proof,
/// takes the pending with it, then drops both before `restore`.
#[cfg(unix)]
#[tokio::test]
async fn test_restore_without_an_engine_lock_refuses_and_keeps_the_backup() {
    let config = common::store_config("restore-no-lock");
    let (_config, connections, _shards) = common::real_components(config.clone()).await;
    let db_path = config.storage_path.join("shard.db");
    let db = connections
        .get_connection(&db_path)
        .await
        .expect("open through the manager");
    seed_v1(db.as_ref()).await;
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "test",
        max_known: 2,
    };

    let lock = match mem_store::engine_lock::acquire_engine_lock(
        &config.storage_path,
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .expect("acquire over a free root")
    {
        mem_store::engine_lock::EngineLockOutcome::Acquired(lock) => lock,
        mem_store::engine_lock::EngineLockOutcome::Lost { holder_pid } => {
            panic!("a fresh temp root must be free, holder pid {holder_pid}")
        }
    };
    let proof = mem_store::delete_guard::ExclusiveRoot::new(lock);
    let error = apply_schema_migrations(
        db.as_ref(),
        &[migration(2, "SELECTX certainly not sql")],
        &options,
        Some(&proof),
    )
    .await
    .expect_err("the failing v2 migration must return the pending");
    let pending = match error {
        StoreError::MigrationRestorePending(pending) => pending,
        other => panic!("expected MigrationRestorePending, got {other:?}"),
    };
    let backup = pending.backup_path().to_path_buf();
    assert!(backup.exists(), "the pending names a live backup");
    drop(proof);
    let inode_before = inode(&db_path);

    let restore_error = pending
        .restore(&connections, db)
        .await
        .expect_err("no live proof must refuse the restore");
    assert!(
        matches!(restore_error, StoreError::NoExclusiveRoot { .. }),
        "the refusal must be the typed no-root error, got {restore_error:?}"
    );
    assert!(backup.exists(), "the refusal keeps the backup it names");
    assert_eq!(
        inode(&db_path),
        inode_before,
        "the refusal must precede the first unlink"
    );
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// The refusal: the probe runs before the first unlink
// ---------------------------------------------------------------------------

/// 46.6/46.2: a live legacy holder refuses the unlink **before any unlink**.
///
/// The holder is an in-process **OFD** write lock (`F_OFD_SETLK`, Linux): it
/// belongs to the holder's open file description, so the probe's open/close
/// cannot release it. A POSIX (`F_SETLK`) holder would be released by the
/// probe's own `close()` (open_guard's measured `t2 S4/S6/S7`), making the
/// fixture vacuous after its first probe. Step 9 adds the cross-process arm.
/// Every file must survive, byte-identical.
///
/// Inverted world: delete the probe from `remove_sqlite_database` → the call
/// unlinks and the "still present" assertions fail.
#[cfg(target_os = "linux")]
#[test]
fn test_remove_refuses_a_legacy_holder_before_any_unlink() {
    let dir = common::temp_dir("remove-refused");
    let proof = common::exclusive_root(&dir);
    let db = dir.join("held.db");
    std::fs::write(&db, b"main").expect("seed the database");
    for suffix in ["-wal", "-shm", "-tshm"] {
        std::fs::write(sidecar(&db, suffix), suffix.as_bytes()).expect("seed a sidecar");
    }

    let holder = OfdHolder::acquire(&db);
    assert_eq!(
        mem_store::open_guard::probe_database_file(&db),
        mem_store::open_guard::ProbeOutcome::Held,
        "the fixture must read Held before the contract is exercised"
    );

    let claim = proof.claim(&db).expect("the database is inside the root");
    let error = mem_store::sqlite_files::remove_sqlite_database(claim)
        .expect_err("a live legacy holder must refuse the unlink");
    assert!(
        matches!(error, StoreError::ExclusivityRefused { .. }),
        "the refusal must be the typed exclusivity error, got {error:?}"
    );

    // Release the holder only after the assertions; the files must be untouched
    // by then.
    drop(holder);

    for suffix in ["", "-wal", "-shm", "-tshm"] {
        assert!(
            sidecar(&db, suffix).exists(),
            "the refused removal must leave {suffix:?} present"
        );
    }
    assert_eq!(std::fs::read(&db).expect("db bytes"), b"main");
    let _ = common::cleanup_temp_dir(&dir);
}

/// An in-process **OFD** write lock (`F_OFD_SETLK`, Linux) on byte 0 of one
/// file. It belongs to this open file description, so the probe's own
/// `open`/`close` cannot release it (unlike a POSIX `F_SETLK` holder, which
/// `close()` drops — open_guard's measured `t2 S4/S6/S7`). The probe's OFD
/// attempt conflicts with it, so it reads `Held`.
#[cfg(target_os = "linux")]
struct OfdHolder {
    file: std::fs::File,
}

#[cfg(target_os = "linux")]
impl OfdHolder {
    fn acquire(path: &Path) -> Self {
        use std::os::unix::io::AsRawFd;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("open the holder");
        // SAFETY: `libc::flock` is plain old data; all-zero is a valid bit
        // pattern and every meaningful field is overwritten below.
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        lock.l_start = 0;
        lock.l_len = 1;
        lock.l_pid = 0;
        // SAFETY: `file` owns the fd for the whole call and `lock` outlives it.
        let taken = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &lock) };
        assert_eq!(
            taken,
            0,
            "the fixture's OFD write lock must be taken: {}",
            std::io::Error::last_os_error()
        );
        Self { file }
    }
}

#[cfg(target_os = "linux")]
impl Drop for OfdHolder {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;

        // SAFETY: same owned fd; releasing is best-effort on the way out.
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_UNLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        lock.l_start = 0;
        lock.l_len = 1;
        let _ = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_OFD_SETLK, &lock) };
    }
}

/// 46.3: a copy claims **only** the destination — the source is a documented
/// snapshot.
///
/// The source is held by a live OFD lock, so a copy that (wrongly) probed the
/// source would answer `Held` and refuse. It succeeds, copies main + sidecars,
/// and leaves the source's own bytes untouched.
///
/// Inverted world: add a source probe to `copy_sqlite_database` → this test
/// fails with `ExclusivityRefused`.
#[cfg(target_os = "linux")]
#[test]
fn test_copy_allows_a_live_source_but_claims_only_the_destination() {
    let dir = common::temp_dir("copy-live-source");
    let proof = common::exclusive_root(&dir);
    let source = dir.join("source.db");
    let destination = dir.join("snapshot.db");
    std::fs::write(&source, b"source-main").expect("seed the source");
    std::fs::write(sidecar(&source, "-wal"), b"source-wal").expect("seed the source wal");
    let holder = OfdHolder::acquire(&source);

    let destination_claim = proof.claim(&destination).expect("inside the root");
    mem_store::sqlite_files::copy_sqlite_database(&source, destination_claim)
        .expect("a live source does not block a snapshot copy");

    assert_eq!(
        std::fs::read(&source).expect("source bytes"),
        b"source-main"
    );
    assert_eq!(
        std::fs::read(sidecar(&source, "-wal")).expect("source wal"),
        b"source-wal"
    );
    assert_eq!(
        std::fs::read(&destination).expect("destination bytes"),
        b"source-main"
    );
    assert_eq!(
        std::fs::read(sidecar(&destination, "-wal")).expect("destination wal"),
        b"source-wal"
    );

    drop(holder);
    let _ = common::cleanup_temp_dir(&dir);
}

/// 46.3: a held destination refuses **before** its stale sidecars are dropped.
///
/// Inverted world: drop the `to` probe from `copy_sqlite_database` → the stale
/// destination `-wal` is deleted before the copy and this fails.
#[cfg(target_os = "linux")]
#[test]
fn test_copy_refuses_when_the_destination_is_held() {
    let dir = common::temp_dir("copy-held-destination");
    let proof = common::exclusive_root(&dir);
    let source = dir.join("source.db");
    let destination = dir.join("held.db");
    std::fs::write(&source, b"source-main").expect("seed the source");
    std::fs::write(&destination, b"destination-main").expect("seed the destination");
    std::fs::write(sidecar(&destination, "-wal"), b"stale-wal").expect("seed a stale sidecar");
    let holder = OfdHolder::acquire(&destination);

    let destination_claim = proof.claim(&destination).expect("inside the root");
    let error = mem_store::sqlite_files::copy_sqlite_database(&source, destination_claim)
        .expect_err("a held destination must refuse the copy");
    assert!(
        matches!(error, StoreError::ExclusivityRefused { .. }),
        "the refusal must be the typed exclusivity error, got {error:?}"
    );
    assert_eq!(
        std::fs::read(&destination).expect("destination bytes"),
        b"destination-main",
        "the refused copy must leave the destination's main file untouched"
    );
    assert!(
        sidecar(&destination, "-wal").exists(),
        "the refused copy must not have dropped the destination's stale sidecars"
    );

    drop(holder);
    let _ = common::cleanup_temp_dir(&dir);
}

/// 46.3: a rename probes **both** ends before the main file moves.
///
/// The source is held, so the refusal must leave the main file and every
/// source sidecar where they were and the destination absent.
///
/// Inverted world: drop the `from` probe from `rename_sqlite_database` → the
/// main file moves and the absence assertion fails.
#[cfg(target_os = "linux")]
#[test]
fn test_rename_refuses_and_changes_nothing_when_an_end_is_held() {
    let dir = common::temp_dir("rename-held-end");
    let proof = common::exclusive_root(&dir);
    let from = dir.join("from.db");
    let to = dir.join("to.db");
    std::fs::write(&from, b"from-main").expect("seed the source");
    std::fs::write(sidecar(&from, "-wal"), b"from-wal").expect("seed the source wal");
    let holder = OfdHolder::acquire(&from);

    let from_claim = proof.claim(&from).expect("inside the root");
    let to_claim = proof.claim(&to).expect("inside the root");
    let error = mem_store::sqlite_files::rename_sqlite_database(from_claim, to_claim)
        .expect_err("a held rename end must refuse");
    assert!(
        matches!(error, StoreError::ExclusivityRefused { .. }),
        "the refusal must be the typed exclusivity error, got {error:?}"
    );

    assert!(from.exists(), "the refused rename must not move the source");
    assert_eq!(std::fs::read(&from).expect("source bytes"), b"from-main");
    assert!(
        sidecar(&from, "-wal").exists(),
        "the refused rename must not move the source sidecars"
    );
    assert!(
        !to.exists(),
        "the refused rename must not create the target"
    );

    drop(holder);
    let _ = common::cleanup_temp_dir(&dir);
}

// ---------------------------------------------------------------------------
// The child harness: this test binary re-executed as a database holder
// ---------------------------------------------------------------------------

/// `<path><suffix>` — the turso sidecar spelling.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

/// How long any single handshake step may take (never an unbounded read: a
/// hung child would hold the cross-wave cargo serialiser's descriptor).
const HOLDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The child's config: the storage root is the database's parent, because the
/// child receives only `MEM_STORE_TSHM_HOLDER_DB`.
#[cfg(unix)]
fn child_config(db_path: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: db_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        embedding_dimensions: 2,
        embedding_model: "holder-child".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: std::env::var("MEM_STORE_TSHM_HOLDER_LEGACY").as_deref() == Ok("1"),
    }
}

/// The fixture child. A normal `#[test]` (so the parent can re-exec it with
/// `--exact test_tshm_holder_child`); with no `MEM_STORE_TSHM_HOLDER_DB` it is
/// a no-op in the ordinary suite.
///
/// Protocol on stdout: `READY <pid>`, then one line per stdin command —
/// `ino` → `TSHM_INO <dev>:<ino>` and `write` → `AFTER <ok|err …>`. `exit`
/// ends the child.
#[cfg(unix)]
#[test]
fn test_tshm_holder_child() {
    use std::os::unix::fs::MetadataExt;

    let Ok(db_path) = std::env::var("MEM_STORE_TSHM_HOLDER_DB") else {
        return;
    };
    let db_path = PathBuf::from(db_path);
    let legacy = std::env::var("MEM_STORE_TSHM_HOLDER_LEGACY").as_deref() == Ok("1");
    let config = child_config(&db_path);

    /// The fixture's handle: the shipping seam for the post-drop (legacy) child,
    /// and a **raw flag-on builder** for the pre-drop one. Production no longer
    /// builds a multiprocess option set (the guard refuses it by construction), so
    /// the only way to hold a `-tshm` — the file G2's split is about — is to speak
    /// to turso directly.
    enum HolderHandle {
        Guarded(mem_store::db::DbHandle),
        PreDrop(turso::Connection),
    }

    impl HolderHandle {
        async fn execute(&self, sql: &str) -> Result<(), String> {
            match self {
                Self::Guarded(db) => db
                    .execute(sql, Vec::new())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.message()),
                Self::PreDrop(connection) => connection
                    .execute(sql, ())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            }
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    runtime.block_on(async {
        let db = if legacy {
            HolderHandle::Guarded(
                turso_connect_factory()
                    .connect(
                        db_path.clone(),
                        ConnectOptions::for_config(&config, Platform::current()),
                    )
                    .await
                    .expect("the child's database opens"),
            )
        } else {
            let database = turso::Builder::new_local(&db_path.to_string_lossy())
                .experimental_multiprocess_wal(true)
                .build()
                .await
                .expect("the pre-drop child's database opens");
            HolderHandle::PreDrop(database.connect().expect("the pre-drop connection"))
        };
        db.execute("CREATE TABLE IF NOT EXISTS holder_rows (id INTEGER PRIMARY KEY, v TEXT)")
            .await
            .expect("the child creates its table");
        db.execute("INSERT INTO holder_rows (v) VALUES ('child-before')")
            .await
            .expect("the child's first write");
        println!("READY {}", std::process::id());
        std::io::stdout().flush().expect("flush READY");

        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            match line.trim() {
                "ino" => match std::fs::metadata(sidecar(&db_path, "-tshm")) {
                    Ok(meta) => println!("TSHM_INO {}:{}", meta.dev(), meta.ino()),
                    Err(_) => println!("TSHM_INO missing"),
                },
                "write" => {
                    let id = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_nanos())
                        .unwrap_or(0);
                    let sql = format!("INSERT INTO holder_rows (v) VALUES ('child-after-{id}')");
                    match db.execute(&sql).await {
                        Ok(()) => println!("AFTER ok"),
                        Err(error) => println!("AFTER err {error}"),
                    }
                }
                "exit" => break,
                _ => {}
            }
            std::io::stdout().flush().expect("flush the protocol line");
        }
        drop(db);
    });
}

/// A spawned child holding one database, with a bounded line protocol.
#[cfg(unix)]
struct TshmHolder {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

#[cfg(unix)]
impl TshmHolder {
    fn spawn(db_path: &Path, legacy: bool) -> Self {
        let mut command = Command::new(std::env::current_exe().expect("current_exe"));
        command
            .arg("--exact")
            .arg("test_tshm_holder_child")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("MEM_STORE_TSHM_HOLDER_DB", db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if legacy {
            command
                .env("MEM_STORE_TSHM_HOLDER_LEGACY", "1")
                .env("OPENCODE_MEM_ALLOW_UNGUARDED_OPEN", "1");
        }
        let mut child = command.spawn().expect("spawn the holder child");
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped child stdout");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let holder = Self {
            child: Some(child),
            stdin,
            lines,
        };
        let ready = holder.expect_line("READY ");
        assert_eq!(
            ready.trim().parse::<u32>().ok(),
            Some(pid),
            "READY must name this child's pid: {ready:?}"
        );
        holder
    }

    /// Bounded wait for a line containing `marker`; returns the text after it.
    fn expect_line(&self, marker: &str) -> String {
        let mut seen: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + HOLDER_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some((_, rest)) = line.split_once(marker) {
                        return rest.trim().to_string();
                    }
                    seen.push(line);
                }
                Err(error) => panic!(
                    "the holder child did not print `{marker}` within {HOLDER_TIMEOUT:?} \
                     ({error}); output so far: {seen:?}"
                ),
            }
        }
    }

    fn send(&mut self, command: &str) {
        let stdin = self.stdin.as_mut().expect("child stdin still open");
        stdin
            .write_all(format!("{command}\n").as_bytes())
            .and_then(|()| stdin.flush())
            .expect("write to the holder child");
    }

    fn ino(&mut self) -> String {
        self.send("ino");
        self.expect_line("TSHM_INO ")
    }

    fn write_more(&mut self) -> String {
        self.send("write");
        self.expect_line("AFTER ")
    }

    fn exit(mut self) {
        self.send("exit");
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
impl Drop for TshmHolder {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// G2 — the split is silent: a multiprocess child's write through an unlinked
/// handle is acknowledged, and the next opener gets a fresh `-tshm`.
///
/// This is a **positive control**, not a claim about our code: the test itself
/// performs the raw unlink while the child holds the files, which is exactly
/// the failure the contract exists to prevent. It asserts the split is silent
/// on both sides (the child's write succeeds; the next opener's `-tshm` is a
/// different identity), so a later refusal test cannot pass vacuously.
#[cfg(unix)]
#[tokio::test]
async fn test_g2_positive_control_tshm_split_is_silent() {
    use std::os::unix::fs::MetadataExt;

    let config = common::store_config("g2-tshm-split");
    let db_path = config.storage_path.join("g2.db");
    {
        let seed = common::open_real_db(&config, "g2.db")
            .await
            .expect("seed open");
        seed.execute(
            "CREATE TABLE IF NOT EXISTS holder_rows (id INTEGER PRIMARY KEY, v TEXT)",
            Vec::new(),
        )
        .await
        .expect("seed table");
        drop(seed);
    }

    let mut holder = TshmHolder::spawn(&db_path, false);
    let before = holder.ino();
    let before_meta = std::fs::metadata(sidecar(&db_path, "-tshm"))
        .expect("the live multiprocess child has a -tshm at the path");
    assert_eq!(
        before,
        format!("{}:{}", before_meta.dev(), before_meta.ino()),
        "the child's view of -tshm must be the path's file"
    );
    let probe = mem_store::open_guard::probe_database_file(&db_path);
    println!("[G2] probe under a live multiprocess child = {probe:?}");

    // The split itself: raw unlinks, performed by the test, while the child
    // still holds every one of these files.
    std::fs::remove_file(&db_path).expect("raw unlink the database");
    for suffix in ["-wal", "-shm", "-tshm"] {
        let _ = std::fs::remove_file(sidecar(&db_path, suffix));
    }
    for suffix in ["", "-wal", "-shm", "-tshm"] {
        assert!(
            !sidecar(&db_path, suffix).exists(),
            "the raw unlink must remove {suffix:?} from the path"
        );
    }

    let after = holder.write_more();
    assert_eq!(
        after, "ok",
        "the child's write through its unlinked handle is still acknowledged"
    );

    let reopened = common::open_real_db(&config, "g2.db")
        .await
        .expect("the next opener creates a fresh database");
    reopened
        .execute(
            "CREATE TABLE IF NOT EXISTS holder_rows (id INTEGER PRIMARY KEY, v TEXT)",
            Vec::new(),
        )
        .await
        .expect("the next opener writes");
    // Post-drop (issue 43 W6a) the next opener is the legacy (production) mode, so
    // it never recreates a `-tshm`: the split leaves no shared coordination state at
    // all, and the child's identity is unrecoverable. Pre-drop this half asserted a
    // *fresh* `-tshm` identity; the observable that survives the mode change is the
    // absence, which is exactly what makes the split total.
    println!(
        "[G2] TSHM_INO before={before} (len {}) AFTER=ok; the post-drop opener leaves no \
         `-tshm` at the path",
        before_meta.len()
    );
    assert!(
        !sidecar(&db_path, "-tshm").exists(),
        "the post-drop opener must not recreate a `-tshm`; a fresh file would mean the raw \
         unlink did not happen and this control is vacuous"
    );

    drop(reopened);
    holder.exit();
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// 46.5 — the two recovery locks
// ---------------------------------------------------------------------------

/// The recovery consults the operation lock **before its first unlink**.
///
/// The seeded holder's name (`"test-holder"`) is what the refusal must report:
/// per M27 the field comes from the holder's record, so asserting our own
/// operation name would be circular. The literal `"path-migrate-recovery"` is
/// pinned by the rg gate, not by this test.
#[cfg(unix)]
#[tokio::test]
async fn test_path_migrate_recovery_refuses_while_the_operation_lock_is_held() {
    let config = common::store_config("recovery-lock");
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn mem_store::shard_manager::ShardRegistry> = Arc::new(shards);
    std::fs::create_dir_all(storage.join("projects")).expect("projects dir");

    let old_path = storage.join("projects/old.db");
    let new_path = storage.join("projects/new.db");
    let staged = storage.join("projects/staged.tmp");
    let backup = storage.join("projects/backup.bak");
    std::fs::write(&old_path, b"old").expect("seed old");
    std::fs::write(&new_path, b"new").expect("seed new");
    let journal = common::write_swap_fixture(
        &storage,
        &PathMigrateSwapState {
            operation: "path-migrate".to_string(),
            old_hash: "a1b2c3d4e5f60001".to_string(),
            new_hash: "ffffffffffffffff".to_string(),
            moves: vec![ShardMove {
                shard_id: 1,
                shard_index: 0,
                old_path: old_path.clone(),
                new_path: new_path.clone(),
                staged_path: staged.clone(),
                backup_path: backup.clone(),
                is_active: true,
                vector_count: 1,
                old_container_tag: "opencode_project_a1b2c3d4e5f60001".to_string(),
            }],
            archived_targets: Vec::new(),
        },
    );
    let bytes_before = [
        std::fs::read(&old_path).expect("old bytes"),
        std::fs::read(&new_path).expect("new bytes"),
        std::fs::read(&journal).expect("journal bytes"),
    ];
    common::write_lock_state(
        &storage,
        OPERATION_LOCK_FILE,
        std::process::id(),
        Some("test-holder"),
    );

    let error = mem_store::shard_path_migration::recover_interrupted_swap(
        &storage,
        &connections,
        registry.as_ref(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .await
    .expect_err("a live operation lock must refuse the recovery");
    match error {
        StoreError::MigrationLocked { operation, .. } => assert_eq!(
            operation.as_deref(),
            Some("test-holder"),
            "the refusal reports the HOLDER's operation name"
        ),
        other => panic!("expected MigrationLocked, got {other:?}"),
    }

    assert_eq!(
        bytes_before,
        [
            std::fs::read(&old_path).expect("old bytes"),
            std::fs::read(&new_path).expect("new bytes"),
            std::fs::read(&journal).expect("journal bytes"),
        ],
        "the refusal precedes the first unlink: every file is byte-identical"
    );
    assert!(!staged.exists(), "no staged file was created");
    assert!(!backup.exists(), "no backup was moved");
    let _ = common::cleanup_temp_dir(&storage);
}

/// The legacy re-embed recovery consults the operation lock before its first
/// unlink. The seeded holder is the realistic conflicting `"path-migrate"`.
#[cfg(unix)]
#[tokio::test]
async fn test_legacy_reembed_recovery_refuses_while_the_operation_lock_is_held() {
    let config = common::store_config("legacy-recovery-lock");
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);
    std::fs::create_dir_all(storage.join("projects")).expect("projects dir");

    let db_path = storage.join("projects/project_a1b2c3d4e5f60001_shard_0.db");
    let staged = PathBuf::from(format!("{}.reembed-fixture.tmp", db_path.display()));
    let backup = PathBuf::from(format!("{}.pre-reembed-fixture.bak", db_path.display()));
    std::fs::write(&db_path, b"active").expect("seed active");
    let state_path = common::write_reembed_swap_state(&db_path, &staged, &backup);
    let bytes_before = [
        std::fs::read(&db_path).expect("active bytes"),
        std::fs::read(&state_path).expect("journal bytes"),
    ];
    common::write_lock_state(
        &storage,
        OPERATION_LOCK_FILE,
        std::process::id(),
        Some("path-migrate"),
    );

    let sleeper = common::RecordingSleeper::new();
    let sleep = sleeper.as_sleep_fn();
    let error = mem_store::legacy_migrator::run_legacy_turso_migration(
        &storage,
        &shards,
        &connections,
        &sleep,
    )
    .await
    .expect_err("a live operation lock must refuse the re-embed recovery");
    match error {
        StoreError::MigrationLocked { operation, .. } => assert_eq!(
            operation.as_deref(),
            Some("path-migrate"),
            "the refusal reports the HOLDER's operation name"
        ),
        other => panic!("expected MigrationLocked, got {other:?}"),
    }
    assert_eq!(
        bytes_before,
        [
            std::fs::read(&db_path).expect("active bytes"),
            std::fs::read(&state_path).expect("journal bytes"),
        ],
        "the refusal precedes the first unlink: every file is byte-identical"
    );
    assert!(!staged.exists(), "no staged replacement was created");
    assert!(!backup.exists(), "no backup was moved");
    let _ = common::cleanup_temp_dir(&storage);
}

/// The recovery lock is released before `run_legacy_turso_migration` returns:
/// a clean run leaves no `.turso-operation.lock` behind.
#[cfg(unix)]
#[tokio::test]
async fn test_legacy_reembed_recovery_releases_the_operation_lock() {
    let config = common::store_config("legacy-recovery-release");
    let (config, connections, shards) = common::real_components(config).await;
    let storage = config.storage_path.clone();
    let shards = Arc::new(shards);
    let sleeper = common::RecordingSleeper::new();
    let sleep = sleeper.as_sleep_fn();

    mem_store::legacy_migrator::run_legacy_turso_migration(&storage, &shards, &connections, &sleep)
        .await
        .expect("a clean run succeeds");
    assert!(
        !storage.join(OPERATION_LOCK_FILE).exists(),
        "the recovery lock is released before the run returns"
    );
    let _ = common::cleanup_temp_dir(&storage);
}

/// G2's inversion arm — a live **cross-process legacy** holder refuses the
/// unlink, and nothing on disk changes.
///
/// The legacy child opens through the documented override
/// (`OPENCODE_MEM_ALLOW_UNGUARDED_OPEN=1`, no multiprocess WAL), so the probe
/// can see it (`Held`). The refusal must precede the first unlink: the main
/// file and all three sidecars stay byte-identical and the child's next write
/// still succeeds.
///
/// Inverted world: delete the probe from `remove_sqlite_database` → the parent
/// unlinks and the "still present" / snapshot assertions fail.
#[cfg(unix)]
#[tokio::test]
async fn test_g2_refusal_with_a_live_legacy_holder_child() {
    let config = common::store_config("g2-legacy-refusal");
    let db_path = config.storage_path.join("g2.db");
    // The child's legacy open never creates `-tshm`; seed it so the refusal's
    // "all three sidecars survive" arm covers the turso-owned file too.
    std::fs::write(sidecar(&db_path, "-tshm"), b"tshm-sentinel").expect("seed -tshm");

    let mut holder = TshmHolder::spawn(&db_path, true);
    let proof = common::exclusive_root(&config.storage_path);
    let baseline = file_snapshot(&db_path);
    assert_eq!(
        baseline
            .iter()
            .find(|(suffix, _)| *suffix == "-tshm")
            .and_then(|(_, bytes)| bytes.clone()),
        Some(b"tshm-sentinel".to_vec()),
        "the seeded -tshm is present before the refusal"
    );
    assert!(
        baseline
            .iter()
            .any(|(suffix, bytes)| suffix.is_empty() && bytes.is_some()),
        "the child's database exists before the refusal"
    );

    let claim = proof
        .claim(&db_path)
        .expect("the database is inside the root");
    let error = mem_store::sqlite_files::remove_sqlite_database(claim)
        .expect_err("a live legacy holder must refuse the unlink");
    assert!(
        matches!(error, StoreError::ExclusivityRefused { .. }),
        "the refusal must be the typed exclusivity error, got {error:?}"
    );
    assert_eq!(
        file_snapshot(&db_path),
        baseline,
        "the refusal precedes the first unlink: db, -wal, -shm and -tshm are unchanged"
    );

    let after = holder.write_more();
    assert_eq!(
        after, "ok",
        "the child's write after the refused unlink still succeeds"
    );
    holder.exit();
    let _ = common::cleanup_temp_dir(&config.storage_path);
}
