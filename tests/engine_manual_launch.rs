//! Engine-silent-500 RED pin: a manually-launched standalone engine fed a
//! RELATIVE `--storage-root` must answer store-backed routes.
//!
//! Root cause (debug: `.tmp/progress/engine-silent-500.debug-1.md`):
//! `ShardManager::new` canonicalizes its storage path
//! (`crates/mem-store/src/shard_manager.rs:201`) while the `ConnectionManager`
//! keeps the raw relative value (`composition.rs:180-181`), so containment
//! (`assert_path_inside_storage`, `db.rs:591`) refuses the engine's own
//! `metadata.db` — absolute target vs relative root — and every
//! `ApiServices.memory.shards` route answers a silent 500 (`port_failure`
//! logs to a target the file sink never accepts). The wrapper is unaffected
//! (it passes an absolute root, MEASURED `/proc` cmdline); this suite pins
//! the manual launch shape.
//!
//! Mutation contract: reverting ONLY the composition-root normalization
//! (`src/main.rs`, Fix A) turns
//! `test_relative_storage_root_answers_store_backed_routes` RED with exactly
//! today's symptom — `/api/health` 200, `/api/tags` 500.
//!
//! House rules (mirroring tests/production_api_wiring.rs): scratch under the
//! repo's gitignored `.tmp/scratch`, serial mutex, kill-and-reap on drop,
//! child stdout/stderr to FILES, every wait bounded, the test process
//! environment is never mutated (the child gets `HOME` through `Command::env`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const SOCKET_BOUND: Duration = Duration::from_secs(5);
const STARTUP_BOUND: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The serial harness: the test spawns a live engine process; a panic
/// mid-test would otherwise leak it into the next test's port probe.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A unique scratch directory under the repo's `.tmp/scratch` (never `/tmp`:
/// this host's tmpfs truncates files to zero under pressure), returned in
/// BOTH forms: absolute for the test's own file layout, and the RELATIVE form
/// (stripped against the package root) that the engine is fed — the argv form
/// that triggers the defect. `cargo test` runs with the cwd at the package
/// root, so the strip is always well-defined.
fn scratch_root(prefix: &str) -> (PathBuf, PathBuf) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let cwd = std::env::current_dir().expect("package root cwd");
    let absolute = cwd
        .join(".tmp")
        .join("scratch")
        .join("engine-manual-launch")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&absolute).expect("create scratch root");
    let relative = absolute
        .strip_prefix(&cwd)
        .expect("scratch lives under the package root")
        .to_path_buf();
    (absolute, relative)
}

/// An engine child that is killed AND reaped when the guard drops.
struct Guarded {
    child: Child,
}

impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A minimal shell: the launcher refuses to start without an `index.html`.
/// Never skip on a missing real `web-ui/dist` — dist readiness is exactly
/// this file, and `/api/*` never touches statics.
fn write_shell(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create dist dir");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body><div id=\"engine-s500\">manual launch fixture</div></body></html>",
    )
    .expect("write index.html");
}

/// Spawns the standalone engine with the RELATIVE storage-root form, its own
/// web port, a synthesized dist and a sandbox `HOME` (isolated config).
fn spawn_engine(
    relative_root: &Path,
    web_port: u16,
    dist: &Path,
    home: &Path,
    log_file: &Path,
    stderr_path: &Path,
) -> Guarded {
    let stderr = std::fs::File::create(stderr_path).expect("create stderr file");
    let stdout =
        std::fs::File::create(stderr_path.with_extension("out")).expect("create stdout file");
    let child = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg("--mode=standalone")
        .arg(format!("--storage-root={}", relative_root.display()))
        .arg(format!("--web-port={web_port}"))
        .arg(format!("--web-dist={}", dist.display()))
        // The dist is named by the CLI flag; an inherited env var must not
        // shadow it in either direction.
        .env_remove(opencode_agents_memory::web_dist::ENV_VAR)
        .env("HOME", home)
        // Isolate the process-global file log from the real home.
        .env("OPENCODE_MEM_LOG_FILE", log_file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("the engine binary spawns");
    Guarded { child }
}

/// The child's stderr so far — every failure message carries it, because the
/// silent-500 class is defined by NOTHING appearing there.
fn child_stderr(stderr_path: &Path) -> String {
    std::fs::read_to_string(stderr_path).unwrap_or_default()
}

/// Polls `TcpStream::connect` until the port accepts, within the startup bound.
fn wait_for_port(port: u16) {
    let started = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                drop(stream);
                return;
            }
            Err(error) => {
                assert!(
                    started.elapsed() < STARTUP_BOUND,
                    "the web port {port} never accepted a connection within {STARTUP_BOUND:?}: {error}"
                );
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// One bounded HTTP/1.1 request; `token` rides the only header the gate
/// accepts (`x-opencode-mem-token`). GET carries no body. Lenient: a
/// connect/read failure (server not yet fully up) returns `None` instead of
/// panicking, so startup polls can simply retry.
fn http_request(port: u16, path: &str, token: Option<&str>) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(SOCKET_BOUND)).ok()?;
    stream.set_write_timeout(Some(SOCKET_BOUND)).ok()?;
    let mut request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\n");
    if let Some(token) = token {
        request.push_str(&format!("x-opencode-mem-token: {token}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    stream.flush().ok()?;
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(_) => return None,
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())?;
    Some((status, text))
}

/// The body half of a raw HTTP response.
fn response_body(raw: &str) -> &str {
    raw.split("\r\n\r\n")
        .nth(1)
        .unwrap_or_else(|| panic!("no body in the raw response {raw:?}"))
}

/// Polls `/api/health` until 200 (token-exempt; touches no store).
fn wait_for_health(port: u16, stderr_path: &Path) {
    let started = Instant::now();
    loop {
        if http_request(port, "/api/health", None).is_some_and(|(status, _)| status == 200) {
            return;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "/api/health never answered 200 within {STARTUP_BOUND:?}; child stderr:\n{}",
            child_stderr(stderr_path)
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Polls for the lazily-written token file (20 ms interval, 5 s bound).
fn wait_for_token(path: &Path) -> String {
    let started = Instant::now();
    loop {
        let token = std::fs::read_to_string(path).unwrap_or_default();
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
        assert!(
            started.elapsed() < SOCKET_BOUND,
            "the token file {path:?} never became non-empty within {SOCKET_BOUND:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// THE pin: relative `--storage-root` + store-backed route ⇒ 200 + success.
#[test]
fn test_relative_storage_root_answers_store_backed_routes() {
    let _lock = serial();
    let (root_absolute, root_relative) = scratch_root("rel-root");
    let dist = root_absolute.join("dist");
    write_shell(&dist);
    let home = root_absolute.join("home");
    std::fs::create_dir_all(&home).expect("create the sandbox home");

    let port = free_port();
    let stderr_path = root_absolute.join("engine.err");
    let _child = spawn_engine(
        &root_relative,
        port,
        &dist,
        &home,
        &root_absolute.join("opencode-mem.log"),
        &stderr_path,
    );
    wait_for_port(port);
    wait_for_health(port, &stderr_path);

    // The token is minted by the first token-less authenticated request
    // (`/api/health` is token-exempt and mints nothing).
    let (status, raw) = http_request(port, "/api/tags", None)
        .expect("the token-less /api/tags request completes once the port accepts");
    assert_eq!(
        status, 401,
        "a token-less /api/tags request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&root_absolute.join(".auth-token"));

    let (status, raw) = http_request(port, "/api/tags", Some(&token))
        .expect("the authenticated /api/tags request completes");
    assert_eq!(
        status,
        200,
        "/api/tags must answer 200 for a RELATIVE --storage-root (the silent-500 RED pin); \
         body: {}\nchild stderr:\n{}",
        raw,
        child_stderr(&stderr_path)
    );
    assert!(
        response_body(&raw).contains("\"success\":true"),
        "the success envelope must be true; raw: {raw}"
    );
}
