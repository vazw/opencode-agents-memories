//! Production-path guard for ADR-0028 (rework F1): the engine's STARTUP
//! selector, not just the selector function.
//!
//! `mem_bridge::engine::engine_capture_provider` decides manual-vs-host, and
//! commit `2bb7e57` shipped it with **zero production call sites**: every test
//! called the function directly while `run_in_runtime` still warmed and passed
//! the host provider, so the operator's manual block was dead code and the
//! `Auto-capture provider ready` line named the wrong provider. A unit row
//! cannot see the difference; only a SPAWNED engine can.
//!
//! Shape (along `tests/engine_shutdown.rs`'s precedent): a private `HOME`
//! carrying the config, a private `OPENCODE_MEM_LOG_FILE`, then assert the
//! sink names the manual provider and no host line. No network — the endpoint
//! is a loopback discard port and `autoCaptureEnabled` is false, so the
//! provider is composed and never dialled.

use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The one line that states which capture provider is live (D-28-3).
const READY: &str = "Auto-capture provider ready";
const UNAVAILABLE: &str = "Auto-capture provider unavailable";
const INSECURE: &str = "Auto-capture provider insecure endpoint";

/// A wiring proof, never a credential: it must still never reach the sink.
const WIRING_KEY: &str = "sk-wiring-proof-do-not-log";
const WIRING_MODEL: &str = "qwen3.8-flash-next-uncensored-nothink";
/// A loopback discard endpoint: allowed (`http://` on loopback is the
/// documented local-provider case) and never dialled.
const WIRING_URL: &str = "http://127.0.0.1:9/v1";

const STARTUP_BOUND: Duration = Duration::from_secs(30);
const EXIT_BOUND: Duration = Duration::from_secs(10);

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn scratch_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "engine-capture-provider-{prefix}-{}-{nanos}-{serial}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create the scratch dir");
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A spawned engine that is KILLED when this guard drops: a panic mid-test
/// never leaks a live engine into the next test's /proc probes.
struct Engine {
    child: Option<Child>,
}

impl Engine {
    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
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

/// The manual block the operator would write, minus the credential.
fn manual_config() -> String {
    format!(
        r#"{{
  "autoCaptureEnabled": false,
  "memoryProvider": "openai-chat",
  "memoryModel": "{WIRING_MODEL}",
  "memoryApiUrl": "{WIRING_URL}",
  "memoryApiKey": "{WIRING_KEY}"
}}"#
    )
}

/// Spawns the packaged engine over a private `HOME` and a private sink.
///
/// `env_clear()` + the explicit set is the isolation (the `--help` precedent in
/// `tests/workspace_smoke.rs`): the developer's live `~/.config/opencode`
/// config and `~/.opencode-mem` sink are never read or written.
fn spawn_engine(home: &Path, storage_root: &Path, log_file: &Path) -> Engine {
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
        .env("OPENCODE_MEM_LOG_FILE", log_file)
        // Any git probe the scope resolver makes stays possible; nothing else
        // of the caller's environment leaks in.
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr));
    Engine {
        child: Some(command.spawn().expect("the engine binary spawns")),
    }
}

/// Polls the sink until it carries a capture-provider line; fails fast if the
/// engine died before writing one.
fn wait_for_log(engine: &mut Engine, log_file: &Path, stderr_path: &Path) -> String {
    let started = Instant::now();
    loop {
        let text = std::fs::read_to_string(log_file).unwrap_or_default();
        if text.contains("Auto-capture provider") {
            return text;
        }
        if let Some(status) = engine.exited() {
            let stderr = std::fs::read_to_string(stderr_path).unwrap_or_default();
            panic!(
                "the engine exited ({status:?}) before any capture-provider line: sink {text:?} / stderr {stderr:?}"
            );
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "no capture-provider line within {STARTUP_BOUND:?}: sink {text:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// F1: the SPAWNED engine composes the configured manual provider, and the
/// authoritative ready line says so.
#[test]
fn test_spawned_engine_composes_the_configured_manual_provider() {
    let root = scratch_dir("manual");
    let home = root.join("home");
    let storage_root = root.join("storage");
    let log_file = root.join("opencode-mem.log");
    let config_dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&config_dir).expect("create the config dir");
    std::fs::create_dir_all(&storage_root).expect("create the storage root");
    std::fs::write(config_dir.join("opencode-mem.json"), manual_config())
        .expect("write the config");

    let mut engine = spawn_engine(&home, &storage_root, &log_file);
    let text = wait_for_log(&mut engine, &log_file, &storage_root.join("engine.err"));

    let ready: Vec<&str> = text.lines().filter(|line| line.contains(READY)).collect();
    assert_eq!(
        ready.len(),
        1,
        "exactly one authoritative ready line, got {ready:?} in: {text}"
    );
    assert!(
        ready[0].contains("\"providerId\":\"openai-chat\""),
        "the ready line names the provider the operator configured: {}",
        ready[0]
    );
    assert!(
        ready[0].contains(&format!("\"baseUrl\":\"{WIRING_URL}\"")),
        "the ready line names the configured endpoint: {}",
        ready[0]
    );
    assert!(
        !text.contains(UNAVAILABLE),
        "a complete manual block must not fail closed: {text}"
    );
    assert!(
        !text.contains("\"providerId\":\"opencode"),
        "the OpenCode host is not the capture provider here: {text}"
    );
    assert!(
        !text.contains(INSECURE),
        "loopback `http://` is the allowed local-provider case: {text}"
    );
    assert!(
        !text.contains(WIRING_KEY),
        "the resolved key never reaches the sink: {text}"
    );

    // EOF is a clean exit: the new startup path never wedges the engine.
    drop(engine.child.as_mut().and_then(|child| child.stdin.take()));
    let started = Instant::now();
    loop {
        if let Some(status) = engine.exited() {
            assert_eq!(status.code(), Some(0), "EOF is a clean exit");
            break;
        }
        assert!(
            started.elapsed() < EXIT_BOUND,
            "the engine did not exit within {EXIT_BOUND:?} of stdin EOF"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Drain the remaining pipe so the child is reaped without a zombie.
    if let Some(mut stdout) = engine.child.as_mut().and_then(|child| child.stdout.take()) {
        let mut rest = String::new();
        let _ = stdout.read_to_string(&mut rest);
    }
    let _ = std::fs::remove_dir_all(&root);
}
