//! W6.4 — the live binary oracle for the skills wiring.
//!
//! The blueprint's AC5: the composition root must hand the skills manager
//! to the web layer, so the live binary answers `GET /api/skills` with
//! `200` and the seeded skill's id, and the OpenCode catalog answers
//! `200` instead of the `500` the un-wired build produced.
//!
//! The house rules of `tests/web_dist_wiring.rs` are obeyed here: the
//! child's stdout/stderr go to FILES (never pipes), every wait is bounded,
//! `std::env::set_var` is never called (the child env is injected with
//! `Command::env`), and each spawn test uses its own `--storage-root` and
//! a free port.
//!
//! **No token ever reaches the evidence.** The local token is read from
//! the child's own `{storage_root}/.auth-token` file, sent in a request
//! header and never written to a log, an assertion message or a file.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use opencode_agents_memory::web_dist;

/// The 5 s bound on every socket read/write.
const SOCKET_BOUND: Duration = Duration::from_secs(5);

/// The startup bound: the engine composes its services before it serves.
const STARTUP_BOUND: Duration = Duration::from_secs(60);

/// The seeded skill's id (upstream's kebab-case rule).
const SKILL_ID: &str = "live-wiring-skill";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A unique, gitignored scratch directory under the repo's `.tmp/scratch`
/// (never `/tmp`: this host's tmpfs truncates under pressure).
fn scratch_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::current_dir().expect("repo root");
    let dir = root
        .join(".tmp")
        .join("scratch")
        .join("skills-live-wiring")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// The serial harness: every spawning test takes this for its whole body.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Writes a minimal shell so the web launch is not refused (a missing
/// build refuses the launch, and then no socket is ever bound).
fn write_shell(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create dist dir");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><title>W6.4 fixture</title></head><body></body></html>",
    )
    .expect("write index.html");
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

/// Spawns a standalone engine with its stderr redirected to `stderr_path`.
fn spawn_standalone(
    storage_root: &Path,
    web_port: u16,
    dist: &Path,
    stderr_path: &Path,
) -> Guarded {
    let stderr = std::fs::File::create(stderr_path).expect("create stderr file");
    let stdout =
        std::fs::File::create(stderr_path.with_extension("out")).expect("create stdout file");
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .arg("--mode=standalone")
        .arg(format!("--storage-root={}", storage_root.display()))
        .arg(format!("--web-port={web_port}"))
        .arg(format!("{}={}", web_dist::CLI_FLAG, dist.display()))
        .env_remove(web_dist::ENV_VAR)
        // Isolate the process-global file log from the real home.
        .env(
            "OPENCODE_MEM_LOG_FILE",
            storage_root.join("opencode-mem.log"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    Guarded {
        child: command.spawn().expect("engine spawns"),
    }
}

/// Polls `TcpStream::connect` until the port accepts, within a bounded
/// deadline.
fn wait_for_port(port: u16) -> TcpStream {
    let started = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(error) => {
                assert!(
                    started.elapsed() < STARTUP_BOUND,
                    "the web port {port} never accepted a connection within {STARTUP_BOUND:?}: {error}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// The local API token of the running engine.
///
/// The engine writes its token file LAZILY, on the first gated request, so
/// the probe request comes first: an unauthenticated `GET /api/skills` must
/// answer `401` (the API surface is token-gated), and the probe is what
/// materialises the token file.
///
/// The token is read from the child's OWN token file. It is never printed,
/// never written to a file and never part of an assertion message.
fn local_token_after_probe(storage_root: &Path, port: u16) -> String {
    let (status, raw) = http_exchange(port, "GET", "/api/skills", None, None);
    assert_eq!(
        status, 401,
        "the API surface is token-gated: an unauthenticated request is refused: {raw}"
    );
    local_token(storage_root)
}

/// Reads the local token file (which the probe request materialised).
fn local_token(storage_root: &Path) -> String {
    let path = storage_root.join(".auth-token");
    let started = Instant::now();
    loop {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "the engine never wrote its local token file within {STARTUP_BOUND:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One bounded HTTP/1.1 exchange.
///
/// `token` is sent as the `x-opencode-mem-token` header. The request asks
/// for `Connection: close`, so a keep-alive server cannot hang the test.
fn http_exchange(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(SOCKET_BOUND))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(SOCKET_BOUND))
        .expect("set write timeout");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    if let Some(token) = token {
        request.push_str(&format!("x-opencode-mem-token: {token}\r\n"));
    }
    if let Some(body) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("Connection: close\r\n\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error) => {
                let seen = String::from_utf8_lossy(&raw);
                panic!(
                    "HTTP read deadline ({SOCKET_BOUND:?}) on {path}: {error}; bytes so far: {seen}"
                );
            }
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no HTTP status line in {text:?}"));
    (status, text)
}

/// The body of a raw HTTP response (everything after the header block).
fn body_of(raw: &str) -> &str {
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

// ---------------------------------------------------------------------------
// The oracle
// ---------------------------------------------------------------------------

/// The W6.4 live oracle: the binary answers `200` on `/api/skills` and on
/// the OpenCode catalog, and the seeded skill is in the list.
#[test]
fn test_skills_endpoint_returns_200_after_wiring() {
    let _lock = serial();
    let root = scratch_dir("skills-live");
    let storage = root.join("storage");
    let dist = root.join("dist");
    write_shell(&dist);
    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_standalone(&storage, port, &dist, &stderr_path);
    drop(wait_for_port(port));

    let token = local_token_after_probe(&storage, port);

    // --- Seed one skill through the REST surface.
    let body = format!(
        r#"{{"id":"{SKILL_ID}","name":"Live Wiring Skill","description":"seeded by the W6.4 oracle","content":"Do the live thing."}}"#
    );
    let (status, raw) = http_exchange(port, "POST", "/api/skills", Some(&token), Some(&body));
    assert_eq!(status, 200, "the seeded skill must be created: {raw}");
    let created: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(body_of(&raw)).expect("the create response is JSON");
    assert_eq!(created["success"], mem_store::serde_json::json!(true));
    assert_eq!(
        created["data"]["id"],
        mem_store::serde_json::json!(SKILL_ID)
    );

    // --- The list endpoint answers 200 and carries the seeded id.
    let (status, raw) = http_exchange(port, "GET", "/api/skills", Some(&token), None);
    assert_eq!(
        status, 200,
        "GET /api/skills must answer 200 after the wiring (the un-wired build answered 500): {raw}"
    );
    let listed: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(body_of(&raw)).expect("the list response is JSON");
    assert_eq!(listed["success"], mem_store::serde_json::json!(true));
    let items = listed["data"]["items"]
        .as_array()
        .expect("the list carries an items array");
    let ids: Vec<&str> = items
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert!(
        ids.contains(&SKILL_ID),
        "the seeded skill is in the list: {ids:?}"
    );

    // --- The OpenCode catalog answers 200 (the un-wired build answered 500).
    let (status, raw) = http_exchange(port, "GET", "/skills/index.json", Some(&token), None);
    assert_eq!(
        status, 200,
        "GET /skills/index.json must answer 200 after the wiring (the un-wired build answered 500): {raw}"
    );
    let catalog: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(body_of(&raw)).expect("the catalog response is JSON");
    let entries = catalog["skills"]
        .as_array()
        .expect("the catalog carries skills");
    assert!(
        entries
            .iter()
            .any(|entry| entry["name"].as_str() == Some(SKILL_ID)),
        "the catalog carries the seeded skill: {catalog}"
    );

    // --- The catalog markdown of the seeded skill is served.
    let (status, raw) = http_exchange(
        port,
        "GET",
        &format!("/skills/{SKILL_ID}/{SKILL_ID}.md"),
        Some(&token),
        None,
    );
    assert_eq!(status, 200, "the catalog markdown is served: {raw}");
    assert!(
        body_of(&raw).contains("name: \"Live Wiring Skill\""),
        "the markdown carries the frontmatter: {raw}"
    );
}
