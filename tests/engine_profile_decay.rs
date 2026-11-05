//! Issue 90 — the composed engine actually runs the profile decay sweep.
//!
//! `ProfileStore::sweep_decay_daily` was implemented, unit-tested and never
//! invoked in production. `run_in_runtime` now spawns the decay lifecycle; this
//! PROCESS-LEVEL oracle (the `tests/engine_startup_drain.rs` precedent —
//! `CARGO_BIN_EXE_opencode-agents-memory` exists only for the package that
//! declares the binary) is what fails when that call is removed from the
//! composition while every in-process test stays green.
//!
//! A profile is seeded into a PRIVATE storage root's `user-profiles.db`, the
//! seeding store is dropped (the engine takes `.engine.lock` and opens
//! guarded), the engine is spawned over a private `HOME`, and we wait — bounded
//! — for the named `profile.decay_tick` event. After a clean EOF exit the row
//! is read back. No live store, config or log is touched.
//!
//! What `hello` proves here is liveness only: a one-profile sweep is too fast
//! for a wall-clock bound to separate "awaited" from "scheduled", so the
//! ordering claim lives in `crates/mem-bridge/tests/profile_decay_lifecycle.rs`
//! (`test_startup_tick_is_scheduled_not_awaited`).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileItem, ProfileStore};

const HELLO_LINE: &str = r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#;

const DAY_MS: i64 = 86_400_000;

/// How long the engine may take to compose, launch and run its startup tick.
const TICK_BOUND: Duration = Duration::from_secs(20);
const TICK_POLL: Duration = Duration::from_millis(50);

/// D-45's total shutdown budget is 2 s; headroom for spawn/exit.
const EXIT_BOUND: Duration = Duration::from_secs(6);

/// Scratch lives under `<project-root>/.tmp/scratch/engine-profile-decay/`.
fn scratch(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join("scratch")
        .join("engine-profile-decay")
        .join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the scratch root");
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// The store config the engine derives for this private installation: the
/// documented defaults with only the root overridden.
fn storage_config(storage_root: &Path) -> mem_store::config::StoreConfig {
    mem_store::config::StoreConfig {
        storage_path: storage_root.to_path_buf(),
        embedding_dimensions: 768,
        embedding_model: "Xenova/nomic-embed-text-v1".to_string(),
        max_vectors_per_shard: 50_000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

fn runtime() -> mem_store::tokio::runtime::Runtime {
    mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the helper runtime")
}

fn profile_store(storage_root: &Path) -> ProfileStore {
    ProfileStore::new(
        storage_root.to_path_buf(),
        storage_config(storage_root),
        ProfileConfig::default(),
    )
}

/// One preference: confidence 0.8, frequency 1, three pieces of evidence, a
/// 5:1 Beta posterior, last seen 90 days ago.
fn seed_profile(storage_root: &Path) -> String {
    runtime().block_on(async {
        let store = profile_store(storage_root);
        let now = mem_profile::ids::now_millis();
        let data = ProfileData {
            preferences: vec![ProfileItem {
                description: "decay me".to_string(),
                category: Some("style".to_string()),
                confidence: Some(0.8),
                frequency: Some(1),
                evidence: Some(vec!["e0".into(), "e1".into(), "e2".into()]),
                last_seen: Some(now - 90 * DAY_MS),
                alpha: Some(5.0),
                beta: Some(1.0),
                ..ProfileItem::default()
            }],
            patterns: Vec::new(),
            workflows: Vec::new(),
            learning_paths: None,
        };
        store
            .create_profile(
                "decay@example.com",
                "Unknown",
                "unknown",
                "decay@example.com",
                &data,
                7,
            )
            .await
            .expect("seed the profile")
        // `store` drops here, before the engine is spawned.
    })
}

struct Stored {
    version: i64,
    confidence: f64,
    changelog_summaries: Vec<String>,
}

fn read_profile(storage_root: &Path, id: &str) -> Stored {
    runtime().block_on(async {
        let store = profile_store(storage_root);
        let row = store
            .get_profile_by_id(id)
            .await
            .expect("read the profile")
            .expect("the profile exists");
        let data: ProfileData =
            mem_store::serde_json::from_str(&row.profile_data).expect("parse the profile");
        let changelogs = store
            .get_profile_changelogs(id, 10)
            .await
            .expect("read the changelog");
        Stored {
            version: row.version,
            confidence: data.preferences[0].confidence.expect("confidence stored"),
            changelog_summaries: changelogs.into_iter().map(|c| c.change_summary).collect(),
        }
    })
}

/// A child engine KILLED on drop (a panic mid-test never leaks a live engine).
struct Engine {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    stderr_path: PathBuf,
}

impl Engine {
    /// Spawns the packaged engine over a private `HOME` and storage root:
    /// `env_clear()` + the explicit set, so the developer's live config and log
    /// are never read or written. Stderr goes to a file, never a pipe.
    fn spawn(home: &Path, storage_root: &Path) -> Self {
        let stderr_path = storage_root.join("engine.err");
        let stderr = std::fs::File::create(&stderr_path).expect("create the stderr capture file");
        let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
        command
            .arg("--mode=stdio")
            .arg(format!("--storage-root={}", storage_root.display()))
            .arg(format!("--web-port={}", free_port()))
            .env_clear()
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("OPENCODE_MEM_WEB_HOST", "127.0.0.1")
            .env(
                "OPENCODE_MEM_LOG_FILE",
                storage_root.join("opencode-mem.log"),
            )
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        let mut child = command.spawn().expect("the engine binary spawns");
        let stdin = child.stdin.take().expect("engine stdin");
        let stdout = child.stdout.take().expect("engine stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child: Some(child),
            stdin: Some(stdin),
            lines,
            stderr_path,
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("engine stdin open");
        stdin.write_all(line.as_bytes()).expect("write request");
        stdin.write_all(b"\n").expect("write NDJSON terminator");
        stdin.flush().expect("flush request");
    }

    fn recv_within(&self, bound: Duration) -> Option<String> {
        self.lines.recv_timeout(bound).ok()
    }

    fn stderr_text(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_bounded(&mut self, bound: Duration) -> Option<ExitStatus> {
        let child = self.child.as_mut().expect("engine alive");
        let started = Instant::now();
        while started.elapsed() < bound {
            if let Some(status) = child.try_wait().expect("try_wait") {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn test_spawned_engine_decays_a_seeded_profile_confidence() {
    let root = scratch("decay");
    let home = root.join("home");
    let storage = root.join("storage");
    std::fs::create_dir_all(&home).expect("create the private home");
    std::fs::create_dir_all(&storage).expect("create the storage root");

    let id = seed_profile(&storage);
    let before = read_profile(&storage, &id);
    assert_eq!(before.version, 1, "the seeded profile starts at version 1");
    assert!(
        (before.confidence - 0.8).abs() < 1e-9,
        "the seeded profile starts at 0.8"
    );

    let mut engine = Engine::spawn(&home, &storage);
    engine.send(HELLO_LINE);
    let hello = engine
        .recv_within(TICK_BOUND)
        .expect("the engine answers hello");
    assert!(
        hello.contains(r#""protocol":"1.0.0""#),
        "the handshake answered the protocol line: {hello}"
    );

    // The named event, polled (20 s at 50 ms): the startup tick ran in the
    // spawned composition and updated the one seeded profile.
    let started = Instant::now();
    loop {
        let text = engine.stderr_text();
        if text.contains(r#""event":"profile.decay_tick""#)
            && text.contains(r#""profiles_updated":1"#)
        {
            break;
        }
        assert!(
            started.elapsed() < TICK_BOUND,
            "the spawned engine never emitted profile.decay_tick within {TICK_BOUND:?}:\n{text}"
        );
        std::thread::sleep(TICK_POLL);
    }

    engine.close_stdin();
    let status = engine
        .wait_bounded(EXIT_BOUND)
        .expect("the engine exits on EOF inside the budget");
    assert_eq!(status.code(), Some(0), "EOF is a clean exit");

    let after = read_profile(&storage, &id);
    assert_eq!(
        after.version, 2,
        "the sweep persisted through update_profile"
    );
    assert!(
        (after.confidence - 0.405).abs() < 1e-6,
        "the persisted confidence is in the decayed band (Issue 102 single derivation 0.405), got {}",
        after.confidence
    );
    assert!(
        after
            .changelog_summaries
            .iter()
            .any(|summary| summary == "Decay sweep: confidence decayed"),
        "the sweep writes the changelog row: {:?}",
        after.changelog_summaries
    );
}
