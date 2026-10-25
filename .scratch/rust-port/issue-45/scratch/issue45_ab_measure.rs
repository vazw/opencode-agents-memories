//! TEMPORARY issue-45 A/B measurement harness (prototype; removed after the run).
//!
//! Copied to `.tmp/scratch/issue-45-ab/issue45_ab_measure.rs` with the exact
//! apply/run commands. Nothing here is a deliverable: the guard itself is not
//! implemented in this wave — this file only *measures* the facts the two
//! designs are scored against.
//!
//! No `libc` dependency: `fcntl` is declared directly (Linux x86_64/aarch64
//! constants), so the measurement does not touch `Cargo.toml`/`Cargo.lock`.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mem_store::config::StoreConfig;
use mem_store::db::{
    ConnectFactory, ConnectOptions, DbHandle, ExperimentalFeature, turso_connect_factory,
};
use mem_store::platform::Platform;

// ---------------------------------------------------------------------------
// syscall shim (Linux only — the measurement host)
// ---------------------------------------------------------------------------

#[repr(C)]
struct Flock {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
}

const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const F_SETLK: i32 = 6;
#[cfg(target_os = "linux")]
const F_OFD_SETLK: i32 = 37;

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, arg: *mut Flock) -> i32;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Probe {
    /// Nobody holds a conflicting lock; the probe released what it took.
    Free,
    /// A conflicting lock is held.
    Held,
    /// The target file does not exist (⇒ no holder can exist).
    Missing,
    /// The probe could not answer (`raw_os_error`).
    Unknown(i32),
}

/// Attempts an exclusive byte lock on `offset..offset+len` and releases it.
///
/// `ofd` selects `F_OFD_SETLK` (Linux) over `F_SETLK`. `len == 0` means
/// "whole file", exactly what turso's legacy `fcntl_lock` takes.
fn probe_lock(path: &Path, ofd: bool, len: i64) -> Probe {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Probe::Missing,
        Err(error) => return Probe::Unknown(error.raw_os_error().unwrap_or(-1)),
    };
    #[cfg(target_os = "linux")]
    let cmd = if ofd { F_OFD_SETLK } else { F_SETLK };
    #[cfg(not(target_os = "linux"))]
    let cmd = {
        let _ = ofd;
        F_SETLK
    };
    let mut lock = Flock {
        l_type: F_WRLCK,
        l_whence: 0, // SEEK_SET
        l_start: 0,
        l_len: len,
        l_pid: 0,
    };
    let fd = {
        use std::os::fd::AsRawFd;
        file.as_raw_fd()
    };
    let rc = unsafe { fcntl(fd, cmd, &mut lock as *mut Flock) };
    if rc == 0 {
        let mut release = Flock {
            l_type: F_UNLCK,
            ..lock
        };
        unsafe { fcntl(fd, cmd, &mut release as *mut Flock) };
        return Probe::Free;
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(11) | Some(13) => Probe::Held, // EAGAIN / EACCES
        other => Probe::Unknown(other.unwrap_or(-1)),
    }
}

/// Probe A: the `.tshm` byte turso's `detect_open_mode` uses
/// (`PROCESS_LIFETIME_LOCK_OFFSET = 0`, one byte).
fn probe_authority(tshm: &Path) -> Probe {
    probe_lock(tshm, true, 1)
}

/// Probe B, whole-file `F_SETLK` — what turso's own legacy `fcntl_lock` takes.
fn probe_legacy_wholefile(db: &Path) -> Probe {
    probe_lock(db, false, 0)
}

/// Probe B, one byte `F_SETLK` at offset 0.
fn probe_legacy_byte(db: &Path) -> Probe {
    probe_lock(db, false, 1)
}

/// Probe B, one byte `F_OFD_SETLK` at offset 0 (Linux).
fn probe_legacy_ofd_byte(db: &Path) -> Probe {
    probe_lock(db, true, 1)
}

fn tshm_path(db: &Path) -> PathBuf {
    PathBuf::from(format!("{}-tshm", db.display()))
}

/// One lock+unlock pair on an already-open fd, timed `n` times.
fn timed_lock_unlock(fd: i32, ofd: bool, len: i64, n: usize) -> Vec<std::time::Duration> {
    #[cfg(target_os = "linux")]
    let cmd = if ofd { F_OFD_SETLK } else { F_SETLK };
    #[cfg(not(target_os = "linux"))]
    let cmd = {
        let _ = ofd;
        F_SETLK
    };
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let host = Instant::now();
        let mut lock = Flock {
            l_type: F_WRLCK,
            l_whence: 0,
            l_start: 0,
            l_len: len,
            l_pid: 0,
        };
        let rc = unsafe { fcntl(fd, cmd, &mut lock as *mut Flock) };
        assert_eq!(rc, 0, "timing probe lock");
        let mut release = Flock {
            l_type: F_UNLCK,
            ..lock
        };
        let rc = unsafe { fcntl(fd, cmd, &mut release as *mut Flock) };
        assert_eq!(rc, 0, "timing probe unlock");
        samples.push(host.elapsed());
    }
    samples
}

// ---------------------------------------------------------------------------
// The decision core both designs share (A3 asymmetry, unknown ⇒ refuse)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Writability {
    Writable,
    ReadOnly,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Own {
    Ours,
    NotOurs,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Decision {
    Clear,
    Refuse(&'static str),
}

/// Probe A runs for every open; probe B (`b`) is `Some` only for a writable
/// open. `own == Ours` is the design-specific clearance and skips both probes.
fn decide(own: Own, writability: Writability, a: Probe, b: Option<Probe>) -> Decision {
    if own == Own::Ours {
        return Decision::Clear;
    }
    match a {
        Probe::Held => return Decision::Refuse("multiprocess-authority"),
        Probe::Unknown(_) => return Decision::Refuse("probe-a-unknown"),
        Probe::Free | Probe::Missing => {}
    }
    match writability {
        Writability::ReadOnly => Decision::Clear, // B not run; divergence recorded
        Writability::Writable => match b {
            Some(Probe::Held) => Decision::Refuse("legacy-authority"),
            Some(Probe::Unknown(_)) | None => Decision::Refuse("probe-b-unknown"),
            Some(Probe::Free) | Some(Probe::Missing) => Decision::Clear,
        },
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn store_config(dir: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: dir.to_path_buf(),
        embedding_dimensions: 768,
        embedding_model: "issue-45-measure".to_string(),
        max_vectors_per_shard: 12_500,
        container_tag_prefix: "code".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

/// The shipping option set (`ConnectOptions::for_config`): multiprocess WAL on
/// non-Windows, encryption feature off for an unencrypted config.
fn shipping(config: &StoreConfig) -> ConnectOptions {
    ConnectOptions::for_config(config, Platform::current())
}

/// Shipping minus `MultiprocessWal` — the legacy/flag-off option set, built by
/// subtraction exactly like `turso_capabilities::single_process_options`.
fn legacy(config: &StoreConfig) -> ConnectOptions {
    let mut options = shipping(config);
    options
        .experimental
        .retain(|feature| !matches!(feature, ExperimentalFeature::MultiprocessWal));
    options.encryption =
        mem_store::db::resolve_database_encryption(config).expect("resolve encryption options");
    options
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "mem-store-m45-{prefix}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create the measurement temp dir");
    dir
}

async fn open(
    path: &Path,
    options: ConnectOptions,
) -> Result<DbHandle, mem_store::error::StoreError> {
    turso_connect_factory()
        .connect(path.to_path_buf(), options)
        .await
}

async fn open_ok(path: &Path, options: ConnectOptions) -> DbHandle {
    open(path, options).await.unwrap_or_else(|error| {
        panic!(
            "open {} through TursoConnectFactory: {error}",
            path.display()
        )
    })
}

const CREATE_AB: &str = "CREATE TABLE IF NOT EXISTS ab (id INTEGER PRIMARY KEY, v TEXT NOT NULL)";

async fn seed(db: &DbHandle) {
    db.execute(CREATE_AB, Vec::new()).await.expect("create ab");
    db.execute(
        "INSERT OR IGNORE INTO ab (id, v) VALUES (1, 'seed')",
        Vec::new(),
    )
    .await
    .expect("seed row");
}

async fn insert(db: &DbHandle, id: i64, value: &str) {
    db.execute(
        "INSERT INTO ab (id, v) VALUES (?, ?)",
        vec![
            mem_store::db::Value::from(id),
            mem_store::db::Value::from(value),
        ],
    )
    .await
    .expect("insert row");
}

async fn ids(db: &DbHandle) -> Vec<i64> {
    db.query("SELECT id FROM ab ORDER BY id", Vec::new())
        .await
        .expect("read ids")
        .iter()
        .filter_map(|row| row.i64("id"))
        .collect()
}

async fn integrity(db: &DbHandle) -> String {
    let rows = db
        .query("PRAGMA integrity_check", Vec::new())
        .await
        .expect("integrity_check");
    match rows.first() {
        Some(row) => row
            .text("integrity_check")
            .unwrap_or_else(|| format!("<columns={:?}>", row.column_names())),
        None => "<no row>".to_string(),
    }
}

fn file_bytes(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

// ---------------------------------------------------------------------------
// Child process (the same test binary re-executed; holder or foreign opener)
// ---------------------------------------------------------------------------

struct Kid {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
    dead: bool,
}

const KID_GUARD: Duration = Duration::from_secs(60);

impl Kid {
    /// `kind` is `holder` (seeds, then writes on `WRITE`); `mode` is `on`/`off`.
    fn spawn(kind: &str, mode: &str, db: &Path) -> Self {
        let mut command = std::process::Command::new(
            std::env::current_exe().expect("the running test binary path"),
        );
        command
            .arg("--exact")
            .arg("m45_child")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .arg("--ignored")
            .env("M45_KIND", kind)
            .env("M45_MODE", mode)
            .env("M45_DB", db)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = command.spawn().expect("spawn the measurement child");
        let stdin = child.stdin.take().expect("piped child stdin");
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
        Self {
            child,
            stdin: Some(stdin),
            lines,
            dead: false,
        }
    }

    fn next_line(&mut self) -> String {
        loop {
            match self.lines.recv_timeout(KID_GUARD) {
                Ok(line) => {
                    if let Some(at) = line.find("M45|") {
                        return line[at + 4..].to_string();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    panic!("the measurement child produced no protocol line for {KID_GUARD:?}")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("the measurement child closed stdout without a protocol line")
                }
            }
        }
    }

    fn say(&mut self, line: &str) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.write_all(b"\n");
            let _ = stdin.flush();
            self.stdin = None;
        }
    }

    fn finish(mut self) {
        let deadline = Instant::now() + KID_GUARD;
        loop {
            match self.child.try_wait().expect("poll the measurement child") {
                Some(status) => {
                    self.dead = true;
                    assert!(
                        status.success(),
                        "the measurement child exited with {status:?}"
                    );
                    return;
                }
                None if Instant::now() >= deadline => panic!("the measurement child is wedged"),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.dead = true;
    }
}

impl Drop for Kid {
    fn drop(&mut self) {
        if !self.dead {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The re-executed child. Inert unless `M45_KIND` is set (so a normal suite run
/// reports it as ignored rather than as a green no-op).
#[test]
#[ignore = "re-exec'd worker: Kid::spawn runs it with --ignored"]
fn m45_child() {
    let Ok(kind) = std::env::var("M45_KIND") else {
        return;
    };
    let db = PathBuf::from(std::env::var("M45_DB").expect("M45_DB"));
    let mode = std::env::var("M45_MODE").unwrap_or_else(|_| "on".to_string());
    let dir = db
        .parent()
        .expect("the database has a parent")
        .to_path_buf();
    let config = store_config(&dir);
    let options = if mode == "on" {
        shipping(&config)
    } else {
        legacy(&config)
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    runtime.block_on(async move {
        let db_handle = match open(&db, options).await {
            Ok(handle) => {
                println!("M45|OPEN-OK");
                handle
            }
            Err(error) => {
                println!("M45|OPEN-ERR {error}");
                println!("M45|CLOSED");
                return;
            }
        };
        if kind == "holder" {
            seed(&db_handle).await;
        }
        println!("M45|READY");
        let mut barrier = String::new();
        let _ = std::io::stdin().read_line(&mut barrier);
        if kind == "holder" {
            insert(&db_handle, 3, "holder-after").await;
            println!("M45|WROTE");
        }
        let mut second = String::new();
        let _ = std::io::stdin().read_line(&mut second);
        drop(db_handle);
        println!("M45|CLOSED");
    });
}

// ---------------------------------------------------------------------------
// T1 — G1: the unguarded open through TursoConnectFactory succeeds and the
// pinned row loss reproduces.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn m45_t1_g1_unguarded_open_through_the_factory() {
    let dir = temp_dir("g1");
    let config = store_config(&dir);
    let db_path = dir.join("g1.db");
    {
        let seed_handle = open_ok(&db_path, shipping(&config)).await;
        seed(&seed_handle).await;
    }

    let mut holder = Kid::spawn("holder", "on", &db_path);
    assert_eq!(
        holder.next_line(),
        "OPEN-OK",
        "the flag-on holder must open"
    );
    let ready = holder.next_line();
    assert_eq!(ready, "READY", "the flag-on holder must be live: {ready}");

    // Measured from THIS process, which holds nothing:
    let a_foreign = probe_authority(&tshm_path(&db_path));
    let b_foreign = probe_legacy_wholefile(&db_path);
    println!("[m45] t1 probes_live_flag_on_holder probe_a={a_foreign:?} probe_b={b_foreign:?}");

    // The pre-fix open, through the production seam, legacy options.
    let opener = open(&db_path, legacy(&config)).await;
    match &opener {
        Ok(_) => println!("[m45] t1 opener=OPEN-OK (premise holds: the guard does not exist)"),
        Err(error) => {
            println!("[m45] t1 opener=OPEN-ERR {error}");
            println!("[m45] t1 VERDICT=premise-refuted");
            holder.kill();
            panic!("G1: the unguarded mixed open was refused: {error}");
        }
    }
    let opener = opener.expect("checked above");
    insert(&opener, 2, "opener").await;
    let opener_before = ids(&opener).await;

    holder.say("WRITE");
    let wrote = holder.next_line();
    assert_eq!(wrote, "WROTE", "the holder's later commit: {wrote}");
    holder.say("CLOSE");
    let closed = holder.next_line();
    assert_eq!(closed, "CLOSED");
    holder.finish();

    let opener_after = ids(&opener).await;
    drop(opener);

    let fresh = open_ok(&db_path, shipping(&config)).await;
    let final_ids = ids(&fresh).await;
    let final_integrity = integrity(&fresh).await;
    drop(fresh);

    println!(
        "[m45] t1 RESULT opener=OPEN-OK opener_before={opener_before:?} opener_after={opener_after:?} final={final_ids:?} integrity={final_integrity} lost_row_2={}",
        !final_ids.contains(&2)
    );
    assert_eq!(opener_before, vec![1, 2]);
    assert_eq!(
        opener_after,
        vec![1, 2],
        "the opener must not see the holder's later commit"
    );
    assert_eq!(
        final_ids,
        vec![1, 3],
        "the opener's committed row must be GONE"
    );
    assert_eq!(final_integrity, "ok", "the loss must be silent");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// T2 — self-conflict: the raw probes against a database THIS PROCESS holds,
// and whether probe B costs the holder its lock.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn m45_t2_self_conflict() {
    let dir = temp_dir("self");
    let config = store_config(&dir);

    // -- flag-on own handle: probe A and the 45.6 second open.
    let on_path = dir.join("own_on.db");
    let on_handle = open_ok(&on_path, shipping(&config)).await;
    seed(&on_handle).await;
    let a_own = probe_authority(&tshm_path(&on_path));
    let second = open(&on_path, shipping(&config)).await;
    let second_verdict = match &second {
        Ok(_) => "ok".to_string(),
        Err(error) => format!("err {error}"),
    };
    println!("[m45] t2 S1 own_flag_on probe_a={a_own:?} second_factory_open={second_verdict}");
    drop(second);

    // -- `.tshm` immutability across a probe while the holder is live.
    let tshm_before = file_bytes(&tshm_path(&on_path));
    let a_while_held = probe_authority(&tshm_path(&on_path));
    let tshm_after = file_bytes(&tshm_path(&on_path));
    println!(
        "[m45] t2 S5 probe_a_does_not_mutate_tshm while_held={a_while_held:?} before_len={:?} after_len={:?} identical={}",
        tshm_before.as_ref().map(Vec::len),
        tshm_after.as_ref().map(Vec::len),
        tshm_before == tshm_after
    );
    drop(on_handle);
    let a_stale = probe_authority(&tshm_path(&on_path));
    println!("[m45] t2 S2 stale_tshm_no_holder probe_a={a_stale:?} (G3: must be Free)");

    // -- flag-off own handle: the control, then probe B's cost.
    let off_control = dir.join("own_off_control.db");
    let off_control_handle = open_ok(&off_control, legacy(&config)).await;
    seed(&off_control_handle).await;
    let mut control_kid = Kid::spawn("probe-only", "off", &off_control);
    let control = control_kid.next_line();
    println!("[m45] t2 S3 control_no_probe_foreign_flag_off_open={control}");
    control_kid.kill();
    drop(off_control_handle);

    // whole-file F_SETLK probe against our own held flag-off DB
    let off_whole = dir.join("own_off_whole.db");
    let off_whole_handle = open_ok(&off_whole, legacy(&config)).await;
    seed(&off_whole_handle).await;
    let b_own_whole = probe_legacy_wholefile(&off_whole);
    let mut kid = Kid::spawn("probe-only", "off", &off_whole);
    let after_whole = kid.next_line();
    println!(
        "[m45] t2 S4 own_flag_off probe_b_wholefile={b_own_whole:?} foreign_open_after={after_whole}"
    );
    kid.kill();
    drop(off_whole_handle);

    // one-byte F_SETLK probe against our own held flag-off DB
    let off_byte = dir.join("own_off_byte.db");
    let off_byte_handle = open_ok(&off_byte, legacy(&config)).await;
    seed(&off_byte_handle).await;
    let b_own_byte = probe_legacy_byte(&off_byte);
    let mut kid = Kid::spawn("probe-only", "off", &off_byte);
    let after_byte = kid.next_line();
    println!(
        "[m45] t2 S6 own_flag_off probe_b_one_byte={b_own_byte:?} foreign_open_after={after_byte}"
    );
    kid.kill();
    drop(off_byte_handle);

    // one-byte F_OFD_SETLK probe against our own held flag-off DB (Linux)
    let off_ofd = dir.join("own_off_ofd.db");
    let off_ofd_handle = open_ok(&off_ofd, legacy(&config)).await;
    seed(&off_ofd_handle).await;
    let b_own_ofd = probe_legacy_ofd_byte(&off_ofd);
    let mut kid = Kid::spawn("probe-only", "off", &off_ofd);
    let after_ofd = kid.next_line();
    println!(
        "[m45] t2 S7 own_flag_off probe_b_ofd_byte={b_own_ofd:?} foreign_open_after={after_ofd}"
    );
    kid.kill();
    drop(off_ofd_handle);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// T3 — the A/B discriminator: clearance evidence against a FOREIGN holder of a
// sibling database in the same storage root.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn m45_t3_clearance_evidence_ab() {
    let dir = temp_dir("ab");
    let config = store_config(&dir);

    // DB1 is ours (this is what writes the A5 marker / populates a registry).
    let one = dir.join("one.db");
    let one_handle = open_ok(&one, shipping(&config)).await;
    seed(&one_handle).await;
    let marker_start = mem_store::platform::process_start_time(std::process::id());
    let marker = format!(
        "{{\"pid\":{},\"start_time\":{},\"mode\":\"multiprocess\"}}",
        std::process::id(),
        marker_start
            .map(|token| token.to_string())
            .unwrap_or_else(|| "null".to_string())
    );
    std::fs::write(dir.join(".wal-mode"), &marker).expect("write the A5 marker");
    println!("[m45] t3 marker_written={marker}");

    // DB2 is held by a FOREIGN process in legacy mode (cell 1 shape).
    let two = dir.join("two.db");
    {
        let seed_handle = open_ok(&two, legacy(&config)).await;
        seed(&seed_handle).await;
    }
    let mut foreign = Kid::spawn("holder", "off", &two);
    assert_eq!(
        foreign.next_line(),
        "OPEN-OK",
        "the foreign legacy holder must open"
    );
    let ready = foreign.next_line();
    assert_eq!(
        ready, "READY",
        "the foreign legacy holder must be live: {ready}"
    );

    // The measured probe inputs (probe A is `Missing`: a legacy holder never
    // creates `.tshm`).
    let a = probe_authority(&tshm_path(&two));
    let b = probe_legacy_byte(&two);

    // Design A's clearance evidence: this process's registry of held file
    // identities. DB2 is not in it (we hold DB1 only).
    let mut registry: HashMap<(u64, u64), usize> = HashMap::new();
    if let Some(id) = file_id(&one) {
        *registry.entry(id).or_default() += 1;
    }
    let a_own = if file_id(&two).is_some_and(|id| registry.contains_key(&id)) {
        Own::Ours
    } else {
        Own::NotOurs
    };

    // Design B's clearance evidence: the per-root A5 marker names this process.
    let b_own = marker.contains(&format!("\"pid\":{}", std::process::id()))
        && marker_start.is_some()
        && marker.contains(&format!(
            "\"start_time\":{}",
            marker_start
                .map(|token| token.to_string())
                .unwrap_or_default()
        ));

    let a_decision = decide(a_own, Writability::Writable, a, Some(b));
    let b_decision = decide(
        if b_own { Own::Ours } else { Own::NotOurs },
        Writability::Writable,
        a,
        Some(b),
    );
    println!(
        "[m45] t3 inputs a={a:?} b={b:?} registry={registry:?} a_own={a_own:?} b_own={b_own} a_decision={a_decision:?} b_decision={b_decision:?}"
    );

    // Execute B's clearance for real: open DB2 flag-on (NoLock ⇒ accepted),
    // write, let the legacy holder commit and close, then close and re-read.
    let opened = open(&two, shipping(&config)).await;
    match &opened {
        Ok(_) => println!("[m45] t3 B_path_open=OPEN-OK"),
        Err(error) => println!("[m45] t3 B_path_open=OPEN-ERR {error}"),
    }
    let b_handle = opened.expect("B's clearance lets the open through");
    insert(&b_handle, 2, "opener").await;
    let before = ids(&b_handle).await;
    foreign.say("WRITE");
    assert_eq!(foreign.next_line(), "WROTE");
    foreign.say("CLOSE");
    assert_eq!(foreign.next_line(), "CLOSED");
    foreign.finish();
    let after = ids(&b_handle).await;
    drop(b_handle);
    let fresh = open_ok(&two, shipping(&config)).await;
    let final_ids = ids(&fresh).await;
    let final_integrity = integrity(&fresh).await;
    drop(fresh);
    println!(
        "[m45] t3 B_path_result before={before:?} after={after:?} final={final_ids:?} integrity={final_integrity} lost_row_2={}",
        !final_ids.contains(&2)
    );

    println!(
        "[m45] t3 DECISION A={a_decision:?} B={b_decision:?} differ={}",
        a_decision != b_decision
    );
    assert_eq!(a_decision, Decision::Refuse("legacy-authority"));
    assert_eq!(b_decision, Decision::Clear);
    assert_eq!(
        final_ids,
        vec![1, 3],
        "B's clearance loses the opener's row"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// T4 — cost (45.12). Measured, never guessed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn m45_t4_cost() {
    let dir = temp_dir("cost");
    let config = store_config(&dir);
    let db_path = dir.join("cost.db");
    {
        let handle = open_ok(&db_path, shipping(&config)).await;
        seed(&handle).await;
    }
    let tshm = tshm_path(&db_path);
    assert!(tshm.exists(), "the flag-on open must leave .tshm behind");
    let marker_path = dir.join(".wal-mode");
    std::fs::write(
        &marker_path,
        "{\"pid\":1,\"start_time\":2,\"mode\":\"multiprocess\"}",
    )
    .expect("write marker");

    const N: usize = 2000;
    let mut probe_a = Vec::with_capacity(N);
    let mut probe_b = Vec::with_capacity(N);
    let mut bare_open = Vec::with_capacity(N);
    for _ in 0..N {
        let start = Instant::now();
        let verdict = probe_authority(&tshm);
        probe_a.push(start.elapsed());

        let start = Instant::now();
        let verdict_b = probe_legacy_byte(&db_path);
        probe_b.push(start.elapsed());

        let start = Instant::now();
        drop(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&db_path)
                .expect("open"),
        );
        bare_open.push(start.elapsed());

        assert!(
            matches!(verdict, Probe::Free),
            "probe A on a stale .tshm: {verdict:?}"
        );
        assert!(
            matches!(verdict_b, Probe::Free),
            "probe B with no holder: {verdict_b:?}"
        );
    }

    let mut marker_read = Vec::with_capacity(N);
    for _ in 0..N {
        let start = Instant::now();
        let text = std::fs::read_to_string(&marker_path).expect("read marker");
        let _value: serde_json::Value = serde_json::from_str(&text).expect("parse marker");
        marker_read.push(start.elapsed());
    }

    let registry: HashMap<(u64, u64), usize> = file_id(&db_path)
        .map(|id| (id, 1usize))
        .into_iter()
        .collect();
    const LOOKUPS: usize = 200_000;
    let start = Instant::now();
    let mut hits = 0usize;
    for _ in 0..LOOKUPS {
        if registry.contains_key(&file_id(&db_path).expect("id")) {
            hits += 1;
        }
    }
    let registry_total = start.elapsed();
    assert_eq!(hits, LOOKUPS);
    let known = file_id(&db_path).expect("id");
    let start = Instant::now();
    let mut hits = 0usize;
    for _ in 0..LOOKUPS {
        if registry.contains_key(&known) {
            hits += 1;
        }
    }
    let registry_pure = start.elapsed();
    assert_eq!(hits, LOOKUPS);

    // The fcntl calls alone, on an fd that is already open: this is the
    // "two fcntl calls per open" number 45.12 asks for, separated from the
    // open/close each probe also needs.
    let tshm_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tshm)
        .expect("open tshm");
    let db_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&db_path)
        .expect("open db");
    let fcntl_a = {
        use std::os::fd::AsRawFd;
        timed_lock_unlock(tshm_file.as_raw_fd(), true, 1, N)
    };
    let fcntl_b = {
        use std::os::fd::AsRawFd;
        timed_lock_unlock(db_file.as_raw_fd(), false, 1, N)
    };

    fn stats(label: &str, samples: &[Duration]) -> String {
        let total: Duration = samples.iter().sum();
        let mean = total.as_secs_f64() * 1e6 / samples.len() as f64;
        let mut sorted = samples.to_vec();
        sorted.sort();
        let p95 = sorted[sorted.len() * 95 / 100].as_secs_f64() * 1e6;
        let max = sorted[sorted.len() - 1].as_secs_f64() * 1e6;
        format!(
            "{label}: n={} mean={mean:.3}us p95={p95:.3}us max={max:.3}us",
            samples.len()
        )
    }
    println!("[m45] t4 {}", stats("probe_a_tshm", &probe_a));
    println!("[m45] t4 {}", stats("probe_b_db_one_byte_fsetlk", &probe_b));
    println!("[m45] t4 {}", stats("bare_open_close_baseline", &bare_open));
    println!("[m45] t4 {}", stats("marker_read_parse", &marker_read));
    println!("[m45] t4 {}", stats("fcntl_ofd_lock_unlock_only", &fcntl_a));
    println!(
        "[m45] t4 {}",
        stats("fcntl_setlk_lock_unlock_only", &fcntl_b)
    );
    println!(
        "[m45] t4 registry_contains_key_with_stat: n={LOOKUPS} mean={:.3}ns",
        registry_total.as_secs_f64() * 1e9 / LOOKUPS as f64
    );
    println!(
        "[m45] t4 registry_contains_key_precomputed: n={LOOKUPS} mean={:.3}ns",
        registry_pure.as_secs_f64() * 1e9 / LOOKUPS as f64
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// T5 — the A3 decision table (printed, not asserted separately: T3/T1 hold the
// assertions; this makes the writability asymmetry visible in the artefact).
// ---------------------------------------------------------------------------

#[test]
fn m45_t5_decision_table() {
    let probes = [Probe::Free, Probe::Held, Probe::Missing, Probe::Unknown(5)];
    for writability in [Writability::Writable, Writability::ReadOnly] {
        for a in probes {
            let b = if writability == Writability::Writable {
                Some(Probe::Free)
            } else {
                None
            };
            println!(
                "[m45] t5 writability={writability:?} probe_a={a:?} probe_b={b:?} => {:?}",
                decide(Own::NotOurs, writability, a, b)
            );
            if writability == Writability::Writable {
                println!(
                    "[m45] t5 writability={writability:?} probe_a={a:?} probe_b=Held => {:?}",
                    decide(Own::NotOurs, writability, a, Some(Probe::Held))
                );
            }
        }
        println!(
            "[m45] t5 writability={writability:?} own=Ours => {:?}",
            decide(Own::Ours, writability, Probe::Held, Some(Probe::Held))
        );
    }
}
