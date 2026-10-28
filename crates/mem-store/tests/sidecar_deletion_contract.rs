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
use std::sync::mpsc::Receiver;

use mem_store::config::StoreConfig;
use mem_store::db::{
    BatchMode, ConnectFactory, ConnectOptions, ExperimentalFeature, Statement, Value,
    turso_connect_factory,
};
use mem_store::error::StoreError;
use mem_store::platform::Platform;
use mem_store::schema_migrations::{
    MigrationOptions, SchemaMigration, apply_schema_migrations, memory_shard_migrations,
};
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

/// Seeds a real v1 shard through an open handle and returns that handle plus
/// the database path. The handle stays open on purpose: it is the pre-fix
/// self-holder.
async fn seed_open_v1(
    config: &mem_store::config::StoreConfig,
    file: &str,
) -> (mem_store::db::DbHandle, PathBuf) {
    let db = common::open_real_db(config, file).await.expect("open");
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
    (db, config.storage_path.join(file))
}

fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).expect("stat").ino()
}

/// G1 — the pre-fix self-holder write loss is silent.
///
/// Sequence: a real v1 shard is open through `db`; `apply_schema_migrations`
/// takes a backup, fails v2, then unlinks the live database and renames the
/// backup over its path. A write through the still-open `db` afterwards is
/// acknowledged but lands in the unlinked inode; the reopened path is the
/// backup's, so the write is invisible.
///
/// **Stop-and-branch measurement:** if the write is visible on reopen, the
/// premise is refuted and Steps 3–9 must not run.
#[cfg(unix)]
#[tokio::test]
async fn test_g1_self_holder_write_loss_is_silent() {
    let config = common::store_config("g1-self-holder");
    let (db, db_path) = seed_open_v1(&config, "shard.db").await;
    let original_inode = inode(&db_path);
    let options = MigrationOptions {
        db_path: Some(db_path.clone()),
        label: "test",
    };

    let error = apply_schema_migrations(
        db.as_ref(),
        &[migration(2, "SELECTX certainly not sql")],
        &options,
    )
    .await
    .expect_err("the failing v2 migration must re-raise");
    assert!(
        !error.to_string().is_empty(),
        "the original failure is surfaced: {error}"
    );

    // The restore unlinked the live file and renamed the backup over its path,
    // consuming the `.bak`: the path now names a different inode — the backup.
    let restored_inode = inode(&db_path);
    let leftovers: Vec<PathBuf> = std::fs::read_dir(&config.storage_path)
        .expect("storage dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.to_string_lossy().ends_with(".bak"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the restore consumes the backup it renames into place: {leftovers:?}"
    );
    assert_ne!(
        original_inode, restored_inode,
        "the database path must name the backup's inode after the restore"
    );

    // The write through the old handle: acknowledged (or an error we record),
    // never reaching the restored file.
    let write = db
        .execute(PRE_V2_INSERT, pre_v2_row("row2", "lost-write"))
        .await;
    let write_error = match &write {
        Ok(_) => "none".to_string(),
        Err(error) => error.to_string(),
    };

    drop(db);
    let reopened = common::open_real_db(&config, "shard.db")
        .await
        .expect("reopen");
    let version = reopened
        .query(sql::PRAGMA_USER_VERSION, Vec::new())
        .await
        .expect("version probe")[0]
        .i64("user_version")
        .unwrap_or(-1);
    let rows = reopened
        .query("SELECT id FROM memories", Vec::new())
        .await
        .expect("rows probe");
    let ids: Vec<String> = rows.iter().filter_map(|row| row.text("id")).collect();

    println!(
        "[G1 OBSERVED] write_error={write_error} user_version_after_reopen={version} \
         row_ids_after_reopen={ids:?} restored_inode={restored_inode}"
    );

    assert!(
        !ids.iter().any(|id| id == "row2"),
        "the write through the still-open handle must NOT survive the reopen; if it does, the \
         premise is refuted and this wave must stop (step 1 stop-and-branch). ids: {ids:?}"
    );
    assert_eq!(
        ids,
        vec!["row1".to_string()],
        "the reopened database is the backup, carrying exactly the seeded v1 row"
    );
    assert_eq!(version, 1, "the reopened database is the v1 backup");

    drop(reopened);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// The refusal: the probe runs before the first unlink
// ---------------------------------------------------------------------------

/// 46.6/46.2: a live legacy holder refuses the unlink **before any unlink**.
///
/// The holder is an in-process `F_WRLCK` POSIX lock on byte 0 — the probe's OFD
/// attempt conflicts with it (the measured `t2 S7` shape in `open_guard.rs`), so
/// this exercises the same refusal a cross-process legacy holder produces
/// (Step 9 adds that arm). Every file must survive, byte-identical.
///
/// Inverted world: delete the probe from `remove_sqlite_database` → the call
/// unlinks and the "still present" assertions fail.
///
/// The holder is an in-process **OFD** write lock (`F_OFD_SETLK`, Linux): it
/// belongs to the holder's open file description, so the probe's open/close
/// cannot release it. A POSIX (`F_SETLK`) holder would be released by the
/// probe's own `close()` (open_guard's measured `t2 S4/S6/S7`), making the
/// fixture vacuous after its first probe.
#[cfg(target_os = "linux")]
#[test]
fn test_remove_refuses_a_legacy_holder_before_any_unlink() {
    use std::os::unix::io::AsRawFd;

    let dir = common::temp_dir("remove-refused");
    let proof = common::exclusive_root(&dir);
    let db = dir.join("held.db");
    std::fs::write(&db, b"main").expect("seed the database");
    for suffix in ["-wal", "-shm", "-tshm"] {
        std::fs::write(sidecar(&db, suffix), suffix.as_bytes()).expect("seed a sidecar");
    }

    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&db)
        .expect("open the holder");
    // SAFETY: `libc::flock` is plain old data; all-zero is a valid bit pattern
    // and every meaningful field is overwritten below.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 0;
    lock.l_len = 1;
    lock.l_pid = 0;
    // SAFETY: `holder` owns the fd for the whole call and `lock` outlives it.
    let taken = unsafe { libc::fcntl(holder.as_raw_fd(), libc::F_OFD_SETLK, &lock) };
    assert_eq!(
        taken,
        0,
        "the fixture's OFD write lock must be taken: {}",
        std::io::Error::last_os_error()
    );
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
    lock.l_type = libc::F_UNLCK as _;
    // SAFETY: same owned fd, same still-live `flock`.
    let _ = unsafe { libc::fcntl(holder.as_raw_fd(), libc::F_SETLK, &lock) };
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

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    runtime.block_on(async {
        let mut options = ConnectOptions::for_config(&config, Platform::current());
        if legacy {
            options
                .experimental
                .retain(|feature| !matches!(feature, ExperimentalFeature::MultiprocessWal));
        }
        let db = turso_connect_factory()
            .connect(db_path.clone(), options)
            .await
            .expect("the child's database opens");
        db.execute(
            "CREATE TABLE IF NOT EXISTS holder_rows (id INTEGER PRIMARY KEY, v TEXT)",
            Vec::new(),
        )
        .await
        .expect("the child creates its table");
        db.execute(
            "INSERT INTO holder_rows (v) VALUES ('child-before')",
            Vec::new(),
        )
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
                    match db.execute(&sql, Vec::new()).await {
                        Ok(_) => println!("AFTER ok"),
                        Err(error) => println!("AFTER err {}", error.message()),
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
    let new_meta =
        std::fs::metadata(sidecar(&db_path, "-tshm")).expect("the next opener creates a new -tshm");
    let after_ino = format!("{}:{}", new_meta.dev(), new_meta.ino());

    println!(
        "[G2] TSHM_INO before={before} (len {}) AFTER=ok TSHM_INO after={after_ino} (len {}); \
         pre-existing coordination bytes gone",
        before_meta.len(),
        new_meta.len()
    );
    assert_ne!(
        (before_meta.dev(), before_meta.ino()),
        (new_meta.dev(), new_meta.ino()),
        "the next opener must get a different -tshm identity; the same inode means the raw \
         unlink did not happen and this control is vacuous"
    );

    drop(reopened);
    holder.exit();
    let _ = common::cleanup_temp_dir(&config.storage_path);
}
