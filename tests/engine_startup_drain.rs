//! The STARTUP capture drain must never gate the `hello` handshake.
//!
//! Measured defect: `crates/mem-bridge/src/engine.rs` awaited
//! `runtime.capture.drain_once()` before the stdio loop, so the handshake
//! could not be answered until the replay finished. With four queued captures
//! behind a self-hosted model (5–30 s per call) the engine answered `hello`
//! after 90 s+, the wrapper's `HELLO_TIMEOUT_MS = 5000`
//! (`plugin/src/plugin.js:60`) expired, and the whole memory tool degraded.
//!
//! These are PROCESS-LEVEL oracles, along `tests/engine_shutdown.rs`'s
//! precedent: `CARGO_BIN_EXE_opencode-agents-memory` is only defined for the
//! package that declares the binary. The capture provider is a loopback
//! fixture (never a network service, never a secret) behind the documented
//! manual-provider config block, and the queued capture is seeded directly
//! into a private storage root's durable inbox — no live store is touched.
//!
//! What each row pins:
//!
//! * (a) `hello` answers in well under the wrapper's deadline while the drain
//!   is demonstrably in flight (the fixture has accepted the provider call).
//! * (d) stdin EOF still exits inside the D-45 budget with that drain blocked,
//!   and the unfinished replay stays durable for the next start.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mem_bridge::capture::CaptureParams;
use mem_episodic::inbox::{CaptureInbox, InboxNewEntry};
use mem_episodic::store::EpisodicStore;

const HELLO_LINE: &str = r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#;

/// The wrapper's own handshake deadline is 5 s; the handshake must not spend
/// the drain's time at all, so the bound here is a wide order of magnitude
/// under it and still generous for a cold process.
const HELLO_BOUND: Duration = Duration::from_secs(1);

/// How long the engine may take to compose, launch and reach the drain.
const STARTUP_BOUND: Duration = Duration::from_secs(20);

/// D-45's total shutdown budget is 2 s; the process-level oracle allows
/// headroom for spawn/exit, exactly as `tests/engine_shutdown.rs` does.
const EXIT_BOUND: Duration = Duration::from_secs(6);

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "root-drain-{prefix}-{}-{nanos}",
        std::process::id()
    ));
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

// ---------------------------------------------------------------------------
// The loopback provider fixture
// ---------------------------------------------------------------------------

/// A provider endpoint that ACCEPTS the capture-summarizer call and never
/// answers. The engine's drain is then demonstrably in flight (its HTTP
/// request is open) for as long as the test needs it; request timeouts are
/// minutes away, so nothing depends on a real completion.
struct HangingProvider {
    port: u16,
    accepted: Arc<AtomicUsize>,
}

impl HangingProvider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the provider fixture");
        let port = listener.local_addr().expect("provider addr").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        std::thread::spawn(move || {
            // Held, never answered: dropping a stream would look like EOF to
            // the client and end the "still in flight" state.
            let mut held: Vec<TcpStream> = Vec::new();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                held.push(stream);
            }
        });
        Self { port, accepted }
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn wait_for_a_connection(&self) {
        let started = Instant::now();
        while self.accepted() == 0 {
            assert!(
                started.elapsed() < STARTUP_BOUND,
                "the startup drain never reached the provider within {STARTUP_BOUND:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

// ---------------------------------------------------------------------------
// The private engine installation
// ---------------------------------------------------------------------------

/// The documented manual capture-provider block, pointed at the fixture.
/// The key is a wiring proof, never a credential.
fn write_config(home: &Path, provider_port: u16) {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create the private config dir");
    std::fs::write(
        dir.join("opencode-mem.json"),
        format!(
            r#"{{
  "memoryProvider": "openai-chat",
  "memoryModel": "drain-fixture-model",
  "memoryApiUrl": "http://127.0.0.1:{provider_port}/v1",
  "memoryApiKey": "drain-fixture-not-a-credential"
}}"#
        ),
    )
    .expect("write the private config");
}

/// The store config the engine derives for this private installation: the
/// documented defaults with only the root overridden. The inbox schema is
/// dimension-independent, but the values still mirror `mem_core`'s DEFAULTS
/// so the seeding open and the engine's open are byte-compatible.
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

fn seed_runtime() -> mem_store::tokio::runtime::Runtime {
    mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the seeding runtime")
}

/// Queues ONE capture with NO supplied summary, so a drain must call the
/// provider — the exact shape that held the handshake.
fn seed_queued_capture(storage_root: &Path) {
    seed_runtime().block_on(async {
        let store = Arc::new(EpisodicStore::new(
            storage_root.to_path_buf(),
            storage_config(storage_root),
        ));
        store.ready().await.expect("the seeded store opens");
        let params = CaptureParams {
            session_id: "sess-drain".to_string(),
            capture_id: "019a0000-0000-7f00-8000-00000000d4a1".to_string(),
            project_id: "/srv/engine-drain-project".to_string(),
            agent_id: "agent-drain".to_string(),
            transcript: Some("fix the drain handshake ordering".to_string()),
            ..CaptureParams::default()
        };
        CaptureInbox::new(store)
            .enqueue(InboxNewEntry {
                capture_id: params.capture_id.clone(),
                project_id: params.project_id.clone(),
                session_id: params.session_id.clone(),
                agent_id: params.agent_id.clone(),
                payload: mem_store::serde_json::to_value(&params).expect("capture payload"),
            })
            .await
            .expect("queue the durable capture");
    });
}

/// The queued count AFTER the engine exited: an unfinished replay must still
/// be durable (the next start's replay picks it up).
fn queued_captures(storage_root: &Path) -> u64 {
    seed_runtime().block_on(async {
        let store = Arc::new(EpisodicStore::new(
            storage_root.to_path_buf(),
            storage_config(storage_root),
        ));
        store.ready().await.expect("the reopened store opens");
        CaptureInbox::new(store)
            .queued_count()
            .await
            .expect("queued count")
    })
}

// ---------------------------------------------------------------------------
// The spawned engine + its wire
// ---------------------------------------------------------------------------

/// A child engine KILLED on drop (a panic mid-test never leaks a live engine
/// into the next test's `/proc` probes).
struct Engine {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
}

impl Engine {
    /// Spawns the packaged engine over a private `HOME` and storage root.
    /// `env_clear()` + the explicit set is the isolation precedent from
    /// `tests/engine_capture_provider.rs`: the developer's live config and
    /// log are never read or written.
    fn spawn(home: &Path, storage_root: &Path) -> Self {
        let stderr = std::fs::File::create(storage_root.join("engine.err"))
            .expect("create the stderr capture file");
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
        // A dedicated reader: the NDJSON responses arrive in order and the
        // test thread only ever does bounded `recv_timeout`s, so a missing
        // response is an assertion failure rather than a wedged suite.
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

// ---------------------------------------------------------------------------
// (a) `hello` never waits for the startup drain
// ---------------------------------------------------------------------------

#[test]
fn test_hello_answers_while_the_startup_drain_is_blocked_on_the_provider() {
    let root = temp_dir("hello");
    let home = root.join("home");
    let storage = root.join("storage");
    std::fs::create_dir_all(&home).expect("create the private home");
    std::fs::create_dir_all(&storage).expect("create the storage root");

    let provider = HangingProvider::start();
    write_config(&home, provider.port());
    seed_queued_capture(&storage);

    let mut engine = Engine::spawn(&home, &storage);
    // The drain has reached the provider: it is IN FLIGHT before `hello` is
    // even written, so a fast answer cannot be explained by an empty inbox.
    provider.wait_for_a_connection();

    let started = Instant::now();
    engine.send(HELLO_LINE);
    let hello = engine.recv_within(HELLO_BOUND).unwrap_or_else(|| {
        panic!(
            "`hello` must never wait for the startup drain: no response within {HELLO_BOUND:?} \
             while the drain is in flight (the wrapper's deadline is 5 s)"
        )
    });
    assert!(
        started.elapsed() < HELLO_BOUND,
        "the handshake was answered inside {HELLO_BOUND:?}"
    );
    assert!(
        hello.contains(r#""protocol":"1.0.0""#),
        "the handshake answered the protocol line: {hello}"
    );
    assert!(
        provider.accepted() >= 1,
        "the provider call was open while `hello` was answered"
    );
}

// ---------------------------------------------------------------------------
// (d) shutdown stays clean with that drain still blocked
// ---------------------------------------------------------------------------

#[test]
fn test_shutdown_with_a_blocked_startup_drain_exits_and_keeps_the_durable_row() {
    let root = temp_dir("shutdown");
    let home = root.join("home");
    let storage = root.join("storage");
    std::fs::create_dir_all(&home).expect("create the private home");
    std::fs::create_dir_all(&storage).expect("create the storage root");

    let provider = HangingProvider::start();
    write_config(&home, provider.port());
    seed_queued_capture(&storage);

    let mut engine = Engine::spawn(&home, &storage);
    provider.wait_for_a_connection();
    // EOF while the drain is still stuck in the provider call.
    engine.close_stdin();
    let status = engine.wait_bounded(EXIT_BOUND).unwrap_or_else(|| {
        panic!(
            "the engine must exit inside the D-45 budget with the startup drain still in \
             flight; it did not within {EXIT_BOUND:?}"
        )
    });
    assert_eq!(status.code(), Some(0), "EOF is a clean exit");
    assert_eq!(
        queued_captures(&storage),
        1,
        "the drain's unfinished work must stay durable for the next start"
    );
}
