//! Issue 41 — PROCESS-LEVEL oracles for the capture-triggered cleanup tick.
//!
//! Every case spawns the real engine binary over a private `HOME` and storage
//! root, seeds ordinary shard rows, and drives captures through the stdio
//! protocol. What is under test is the *wiring*: the hook `run_in_runtime`
//! installs, the tick `RootWebLauncher` spawns before its dist check, the
//! sidecar that carries the gate across a restart, and the fact that
//! `autoCleanupEnabled` gates the tick and nothing else.
//!
//! # Why stderr and not a return value
//!
//! The tick is a detached `tokio::spawn`, and `run_in_runtime` drops the
//! runtime as soon as `serve_stdio` returns — an in-flight tick is cancelled
//! silently. Every case therefore reads the child's stderr file until the
//! expected line appears (bounded at 10 s) and only THEN closes stdin. A
//! negative case instead waits for the ACK plus a 2 s settle, closes stdin, and
//! asserts the line count after exit.
//!
//! # The token
//!
//! `api_token` is `None`, so `/api/*` accepts only the file-minted local token
//! (`x-opencode-mem-token`). The token does not exist on disk until a gated
//! request mints it: one token-less `GET /api/memories` answers 401 and writes
//! it. **Never `Authorization: Bearer`** — that answers 401 here.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{TursoConnectFactory, Value};
use mem_store::platform::Platform;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert};

/// 3 shards x 60 idle rows, and 30 expired prompts.
const SHARDS: u32 = 3;
const ROWS_PER_SHARD: usize = 60;
const PROMPTS: usize = 30;
const RETENTION_DAYS: u32 = 30;
const DAY_MS: i64 = 86_400_000;

const TICK_BOUND: Duration = Duration::from_secs(10);
const TICK_POLL: Duration = Duration::from_millis(10);
const ACK_BOUND: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(2);
const EXIT_BOUND: Duration = Duration::from_secs(20);
const STARTUP_BOUND: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const SOCKET_BOUND: Duration = Duration::from_secs(10);

const PROJECT_HASH: &str = "fedcba9876543210";

fn scratch(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join("scratch")
        .join("cleanup-tick-capture")
        .join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("home")).expect("create the private home");
    std::fs::create_dir_all(dir.join("storage")).expect("create the storage root");
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

fn runtime() -> mem_store::tokio::runtime::Runtime {
    mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the helper runtime")
}

/// The config the child will resolve from `home`, over the private root — the
/// same derivation `mem-bridge`'s composition performs, so the shard schema the
/// fixture writes is the one the engine reads.
fn store_config(home: &Path, storage: &Path) -> StoreConfig {
    let env: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::from([("HOME".to_string(), home.display().to_string())]);
    let config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
            .expect("resolve the sandbox config");
    let mut store = mem_store::config::from_core(&config);
    store.storage_path = storage.to_path_buf();
    store
}

/// Writes the child's `opencode-mem.jsonc` under the private `HOME`.
fn write_config(home: &Path, jsonc: &str) {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create the config dir");
    std::fs::write(dir.join("opencode-mem.jsonc"), jsonc).expect("write the config");
}

fn manager(config: &StoreConfig) -> ShardManager {
    let connections = std::sync::Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        std::sync::Arc::new(TursoConnectFactory),
    ));
    ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections,
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
        None,
    )
}

/// `SHARDS` project shards, each holding `ROWS_PER_SHARD` rows last updated a
/// day past the retention window — every one of them is a sweep candidate.
fn seed_shards(config: &StoreConfig) -> u32 {
    runtime().block_on(async {
        let manager = manager(config);
        let manager = &manager;
        let now = mem_profile::ids::now_millis();
        let updated_at = now - (i64::from(RETENTION_DAYS) + 1) * DAY_MS;
        let mut total = 0u32;
        for index in 0..SHARDS {
            // One active shard per scope: retire the predecessor first, exactly
            // as the rolled-shard path does.
            if let Some(active) = manager
                .get_active_shard(Scope::Project, PROJECT_HASH)
                .await
                .expect("read the active shard")
            {
                manager
                    .mark_shard_read_only(active.id)
                    .await
                    .expect("retire the predecessor shard");
            }
            let shard = manager
                .create_shard(Scope::Project, PROJECT_HASH, index)
                .await
                .expect("create the fixture shard");
            let db = manager
                .open_shard(&shard.db_path)
                .await
                .expect("open the fixture shard");
            for row in 0..ROWS_PER_SHARD {
                insert_row(db.as_ref(), &format!("idle-{index}-{row}"), updated_at).await;
                total += 1;
            }
        }
        total
    })
}

async fn insert_row(db: &dyn mem_store::db::Db, id: &str, updated_at: i64) {
    vector::insert_vector(
        db,
        &MemoryInsert {
            id: id.to_string(),
            content: format!("row {id}"),
            vector: vec![0.0; 768],
            tags_vector: None,
            container_tag: format!("opencode_project_{PROJECT_HASH}"),
            tags: None,
            memory_type: None,
            created_at: updated_at,
            updated_at,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
            source_prompt_created_at: None,
        },
    )
    .await
    .expect("insert the fixture row");
}

/// `PROMPTS` prompts created a day past the retention window. No
/// `linked_memory_id`, so no row is grace-exempt through them.
fn seed_prompts(config: &StoreConfig) {
    runtime().block_on(async {
        let prompts = mem_profile::prompt::UserPromptManager::new(
            config.storage_path.clone(),
            config.clone(),
            RETENTION_DAYS,
        );
        let db = prompts.ready().await.expect("the prompts db is ready");
        let created_at = mem_profile::ids::now_millis() - (i64::from(RETENTION_DAYS) + 1) * DAY_MS;
        for index in 0..PROMPTS {
            db.execute(
                "INSERT INTO user_prompts (id, session_id, message_id, project_path, content, created_at, linked_memory_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
                vec![
                    Value::Text(format!("prompt-{index}")),
                    Value::Text("ses-cleanup".to_string()),
                    Value::Text(format!("msg-{index}")),
                    Value::Text("/probe/project".to_string()),
                    Value::Text("prompt".to_string()),
                    Value::Integer(created_at),
                    Value::Null,
                ],
            )
            .await
            .expect("insert the fixture prompt");
        }
    })
}

/// A dist directory the resolver accepts: `Ready` iff it holds `index.html`.
fn dist_dir(root: &Path, ready: bool) -> PathBuf {
    let dir = root.join(if ready { "dist-ready" } else { "dist-empty" });
    std::fs::create_dir_all(&dir).expect("create the dist dir");
    if ready {
        std::fs::write(dir.join("index.html"), "<!doctype html>").expect("write index.html");
    }
    dir
}

/// The child engine, killed on drop so a panic never leaks a live process.
struct Engine {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    stderr_path: PathBuf,
}

impl Engine {
    fn spawn(home: &Path, storage: &Path, dist: &Path, port: u16) -> Self {
        let stderr_path = storage.join("engine.err");
        let stderr = std::fs::File::create(&stderr_path).expect("create the stderr capture file");
        let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
        command
            .arg("--mode=stdio")
            .arg(format!("--storage-root={}", storage.display()))
            .arg(format!("--web-port={port}"))
            .arg(format!("--web-dist={}", dist.display()))
            .env_clear()
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("OPENCODE_MEM_WEB_HOST", "127.0.0.1")
            .env("OPENCODE_MEM_LOG_FILE", storage.join("opencode-mem.log"))
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

    /// Sends one NDJSON request and returns its response line.
    fn request(&mut self, line: &str) -> String {
        {
            let stdin = self.stdin.as_mut().expect("engine stdin open");
            stdin.write_all(line.as_bytes()).expect("write request");
            stdin.write_all(b"\n").expect("write NDJSON terminator");
            stdin.flush().expect("flush request");
        }
        self.lines
            .recv_timeout(ACK_BOUND)
            .expect("the engine answers the request")
    }

    fn hello(&mut self) {
        let hello = self.request(
            r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#,
        );
        assert!(
            hello.contains(r#""protocol":"1.0.0""#),
            "the handshake answered the protocol line: {hello}"
        );
    }

    /// One capture, acknowledged. The envelope's session-id gate and every
    /// payload gate run for real.
    fn capture(&mut self, id: &str) {
        let line = format!(
            r#"{{"id":"capture-{id}","jsonrpc":"2.0","method":"capture","params":{{"session_id":"ses-cleanup","capture_id":"019a0000-0000-7f00-8000-0000000000{id}","project_id":"/probe/project","agent_id":"agent-1","transcript":"fix the login bug","metadata":{{"promptId":"prompt-{id}"}}}}}}"#
        );
        let response = self.request(&line);
        assert!(
            !response.contains(r#""error""#),
            "the capture must be accepted, got {response}"
        );
        assert!(
            response.contains(r#""accepted":true"#),
            "the capture ACK must be a durable accept, got {response}"
        );
    }

    fn stderr_text(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    /// Reads the stderr file until a line carrying `needle` appears.
    fn wait_for_stderr(&self, needle: &str, bound: Duration) -> String {
        let started = Instant::now();
        loop {
            let text = self.stderr_text();
            if let Some(line) = text.lines().find(|line| line.contains(needle)) {
                return line.to_string();
            }
            assert!(
                started.elapsed() < bound,
                "the engine never wrote {needle:?} within {bound:?}:\n{text}"
            );
            std::thread::sleep(TICK_POLL);
        }
    }

    fn count_stderr(&self, needle: &str) -> usize {
        self.stderr_text()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let child = self.child.as_mut().expect("engine alive");
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                started.elapsed() < EXIT_BOUND,
                "the engine never exited within {EXIT_BOUND:?}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
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

/// One bounded HTTP/1.1 request. `token` rides `x-opencode-mem-token`, the only
/// header the local gate accepts; a POST carries no body.
fn http_request(port: u16, method: &str, path: &str, token: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(SOCKET_BOUND))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(SOCKET_BOUND))
        .expect("set write timeout");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\n"
    );
    if method == "POST" {
        request.push_str("Content-Length: 0\r\n");
    }
    request.push_str("Connection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("x-opencode-mem-token: {token}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error) => panic!(
                "HTTP read deadline ({SOCKET_BOUND:?}) on {path}: {error}; bytes so far: {}",
                String::from_utf8_lossy(&raw)
            ),
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

fn wait_for_token(path: &Path) -> String {
    let started = Instant::now();
    loop {
        let token = std::fs::read_to_string(path).unwrap_or_default();
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "the token file {path:?} never became non-empty within {STARTUP_BOUND:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The seeded fixture: 180 idle rows over 3 shards and 30 expired prompts.
struct Seed {
    root: PathBuf,
    home: PathBuf,
    storage: PathBuf,
    port: u16,
}

fn seed(label: &str, enabled: bool) -> Seed {
    let root = scratch(label);
    let home = root.join("home");
    let storage = root.join("storage");
    write_config(
        &home,
        &format!(r#"{{ "autoCleanupEnabled": {enabled}, "autoCleanupInterval": "24h" }}"#),
    );
    let config = store_config(&home, &storage);
    assert_eq!(
        seed_shards(&config),
        (SHARDS as usize * ROWS_PER_SHARD) as u32,
        "the fixture seeds every row"
    );
    seed_prompts(&config);
    Seed {
        root,
        home,
        storage,
        port: free_port(),
    }
}

fn cleanup_tick_line(engine: &Engine) -> mem_store::serde_json::Value {
    let line = engine.wait_for_stderr(r#""event":"cleanup.tick""#, TICK_BOUND);
    mem_store::serde_json::from_str(&line).expect("the cleanup.tick line is JSON")
}

// ---------------------------------------------------------------------------
// AC1 — a capture is the trigger
// ---------------------------------------------------------------------------

#[test]
fn capture_triggers_exactly_one_tick() {
    let seed = seed("one-tick", true);
    let dist = dist_dir(&seed.root, true);
    let mut engine = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    engine.hello();
    engine.capture("1");
    engine.capture("2");

    let tick = cleanup_tick_line(&engine);
    engine.close_stdin();
    engine.wait_exit();

    assert_eq!(
        tick["deleted_count"],
        mem_store::serde_json::json!(SHARDS as usize * ROWS_PER_SHARD),
        "the tick sweeps every idle row: {tick}"
    );
    assert_eq!(
        tick["prompts_deleted"],
        mem_store::serde_json::json!(PROMPTS),
        "and the expired prompts: {tick}"
    );
    assert_eq!(
        engine.count_stderr(r#""event":"cleanup.tick""#),
        1,
        "two captures coalesce into one sweep:\n{}",
        engine.stderr_text()
    );
}

// ---------------------------------------------------------------------------
// AC5 / R1 — a degraded web launch must not disable cleanup
// ---------------------------------------------------------------------------

#[test]
fn dist_missing_still_ticks() {
    let seed = seed("dist-missing", true);
    let dist = dist_dir(&seed.root, false);
    let mut engine = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    engine.hello();
    engine.capture("1");

    let _tick = cleanup_tick_line(&engine);
    engine.close_stdin();
    engine.wait_exit();

    let stderr = engine.stderr_text();
    assert!(
        stderr.contains(r#""event":"web.dist_missing""#),
        "the dist check must have run and failed:\n{stderr}"
    );
    assert!(
        stderr.contains(r#""event":"web.launch_failed""#),
        "and the launch must have degraded:\n{stderr}"
    );
    assert_eq!(
        engine.count_stderr(r#""event":"cleanup.tick""#),
        1,
        "the tick is spawned BEFORE the dist check:\n{stderr}"
    );
}

#[test]
fn bind_failure_still_ticks() {
    let seed = seed("bind-failure", true);
    // A ready dist forces the case past `web_dist::is_ready`, so the failure
    // below can only be the occupied port.
    let dist = dist_dir(&seed.root, true);
    let occupied = TcpListener::bind("127.0.0.1:0").expect("hold a port");
    let port = occupied.local_addr().expect("local addr").port();

    let mut engine = Engine::spawn(&seed.home, &seed.storage, &dist, port);
    engine.hello();
    engine.capture("1");

    let _tick = cleanup_tick_line(&engine);
    engine.close_stdin();
    engine.wait_exit();

    let stderr = engine.stderr_text();
    assert!(
        stderr.contains(r#""event":"web.launch_failed""#),
        "the occupied port must degrade the launch:\n{stderr}"
    );
    assert!(
        !stderr.contains(r#""event":"web.dist_missing""#),
        "a ready dist must never take the dist-missing path (else this case is vacuous):\n{stderr}"
    );
    assert_eq!(
        engine.count_stderr(r#""event":"cleanup.tick""#),
        1,
        "the tick is spawned BEFORE the server binds:\n{stderr}"
    );
    drop(occupied);
}

// ---------------------------------------------------------------------------
// AC3 — the switch gates the tick, not the port
// ---------------------------------------------------------------------------

#[test]
fn disabled_config_never_ticks_but_manual_endpoint_cleans() {
    let seed = seed("disabled", false);
    let dist = dist_dir(&seed.root, true);
    let mut engine = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    engine.hello();
    engine.capture("1");
    engine.capture("2");
    std::thread::sleep(SETTLE);

    assert_eq!(
        engine.count_stderr(r#""event":"cleanup.tick""#),
        0,
        "autoCleanupEnabled:false must silence the tick:\n{}",
        engine.stderr_text()
    );

    // The port is still wired: mint the local token with one token-less request,
    // then clean manually.
    wait_for_port(seed.port);
    let (status, raw) = http_request(seed.port, "GET", "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&seed.storage.join(".auth-token"));
    let (status, raw) = http_request(seed.port, "POST", "/api/cleanup", Some(&token));
    assert_eq!(status, 200, "the manual endpoint must run: {raw}");
    assert!(
        raw.contains(r#""deletedCount""#),
        "the manual run reports its sweep: {raw}"
    );
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default();
    let parsed: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(body).expect("the cleanup report is JSON");
    assert!(
        parsed["data"]["deletedCount"].as_u64().unwrap_or(0) > 0,
        "the manual run deletes the idle rows the tick would have:\n{body}"
    );

    engine.close_stdin();
    engine.wait_exit();
    assert_eq!(
        engine.count_stderr(r#""event":"cleanup.tick""#),
        0,
        "the manual run must not be reported as a tick:\n{}",
        engine.stderr_text()
    );
}

// ---------------------------------------------------------------------------
// AC4 — the sidecar carries the gate across a restart
// ---------------------------------------------------------------------------

#[test]
fn restart_inside_the_interval_does_not_tick() {
    let seed = seed("restart-inside", true);
    let dist = dist_dir(&seed.root, true);

    let mut first = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    first.hello();
    first.capture("1");
    let _tick = cleanup_tick_line(&first);
    first.close_stdin();
    first.wait_exit();
    assert_eq!(first.count_stderr(r#""event":"cleanup.tick""#), 1);
    drop(first);

    let mut second = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    second.hello();
    second.capture("2");
    std::thread::sleep(SETTLE);
    second.close_stdin();
    second.wait_exit();

    assert_eq!(
        second.count_stderr(r#""event":"cleanup.tick""#),
        0,
        "the sidecar says the interval has not elapsed, so the restarted engine must not sweep:\n{}",
        second.stderr_text()
    );
}

#[test]
fn restart_after_the_interval_ticks_once() {
    let seed = seed("restart-after", true);
    let dist = dist_dir(&seed.root, true);

    let mut first = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    first.hello();
    first.capture("1");
    let _tick = cleanup_tick_line(&first);
    first.close_stdin();
    first.wait_exit();
    drop(first);

    // Age the stamp one millisecond past a 24 h interval.
    let sweep = seed.storage.join("cleanup-sweep.json");
    let now = mem_profile::ids::now_millis();
    let aged = now - DAY_MS - 1;
    std::fs::write(
        &sweep,
        format!(r#"{{"version":1,"last_cleanup_sweep":{aged}}}"#),
    )
    .expect("age the cleanup sweep stamp");

    // Re-seed: the first engine swept the fixture, so the second needs rows of
    // its own to prove that a due tick actually runs.
    let config = store_config(&seed.home, &seed.storage);
    seed_shards(&config);

    let mut second = Engine::spawn(&seed.home, &seed.storage, &dist, seed.port);
    second.hello();
    second.capture("2");
    let tick = cleanup_tick_line(&second);
    second.close_stdin();
    second.wait_exit();

    assert_eq!(
        second.count_stderr(r#""event":"cleanup.tick""#),
        1,
        "an aged sidecar must make the restarted engine sweep exactly once:\n{}",
        second.stderr_text()
    );
    assert_eq!(
        tick["deleted_count"],
        mem_store::serde_json::json!(SHARDS as usize * ROWS_PER_SHARD),
        "and delete the freshly seeded rows: {tick}"
    );
}
