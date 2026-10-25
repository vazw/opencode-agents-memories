//! The mixed-version open guard battery (issue 45).
//!
//! Steps land in blueprint order: typed refusals first (45.7), then the probe (45.4/45.10), the
//! factory wiring (45.2/45.3), the positive control and its inversion arm (45.8/O6), the mode marker
//! (45.14/A5) and the config key (45.5/O5).
//!
//! The child-holder harness lives here too: it re-execs this same test binary (the
//! `turso_capabilities.rs:2229` precedent) so the "legacy holder" is a real, distinct process.

mod common;

use std::path::PathBuf;

use mem_store::error::{OpenDirection, StoreError};

// ---------------------------------------------------------------------------
// 45.7 — typed refusals: the size budget and the two Display contracts
// ---------------------------------------------------------------------------

/// 45.7: the two new payloads must not grow `StoreError` past the 128-byte
/// budget (and therefore must not trip `clippy::result_large_err` anywhere in
/// the workspace). The printed numbers are what ADR-0025 quotes.
#[test]
fn test_store_error_stays_within_the_large_error_budget() {
    let error_size = std::mem::size_of::<StoreError>();
    let option_size = std::mem::size_of::<Option<StoreError>>();
    let direction_size = std::mem::size_of::<OpenDirection>();
    let path_size = std::mem::size_of::<PathBuf>();
    println!(
        "size_of::<StoreError>() = {error_size}; size_of::<Option<StoreError>>() = {option_size}; \
         size_of::<OpenDirection>() = {direction_size}; size_of::<PathBuf>() = {path_size}"
    );
    assert!(
        error_size <= 128,
        "StoreError must stay within 128 bytes, measured {error_size}"
    );
    assert!(
        option_size <= 128,
        "Option<StoreError> must stay within 128 bytes, measured {option_size}"
    );
}

/// 45.7: `OpenBlocked` names the path and the *direction*, never a holder
/// identity — an OFD lock carries no `l_pid`, so any pid in the text would be
/// an invented fact.
#[test]
fn test_open_blocked_refusal_names_path_and_direction_without_a_pid() {
    let db_path = PathBuf::from("/tmp/mem-store-guard/example.db");
    let legacy = StoreError::OpenBlocked {
        db_path: db_path.clone(),
        direction: OpenDirection::LegacyAuthority,
    };
    let rendered = legacy.to_string();
    assert!(
        rendered.contains(db_path.to_string_lossy().as_ref()),
        "the refusal must name the database file: {rendered}"
    );
    assert!(
        rendered.contains("single-process"),
        "the refusal must name the direction: {rendered}"
    );
    assert!(
        !rendered.to_ascii_lowercase().contains("pid"),
        "the refusal must never claim a holder identity (an OFD lock has no l_pid): {rendered}"
    );

    let unknown = StoreError::OpenBlocked {
        db_path: db_path.clone(),
        direction: OpenDirection::Unknown { errno: Some(11) },
    };
    let rendered = unknown.to_string();
    assert!(
        rendered.contains("could not determine"),
        "an unknown probe answer must be its own outcome, not 'held': {rendered}"
    );
    assert!(
        rendered.contains("11"),
        "the errno must be carried for diagnosis: {rendered}"
    );

    let conflict = StoreError::OpenBlocked {
        db_path,
        direction: OpenDirection::ModeMarkerConflict { recorded: "legacy" },
    };
    let rendered = conflict.to_string();
    assert!(
        rendered.contains("legacy"),
        "a marker conflict must name the recorded mode: {rendered}"
    );
}

/// 45.7: the legacy-open refusal names the path, the mode it saw, and **both**
/// ways to bypass the guard — without re-spelling the constant strings.
#[test]
fn test_unsupported_open_mode_refusal_names_path_mode_and_the_override() {
    let db_path = PathBuf::from("/tmp/mem-store-guard/legacy.db");
    let error = StoreError::UnsupportedOpenMode {
        db_path: db_path.clone(),
        mode: "single-process",
    };
    let rendered = error.to_string();
    for expected in [
        db_path.to_string_lossy().as_ref(),
        "single-process",
        mem_store::open_guard::ALLOW_UNGUARDED_OPEN_ENV,
        mem_store::open_guard::allow_unguarded_open_key(),
    ] {
        assert!(
            rendered.contains(expected),
            "the refusal must name {expected:?}: {rendered}"
        );
    }
    assert!(
        error.is_open_guard_refusal(),
        "the refusal must be recognisable at the manager boundary"
    );
    assert!(
        !StoreError::internal("unrelated").is_open_guard_refusal(),
        "the classifier must not claim unrelated errors"
    );
}

// ---------------------------------------------------------------------------
// 45.4 / 45.10 — the database-file probe against a real child process
// ---------------------------------------------------------------------------

use mem_store::open_guard::{
    LockKind, ProbeOutcome, probe_database_file, probe_database_file_with,
};

/// A child process holding a **legacy** (single-process) turso handle on one
/// database file. The child is this same test binary re-executed, filtered to
/// [`fixture_child_entry`], and the handshake is the house two-line pipe protocol
/// (`turso_capabilities.rs:2229` precedent): the parent waits for `HOLDING <pid>`
/// on the child's stdout and releases it over stdin. No sleeps, no polling, no
/// ready/release files.
///
/// **Two hard rules, both learned from a real deadlock (ledger note, 2026-10-25):**
///
/// 1. libtest writes its `test <name> ... ` prefix **without a trailing newline**
///    under `--nocapture`, so the marker is glued to the banner
///    (`test fixture_child_entry ... HOLDING 1234`). The marker must be matched
///    *inside* the line, never at its start.
/// 2. **No unbounded read may block this binary.** The re-exec'd child inherits the
///    parent's `.tmp/build.lock` descriptor, so a hung fixture holds the
///    cross-wave cargo serialiser for as long as it lives. Every read below goes
///    through [`FIXTURE_TIMEOUT`], and `Drop` kills + reaps on every path
///    (including a panic in the parent), so no fixture child can outlive a test.
struct LegacyHolder {
    child: Option<std::process::Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
}

/// How long any single handshake step may take. Generous enough for a cold page
/// cache, short enough that a regression fails in seconds rather than minutes.
const FIXTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

impl LegacyHolder {
    fn spawn(path: &std::path::Path) -> Self {
        let mut child = std::process::Command::new(
            std::env::current_exe().expect("the running test binary's path"),
        )
        .arg("--exact")
        .arg("fixture_child_entry")
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("MEM_FIXTURE_CHILD", "legacy-holder")
        .env("MEM_FIXTURE_DB", path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the legacy-holder child");
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped child stdout");
        // A reader thread turns the pipe into a bounded channel, so no read can
        // block the test thread indefinitely (rule 2 above).
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
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
        holder.expect_line("HOLDING ", Some(pid));
        holder
    }

    /// Blocks (bounded) until a line containing `marker` arrives, then returns the
    /// text after it. Panics — which kills the child via `Drop` — on timeout or
    /// EOF, naming everything the child did print.
    fn expect_line(&self, marker: &str, expected_pid: Option<u32>) -> String {
        let mut seen: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + FIXTURE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some(rest) = line.split_once(marker).map(|(_, rest)| rest) {
                        let rest = rest.trim().to_string();
                        if let Some(expected) = expected_pid {
                            let announced: u32 = rest
                                .split_whitespace()
                                .next()
                                .and_then(|pid| pid.parse().ok())
                                .unwrap_or_else(|| {
                                    panic!("`{marker}` must be followed by a pid, got {line:?}")
                                });
                            assert_eq!(
                                announced, expected,
                                "the announced pid must be this child's, so the line is provably \
                                 its own: {line:?}"
                            );
                        }
                        return rest;
                    }
                    seen.push(line);
                }
                Err(error) => panic!(
                    "the legacy-holder child did not print `{marker}` within {FIXTURE_TIMEOUT:?} \
                     ({error}); output so far: {seen:?}"
                ),
            }
        }
    }

    /// Tells the child to drop its handles. Bounded, and the child is killed and
    /// reaped by `Drop` on the way out either way.
    fn release(mut self) {
        use std::io::Write;
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"release\n");
            let _ = stdin.flush();
        }
        self.expect_line("RELEASED", None);
    }
}

impl Drop for LegacyHolder {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // `kill()` on an already-exited child is a harmless no-op; `wait()`
            // then reaps it immediately, so this teardown cannot block.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The fixture child itself: a legacy (no `experimental_multiprocess_wal`) turso
/// handle held open until the parent says `release` on stdin.
#[cfg(unix)]
#[test]
#[ignore = "re-exec'd fixture child"]
fn fixture_child_entry() {
    if std::env::var("MEM_FIXTURE_CHILD").as_deref() != Ok("legacy-holder") {
        return;
    }
    use std::io::{BufRead, Write};

    let path = std::env::var("MEM_FIXTURE_DB").expect("MEM_FIXTURE_DB");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    let (database, connection) = runtime.block_on(async move {
        let database = turso::Builder::new_local(&path)
            .build()
            .await
            .expect("the child's legacy database opens");
        let connection = database.connect().expect("the child's connection");
        // A SELECT proves the handle is a real, serving connection, not a builder
        // that never opened.
        connection
            .query("SELECT 1", Vec::<turso::Value>::new())
            .await
            .expect("the child's handle answers");
        (database, connection)
    });
    println!("HOLDING {}", std::process::id());
    std::io::stdout().flush().expect("flush the holding line");

    let mut release = String::new();
    let _ = std::io::stdin().lock().read_line(&mut release);
    drop(connection);
    drop(database);
    drop(runtime);
    println!("RELEASED");
    let _ = std::io::stdout().flush();
}

#[cfg(unix)]
#[test]
fn test_probe_reports_held_for_a_live_legacy_holder() {
    let dir = common::temp_dir("probe-held");
    let path = dir.join("held.db");
    let holder = LegacyHolder::spawn(&path);
    let outcome = probe_database_file(&path);
    holder.release();
    println!(
        "probe(platform = {:?}) against a live legacy holder = {outcome:?}",
        LockKind::platform()
    );
    assert_eq!(
        outcome,
        ProbeOutcome::Held,
        "45.4: a live flag-off holder holds the database file's lock; the probe must see it"
    );
    let _ = common::cleanup_temp_dir(&dir);
}

#[cfg(unix)]
#[test]
fn test_probe_reports_free_for_an_unheld_database_file() {
    let dir = common::temp_dir("probe-free");
    let path = dir.join("free.db");
    std::fs::write(&path, b"").expect("create an empty database file");
    let outcome = probe_database_file(&path);
    println!("probe against an unheld file = {outcome:?}");
    assert_eq!(
        outcome,
        ProbeOutcome::Free,
        "an unheld file must read Free — the probe's own lock is taken and released"
    );
    let _ = common::cleanup_temp_dir(&dir);
}

#[cfg(unix)]
#[test]
fn test_probe_reports_missing_for_an_absent_database_file() {
    let dir = common::temp_dir("probe-missing");
    let path = dir.join("absent.db");
    let outcome = probe_database_file(&path);
    assert_eq!(
        outcome,
        ProbeOutcome::Missing,
        "an absent database is a known answer, not Unknown"
    );
    assert!(
        !path.exists(),
        "the probe must never create a database (O_RDWR without O_CREAT)"
    );
    let _ = common::cleanup_temp_dir(&dir);
}

#[cfg(unix)]
#[test]
fn test_probe_posix_variant_reports_held_for_a_live_legacy_holder() {
    let dir = common::temp_dir("probe-posix-held");
    let path = dir.join("held.db");
    let holder = LegacyHolder::spawn(&path);
    let outcome = probe_database_file_with(&path, LockKind::Posix);
    holder.release();
    println!("probe(F_SETLK) against a live legacy holder = {outcome:?}");
    assert_eq!(
        outcome,
        ProbeOutcome::Held,
        "45.10: the F_SETLK path is tested on Linux, not assumed"
    );
    let _ = common::cleanup_temp_dir(&dir);
}

#[cfg(unix)]
#[test]
fn test_probe_posix_variant_reports_free_for_an_unheld_database_file() {
    let dir = common::temp_dir("probe-posix-free");
    let path = dir.join("free.db");
    std::fs::write(&path, b"").expect("create an empty database file");
    let outcome = probe_database_file_with(&path, LockKind::Posix);
    assert_eq!(outcome, ProbeOutcome::Free);
    let _ = common::cleanup_temp_dir(&dir);
}

// ---------------------------------------------------------------------------
// 45.2 / 45.3 / 45.6 — the guard at the factory seam, no call-site change
// ---------------------------------------------------------------------------

use mem_store::config::StoreConfig;
use mem_store::db::{
    ConnectFactory, ConnectOptions, ExperimentalFeature, TursoConnectFactory, turso_connect_factory,
};
use mem_store::platform::Platform;

/// The shipping option set over `config` — the exact options every production
/// open gets (`for_config`), which is what makes the seam test a production test.
fn shipping_options(config: &StoreConfig) -> ConnectOptions {
    ConnectOptions::for_config(config, Platform::current())
}

/// The shipping set minus the multiprocess-WAL token: the option set issue 45's
/// O3 refuses by construction. Production never builds it; only a legacy build
/// (Windows, or an older opener) would.
fn legacy_options(config: &StoreConfig) -> ConnectOptions {
    let mut options = shipping_options(config);
    options
        .experimental
        .retain(|feature| !matches!(feature, ExperimentalFeature::MultiprocessWal));
    options
}

#[cfg(unix)]
#[tokio::test]
async fn test_factory_open_succeeds_with_no_holder() {
    let config = common::store_config("guard-open-clean");
    let path = config.storage_path.join("clean.db");
    let handle = turso_connect_factory()
        .connect(path.clone(), shipping_options(&config))
        .await
        .expect("45.2/G3: no holder ⇒ the guarded open succeeds with no call-site change");
    let rows = handle
        .query("SELECT 1 AS one", Vec::new())
        .await
        .expect("the guarded handle answers");
    assert_eq!(rows.first().and_then(|row| row.i64("one")), Some(1));
    drop(handle);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

#[cfg(unix)]
#[tokio::test]
async fn test_factory_open_succeeds_with_a_stale_tshm_and_no_holder() {
    let config = common::store_config("guard-open-stale-tshm");
    let path = config.storage_path.join("stale.db");
    let tshm = std::path::PathBuf::from(format!("{}-tshm", path.display()));

    // Produce a real `.tshm` (valid magic, correct length) with a live handle…
    let handle = turso_connect_factory()
        .connect(path.clone(), shipping_options(&config))
        .await
        .expect("seed the database");
    let bytes =
        std::fs::read(&tshm).expect("G3's fixture needs a real `-tshm` while the handle is live");
    drop(handle);
    // …and make sure it is still there afterwards: a stale sidecar, not a live one.
    if !tshm.exists() {
        std::fs::write(&tshm, &bytes).expect("restore the stale `-tshm`");
    }
    assert!(
        tshm.exists(),
        "the fixture requires a `.tshm` on disk with no holder"
    );

    let reopened = turso_connect_factory()
        .connect(path, shipping_options(&config))
        .await;
    assert!(
        reopened.is_ok(),
        "G3: presence of `-tshm` is not liveness — a stale sidecar must not refuse the open: {:?}",
        reopened.err()
    );
    drop(reopened);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

#[cfg(unix)]
#[tokio::test]
async fn test_legacy_connect_options_are_refused_with_unsupported_open_mode() {
    let config = common::store_config("guard-legacy-refused");
    let path = config.storage_path.join("legacy.db");
    let error = match TursoConnectFactory
        .connect(path.clone(), legacy_options(&config))
        .await
    {
        Ok(_) => panic!("O3: a legacy option set must be refused by construction"),
        Err(error) => error,
    };
    match &error {
        StoreError::UnsupportedOpenMode { db_path, mode } => {
            assert_eq!(db_path, &path);
            assert_eq!(*mode, "legacy");
        }
        other => panic!(
            "45.3/45.10: the refusal must be the typed `UnsupportedOpenMode` tripwire (the \
             structural guard against the first legacy opener landing silently), got {other:?}"
        ),
    }
    assert!(
        error
            .to_string()
            .contains("OPENCODE_MEM_ALLOW_UNGUARDED_OPEN"),
        "the refusal must name the documented escape: {error}"
    );
    // The refusal happens before turso: no database, no sidecars.
    assert!(
        !path.exists(),
        "the refusal must not create the database (preflight runs before any turso call)"
    );
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

#[cfg(unix)]
#[tokio::test]
async fn test_second_open_of_a_database_this_process_holds_is_not_refused() {
    let config = common::store_config("guard-second-open");
    let path = config.storage_path.join("twice.db");
    let first = turso_connect_factory()
        .connect(path.clone(), shipping_options(&config))
        .await
        .expect("the first open");
    let second = turso_connect_factory()
        .connect(path, shipping_options(&config))
        .await;
    assert!(
        second.is_ok(),
        "45.6/O7: a multiprocess handle holds no lock on the database file, so a second open of \
         the same file in this process must not be refused: {:?}",
        second.err()
    );
    drop(second);
    drop(first);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

#[cfg(unix)]
#[tokio::test]
async fn test_connection_manager_surfaces_the_typed_refusal_unchanged() {
    use mem_store::connection::ConnectionManager;
    use std::sync::Arc;

    let config = common::store_config("guard-manager-refusal");
    let path = config.storage_path.join("held.db");
    let holder = LegacyHolder::spawn(&path);
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory) as Arc<dyn ConnectFactory>,
    );
    let result = manager.get_connection(&path).await;
    holder.release();
    match result {
        Ok(_) => panic!(
            "45.2/45.7: the manager must surface `StoreError::OpenBlocked`, got a successful open"
        ),
        Err(StoreError::OpenBlocked { direction, .. }) => assert_eq!(
            direction,
            mem_store::error::OpenDirection::LegacyAuthority,
            "45.7: the manager must not launder the refusal into `StoreError::Turso`"
        ),
        Err(other) => {
            panic!("45.2/45.7: the manager must surface `StoreError::OpenBlocked`, got {other:?}")
        }
    }
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// 45.8 / O6 — the one integration direction the guard keeps, and its inversion
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn test_factory_open_refuses_a_live_legacy_holder() {
    use mem_store::error::OpenDirection;

    let config = common::store_config("guard-refuses-legacy-holder");
    let path = config.storage_path.join("held.db");
    let holder = LegacyHolder::spawn(&path);
    let result = TursoConnectFactory
        .connect(path.clone(), shipping_options(&config))
        .await;
    holder.release();

    match result {
        Ok(_) => panic!(
            "45.4/45.8: a live flag-off holder must refuse a writable multiprocess open through \
             the unmodified seam (no call-site change is allowed to be needed)"
        ),
        Err(StoreError::OpenBlocked { db_path, direction }) => {
            assert_eq!(
                db_path, path,
                "the refusal names the database file it refused"
            );
            assert_eq!(
                direction,
                OpenDirection::LegacyAuthority,
                "the direction is the legacy holder the probe saw"
            );
            assert!(
                StoreError::OpenBlocked {
                    db_path: path.clone(),
                    direction,
                }
                .to_string()
                .contains(path.to_string_lossy().as_ref()),
                "the Display text names the database file"
            );
        }
        Err(other) => panic!("the refusal must be the typed `OpenBlocked`, got {other:?}"),
    }
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// The inversion arm, automated (O6): **the same mixed configuration**, same live
/// child holder, with only `allow_unguarded_open` flipped. With the override on **no
/// probe runs at all** — neither the pre-open probe in `preflight` nor the post-open
/// re-probe in `postflight` (which returns early on `!guarded`) — and turso's
/// multiprocess open takes no lock on the database file (A2 / `t1`), so success here
/// is exactly the state that makes the control's `Err` attributable to **our** probe
/// rather than to the engine.
#[cfg(unix)]
#[tokio::test]
async fn test_override_opens_the_same_mixed_configuration_the_guard_refuses() {
    let config = common::store_config("guard-override-opens");
    let path = config.storage_path.join("held.db");
    let holder = LegacyHolder::spawn(&path);

    let mut options = shipping_options(&config);
    options.allow_unguarded_open = true;
    let handle = TursoConnectFactory.connect(path.clone(), options).await;
    let served = match &handle {
        Ok(handle) => handle
            .query("SELECT 1 AS one", Vec::new())
            .await
            .ok()
            .and_then(|rows| rows.first().and_then(|row| row.i64("one"))),
        Err(_) => None,
    };
    holder.release();

    assert!(
        handle.is_ok(),
        "45.8/O4: with the override active the same configuration must open: {:?}",
        handle.as_ref().err()
    );
    assert_eq!(
        served,
        Some(1),
        "the overridden handle must actually serve (`SELECT 1`)"
    );
    drop(handle);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// O7's vacuity proof: a live **multiprocess** handle holds no OS lock on the
/// database file, so probe B answers `Free` against our own handle. This is why
/// `45.6` needs no clearance registry.
#[cfg(unix)]
#[tokio::test]
async fn test_probe_reports_free_against_this_process_own_live_multiprocess_handle() {
    let config = common::store_config("guard-own-live-handle");
    let path = config.storage_path.join("own.db");
    let handle = turso_connect_factory()
        .connect(path.clone(), shipping_options(&config))
        .await
        .expect("open through the guarded seam");
    let outcome = probe_database_file(&path);
    println!("probe against our own live multiprocess handle = {outcome:?}");
    assert_eq!(
        outcome,
        ProbeOutcome::Free,
        "O7: a multiprocess handle holds no lock on the database file, so probe B cannot see it — \
         that is the proof `45.6` rests on"
    );
    drop(handle);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// 45.14 / A5 — the mode marker lives in a file we own; `.tshm` is untouched
// ---------------------------------------------------------------------------

/// `(name, byte length)` for every regular file directly under `dir`, sorted —
/// the file-set observable the `.tshm` test compares across a probe.
fn file_inventory(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut entries: Vec<(String, u64)> = std::fs::read_dir(dir)
        .expect("read the storage root")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((
                entry.file_name().to_string_lossy().into_owned(),
                metadata.len(),
            ))
        })
        .collect();
    entries.sort();
    entries
}

/// 45.14 / Plan Gate B5: `.tshm` is byte-identical across a guarded open.
///
/// Shape is deliberate: **both** snapshots are taken with the **same handle
/// live**, and only [`probe_database_file`] runs between them. A full open/close
/// cycle writes turso's own state into `.tshm` (magic/version header, owner
/// records, resize), so a cycle-shaped test could not attribute a difference to
/// the probe. It reproduces the measurement it stands in for (`t2 S5`).
#[cfg(unix)]
#[tokio::test]
async fn test_guarded_open_leaves_tshm_byte_identical() {
    let config = common::store_config("guard-tshm-identical");
    let path = config.storage_path.join("tshm.db");
    let tshm = std::path::PathBuf::from(format!("{}-tshm", path.display()));

    let handle = turso_connect_factory()
        .connect(path.clone(), shipping_options(&config))
        .await
        .expect("open through the guarded seam");
    handle
        .query("SELECT 1", Vec::new())
        .await
        .expect("the live handle answers, so `.tshm` is fully materialised");

    let before_bytes = std::fs::read(&tshm).expect("the live multiprocess handle owns a `.tshm`");
    let before_files = file_inventory(&config.storage_path);
    let outcome = probe_database_file(&path);
    let after_bytes = std::fs::read(&tshm).expect("the `.tshm` still exists after the probe");
    let after_files = file_inventory(&config.storage_path);

    println!(
        "tshm before_len={} after_len={} identical={} outcome={outcome:?}",
        before_bytes.len(),
        after_bytes.len(),
        before_bytes == after_bytes
    );
    assert_eq!(
        outcome,
        ProbeOutcome::Free,
        "the probe must have actually run (and answered) between the snapshots"
    );
    assert_eq!(
        before_bytes, after_bytes,
        "45.14: the probe must not touch turso's `.tshm` — a difference here is attributable to \
         the probe because both snapshots were taken with the same handle live"
    );
    assert_eq!(
        before_files, after_files,
        "the file set must be unchanged across the probe"
    );
    drop(handle);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// 45.14 / A5: the marker is `root/.wal-mode`, it names this process and the
/// multiprocess mode, and it is **not** inside `.tshm` or the database.
#[cfg(unix)]
#[tokio::test]
async fn test_mode_marker_is_written_in_the_storage_root_and_absent_from_tshm_and_the_database() {
    let config = common::store_config("guard-marker-location");
    let path = config.storage_path.join("marker.db");
    let handle = turso_connect_factory()
        .connect(path.clone(), shipping_options(&config))
        .await
        .expect("open through the guarded seam");
    let _ = handle.query("SELECT 1", Vec::new()).await.expect("serve");

    let marker = config
        .storage_path
        .join(mem_store::open_guard::WAL_MODE_MARKER_FILE);
    assert!(
        marker.is_file(),
        "A5: the marker must live in the storage root we own ({})",
        marker.display()
    );
    let body = std::fs::read_to_string(&marker).expect("read the marker");
    let parsed: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(&body).expect("the marker body is JSON");
    assert_eq!(
        parsed.get("pid").and_then(|pid| pid.as_u64()),
        Some(u64::from(std::process::id())),
        "the marker names the writing process: {body}"
    );
    assert_eq!(
        parsed.get("mode").and_then(|mode| mode.as_str()),
        Some("multiprocess"),
        "the marker records the mode it was written under: {body}"
    );
    #[cfg(target_os = "linux")]
    assert!(
        parsed
            .get("start_time")
            .and_then(|token| token.as_u64())
            .is_some(),
        "Linux observes a start-time token, so the marker must carry it: {body}"
    );

    let needle = b"\"mode\":\"multiprocess\"";
    let tshm = std::path::PathBuf::from(format!("{}-tshm", path.display()));
    for (label, bytes) in [
        (
            "the `.tshm`",
            std::fs::read(&tshm).expect("the live handle owns a `.tshm`"),
        ),
        (
            "the database file",
            std::fs::read(&path).expect("read the database file"),
        ),
    ] {
        assert!(
            !bytes.windows(needle.len()).any(|window| window == needle),
            "A5: the marker must not be written into {label}"
        );
    }
    drop(handle);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// 45.5 / O5 — the config key, end to end (issue 49's reader rule)
// ---------------------------------------------------------------------------

/// A resolved `mem_core` config from a jsonc fixture (the only knob this file
/// needs is `allowUnguardedOpen`; everything else is the shipped default).
fn core_config(json: &str) -> mem_core::config::Config {
    let project = common::temp_dir("guard-config-project");
    let home = common::temp_dir("guard-config-home");
    std::fs::create_dir_all(project.join(".opencode")).expect("create the project config dir");
    std::fs::write(project.join(".opencode/opencode-mem.jsonc"), json)
        .expect("write the project config");
    let paths = mem_core::config::ConfigPaths::for_project(&project, home);
    mem_core::config::Config::load(&paths, &std::collections::BTreeMap::new())
        .expect("the fixture config resolves")
}

/// O5 + issue 49's rule: a parsed-but-unread key is a review failure, so this
/// asserts the **reader chain** — `Config::load` → `from_core` →
/// `ConnectOptions::for_config`.
#[test]
fn test_config_key_allow_unguarded_open_reaches_the_connect_options() {
    let enabled = core_config(r#"{"allowUnguardedOpen": true}"#);
    let store = mem_store::config::from_core(&enabled);
    let options = ConnectOptions::for_config(&store, Platform::current());
    assert!(
        options.allow_unguarded_open,
        "the resolved config's `allowUnguardedOpen: true` must reach ConnectOptions"
    );

    let defaulted = core_config("{}");
    let store = mem_store::config::from_core(&defaulted);
    let options = ConnectOptions::for_config(&store, Platform::current());
    assert!(
        !options.allow_unguarded_open,
        "the default (false) must reach ConnectOptions too"
    );
}

/// The whole chain, not just the field: config `true` lets a **legacy** open
/// through (which the guard otherwise refuses with `UnsupportedOpenMode`);
/// config `false` refuses it.
#[cfg(unix)]
#[tokio::test]
async fn test_config_key_bypasses_the_guard_for_a_legacy_open() {
    let enabled = core_config(r#"{"allowUnguardedOpen": true}"#);
    let mut store = mem_store::config::from_core(&enabled);
    // The resolved config's `storage_path` is the user-level default; this test
    // owns a throwaway root (never a real one).
    store.storage_path = common::temp_dir("guard-config-bypass");
    let path = store.storage_path.join("legacy-bypass.db");
    let overridden = TursoConnectFactory
        .connect(path.clone(), legacy_options(&store))
        .await;
    match &overridden {
        Ok(handle) => {
            let served = handle
                .query("SELECT 1 AS one", Vec::new())
                .await
                .expect("the overridden legacy handle answers");
            assert_eq!(served.first().and_then(|row| row.i64("one")), Some(1));
        }
        Err(error) => panic!(
            "config `allowUnguardedOpen: true` must bypass the legacy refusal, got {error:?}"
        ),
    }
    drop(overridden);

    let defaulted = core_config("{}");
    let mut store = mem_store::config::from_core(&defaulted);
    store.storage_path = common::temp_dir("guard-config-refused");
    let refused = TursoConnectFactory
        .connect(
            store.storage_path.join("legacy-refused.db"),
            legacy_options(&store),
        )
        .await;
    match refused {
        Err(StoreError::UnsupportedOpenMode { .. }) => {}
        Ok(_) => panic!("config `false` must refuse the legacy option set, got a successful open"),
        Err(other) => {
            panic!("config `false` must refuse with `UnsupportedOpenMode`, got {other:?}")
        }
    }
    let _ = common::cleanup_temp_dir(&store.storage_path);
}

/// Marker concurrency (defect found by the workspace battery, 2026-10-25): eight
/// parallel opens in **one** root whose marker is stale must all succeed.
///
/// The original protocol used `create_new` + write, so a concurrent opener could
/// read the marker between creation and the body write, call the empty file stale,
/// unlink it and race the recreation — producing `EEXIST` (errno 17) and a
/// spurious `OpenBlocked` (measured in the mem-bridge `vault_feeder` suite). The
/// marker is now written to a staging file and `rename`d, which is atomic.
///
/// The stale marker names pid 0, which `decide_liveness` answers `Dead` on every
/// host, so every thread takes the reclaim path — the exact race.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_parallel_opens_in_one_root_reclaim_a_stale_marker_without_refusing() {
    let config = common::store_config("guard-parallel-marker");
    std::fs::write(
        config
            .storage_path
            .join(mem_store::open_guard::WAL_MODE_MARKER_FILE),
        r#"{"pid":0,"timestamp":"2026-10-25T00:00:00.000Z","operation":null,"start_time":null,"mode":"multiprocess"}"#,
    )
    .expect("write the stale marker");

    let mut tasks = Vec::new();
    for index in 0..8 {
        let config = config.clone();
        tasks.push(tokio::spawn(async move {
            turso_connect_factory()
                .connect(
                    config.storage_path.join(format!("parallel-{index}.db")),
                    shipping_options(&config),
                )
                .await
        }));
    }
    for (index, task) in tasks.into_iter().enumerate() {
        let result = task.await.expect("the open task joins");
        assert!(
            result.is_ok(),
            "parallel open {index} in one root must not be refused (a stale marker is \
             reclaimable, and the rewrite is atomic): {:?}",
            result.err()
        );
    }
    let body = std::fs::read_to_string(
        config
            .storage_path
            .join(mem_store::open_guard::WAL_MODE_MARKER_FILE),
    )
    .expect("read the reclaimed marker");
    assert!(
        body.contains(&std::process::id().to_string()),
        "the final marker must name this process: {body}"
    );
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

// ---------------------------------------------------------------------------
// Review round 2026-10-26 — RC3 through the manager, and 45.9's missing proof
// ---------------------------------------------------------------------------

/// RC3 at the public seam: a **marker** failure must reach the caller as the typed
/// `OpenBlocked`, not as a `StoreError::Turso` that names neither the marker nor the
/// override.
///
/// Shape: a directory where the marker file belongs (`.wal-mode`), so the marker read
/// fails `EISDIR` — the exact fixture the diagnosis used. Before the fix the caller
/// saw `Turso { message: "Is a directory (os error 21)" }` and
/// `is_open_guard_refusal() == false`.
#[cfg(unix)]
#[tokio::test]
async fn test_manager_surfaces_a_marker_failure_as_the_typed_refusal() {
    use mem_store::connection::ConnectionManager;
    use std::sync::Arc;

    let config = common::store_config("guard-manager-marker-failure");
    std::fs::create_dir_all(
        config
            .storage_path
            .join(mem_store::open_guard::WAL_MODE_MARKER_FILE),
    )
    .expect("a directory where the marker file belongs");
    let path = config.storage_path.join("marker.db");
    let manager = ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory) as Arc<dyn ConnectFactory>,
    );

    let result = manager.get_connection(&path).await;
    match result {
        Ok(_) => panic!("a marker failure must refuse the open"),
        Err(error) => {
            assert!(
                error.is_open_guard_refusal(),
                "RC3: the manager must not launder a marker failure into `StoreError::Turso`: \
                 {error:?}"
            );
            match error {
                StoreError::OpenBlocked { direction, .. } => assert!(
                    matches!(direction, mem_store::error::OpenDirection::Unknown { .. }),
                    "the marker failure carries the `Unknown` direction: {direction:?}"
                ),
                other => panic!("the refusal must be `OpenBlocked`, got {other:?}"),
            }
        }
    }
    // The Display must be actionable at the default (encryption-off) branch.
    let error = StoreError::OpenBlocked {
        db_path: path.clone(),
        direction: mem_store::error::OpenDirection::Unknown { errno: Some(21) },
    };
    let rendered = error.to_string();
    for needle in [
        mem_store::open_guard::WAL_MODE_MARKER_FILE,
        mem_store::open_guard::ALLOW_UNGUARDED_OPEN_ENV,
        mem_store::open_guard::allow_unguarded_open_key(),
    ] {
        assert!(
            rendered.contains(needle),
            "the Display must name {needle:?}: {rendered}"
        );
    }
    let _ = common::cleanup_temp_dir(&config.storage_path);
}

/// **45.9's missing proof** — not a regression test: it is green by construction
/// today, because `purge-session` builds its stores through the same
/// `ConnectionManager` → `TursoConnectFactory` seam as the engine. What was missing
/// was the *evidence*, and this is it.
///
/// The option shape is exactly what `mem-bridge/src/purge.rs:309-311` builds:
/// `mem_store::config::from_core(&resolved_config)` with the storage root pointing at
/// the same root the engine uses.
#[cfg(unix)]
#[tokio::test]
async fn test_purge_session_option_set_stays_multiprocess() {
    use mem_store::open_guard::{WalMode, Writability};

    // Mirrors `mem-bridge/src/purge.rs:309-311` (its `from_core` + storage root): if
    // that shape changes, this test must change with it.
    let mut store = mem_store::config::from_core(&core_config("{}"));
    store.storage_path = common::temp_dir("guard-purge-options");
    let options = ConnectOptions::for_config(&store, Platform::current());
    assert_eq!(
        WalMode::from_options(&options),
        WalMode::Multiprocess,
        "45.9: purge-session's option set is a multiprocess option set (so probe B applies \
         and the guard cannot refuse it as legacy)"
    );
    assert_eq!(
        Writability::from_options(&options),
        Writability::Writable,
        "45.9: purge writes, so it is a writable open — the guarded cell"
    );
    assert!(
        !options.allow_unguarded_open,
        "the default config must not carry the override"
    );

    // Behavioural half: the engine-shaped handle holds the same database, and the
    // purge-shaped open must still be `Ok` (probe B answers Free against our own
    // live multiprocess handle — O7).
    let path = store.storage_path.join("episodic.db");
    let engine_handle = turso_connect_factory()
        .connect(
            path.clone(),
            ConnectOptions::for_config(&store, Platform::current()),
        )
        .await
        .expect("the engine-shaped open");
    let purge_open = turso_connect_factory()
        .connect(
            path,
            ConnectOptions::for_config(&store, Platform::current()),
        )
        .await;
    assert!(
        purge_open.is_ok(),
        "45.9: the purge-shaped open must not be refused while the engine holds the same \
         database (that is the configuration multiprocess WAL exists to serve): {:?}",
        purge_open.err()
    );
    drop(purge_open);
    drop(engine_handle);
    let _ = common::cleanup_temp_dir(&store.storage_path);
}

// ---------------------------------------------------------------------------
// Round 4, item 3 — the probe releases an in-process legacy lock (PINNED residual)
// ---------------------------------------------------------------------------

/// **Pins a documented residual — it does not endorse it.**
///
/// An overridden legacy handle is a legitimate in-process legacy opener
/// (`allow_unguarded_open` is a public per-call field). POSIX record locks are owned
/// by the *process*, so the probe's own open+close releases this process's lock on
/// that file (`t2 S4/S6/S7`): `Held` while the handle is live, `Free` on the next
/// probe, and a guarded writable multiprocess open then **proceeds** while the legacy
/// handle is still live — the mixed-mode row loss the guard exists to prevent.
///
/// If design A §11's identity-keyed registry lands (or the override is constrained to
/// the process-wide env/config legs only), this test MUST fail: that is the point —
/// it forces the decision instead of letting the behaviour change silently.
///
/// **Linux only, because `Held` is a Linux fact here.** The first probe must conflict
/// with a lock the *same process* holds, which only an OFD lock does: on another Unix
/// the probe is `F_SETLK`, and a POSIX record lock never conflicts with a lock its own
/// process holds, so the first probe would read `Free` and this test would be asserting
/// a platform-specific outcome under a broader `cfg`. The hazard itself is if anything
/// more direct there (the guarded open proceeds without the probe ever seeing the
/// holder); it is simply not what this test pins.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_probe_releases_an_in_process_overridden_legacy_lock_and_the_guarded_open_then_proceeds()
 {
    let config = common::store_config("guard-in-process-legacy");
    let path = config.storage_path.join("legacy.db");

    let mut legacy = ConnectOptions::for_config(&config, Platform::current());
    legacy
        .experimental
        .retain(|feature| !matches!(feature, ExperimentalFeature::MultiprocessWal));
    legacy.allow_unguarded_open = true;
    let handle = turso_connect_factory()
        .connect(path.clone(), legacy)
        .await
        .expect("the overridden legacy handle opens");
    handle
        .query("SELECT 1", Vec::new())
        .await
        .expect("the legacy handle serves");

    let while_live = probe_database_file(&path);
    let after_probe = probe_database_file(&path);
    println!(
        "probe with an in-process legacy handle live = {while_live:?}; next probe = {after_probe:?}"
    );
    assert_eq!(
        while_live,
        ProbeOutcome::Held,
        "the probe sees the lock the legacy handle holds"
    );
    assert_eq!(
        after_probe,
        ProbeOutcome::Free,
        "the probe's own open+close released this process's POSIX lock (the residual)"
    );

    let guarded = turso_connect_factory()
        .connect(path, shipping_options(&config))
        .await;
    assert!(
        guarded.is_ok(),
        "PINNED RESIDUAL: the guarded open currently proceeds although an in-process legacy handle is \
         live — if this now refuses, a remedy landed and this test must be updated deliberately: {:?}",
        guarded.err()
    );
    drop(guarded);
    drop(handle);
    let _ = common::cleanup_temp_dir(&config.storage_path);
}
