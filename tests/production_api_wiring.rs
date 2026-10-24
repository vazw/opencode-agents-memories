//! W6.7 Step 2 — the composition-root production-wiring oracle (AC5).
//!
//! The bug pattern this file exists for (`mem_1792561576490_y40u5vlk8`): a
//! crate-level test that assembles `ApiServices::production(...)` itself can
//! never see a composition root that fails to call it. So this oracle runs the
//! BINARY's own wiring path — the real `src/main.rs` → `RootWebLauncher` →
//! `ApiServices::production(...)` chain — against a storage root seeded with
//! exactly one tier-1 memory row, and asserts a store-backed endpoint answers
//! `200` with that row. Revert `src/main.rs` to `ApiServices::unavailable()`
//! and every assertion here goes RED with the documented `500`
//! `{"success":false,"error":"Internal Server Error"}`.
//!
//! House rules obeyed (same as `tests/web_dist_wiring.rs`): child stdout/stderr
//! go to FILES (never pipes) and every wait is bounded; `std::env::set_var` is
//! never called (the child inherits the same environment the test read its
//! config from); the child gets its own `--storage-root` and a free port, and a
//! `Guarded` drop kills AND reaps it so no engine is orphaned by a panic.
//!
//! The token is acquired the only honest way (Decision 4): one token-less
//! request answers 401 and is what makes the server write `.auth-token`; then a
//! bounded 20 ms / 5 s poll reads it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mem_core::env::ProcessEnv;
use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::serde_json;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert};

/// The project-scope hash of the seeded shard.
const PROJECT_HASH: &str = "a1b2c3d4e5f67890";
/// The seeded container tag (carries the canonical `_project_` marker: the
/// listing skips any row without it — `memories.rs:384-388`).
const PROJECT_TAG: &str = "opencode_project_a1b2c3d4e5f67890";
/// The seeded row's id (asserted in the served body).
const MEMORY_ID: &str = "mem_w67_wiring_probe";
/// The seeded row's content (asserted in the served body).
const MEMORY_CONTENT: &str = "W6.7 composition-root wiring probe";
/// How long a socket read/write and every poll loop may take.
const SOCKET_BOUND: Duration = Duration::from_secs(5);
/// The engine composes its stores BEFORE it binds now, so the startup poll is
/// the generous one.
const STARTUP_BOUND: Duration = Duration::from_secs(30);
/// The 20 ms poll interval (startup, token file and readiness).
const POLL_INTERVAL: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// Helpers (mirroring tests/web_dist_wiring.rs)
// ---------------------------------------------------------------------------

/// A unique, gitignored scratch directory under the repo's `tmp/scratch`
/// (never `/tmp`: this host's tmpfs truncates files to zero under pressure).
fn scratch_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::current_dir().expect("repo root");
    let dir = root
        .join("tmp")
        .join("scratch")
        .join("production-api-wiring")
        .join(format!("{prefix}-{}-{nanos}-{serial}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// The serial harness: no two spawning tests share an engine or a port.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
fn write_shell(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create dist dir");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body><div id=\"w67-fixture\">wiring fixture</div></body></html>",
    )
    .expect("write index.html");
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

/// One bounded HTTP/1.1 GET. `token` is attached as the local-token header
/// (`x-opencode-mem-token`), which is the only header the gate accepts.
fn http_get(port: u16, path: &str, token: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(SOCKET_BOUND))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(SOCKET_BOUND))
        .expect("set write timeout");
    let mut request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
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

/// Seeds one tier-1 project memory row through the REAL store primitives.
///
/// The `StoreConfig` is DERIVED from the same resolved config the child loads,
/// with only `storage_path` overridden (Decision 5): a hand-built literal risks
/// a dims/encryption mismatch that makes the seeded DB unopenable or the row
/// invisible. The seed runs in its OWN `current_thread` runtime and BOTH the
/// manager and the runtime are dropped before the child is spawned, so no open
/// connection survives into the engine.
fn seed_row(storage: &Path, store_config: &StoreConfig) {
    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the seed's own current_thread runtime builds");
    runtime.block_on(async {
        let connections = Arc::new(ConnectionManager::new(
            storage.to_path_buf(),
            store_config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory),
        ));
        let manager = ShardManager::new(
            storage.to_path_buf(),
            store_config.clone(),
            connections,
            Arc::new(SystemProcessProbe),
        );
        let shard = manager
            .create_shard(Scope::Project, PROJECT_HASH, 0)
            .await
            .expect("create the project shard");
        let db = manager
            .open_shard(&shard.db_path)
            .await
            .expect("open the project shard");
        // An explicit vector: no embedder and no ONNX model is needed for the
        // list path, which never embeds.
        let vector = vec![0.25_f32; store_config.embedding_dimensions as usize];
        vector::insert_vector(
            db.as_ref(),
            &MemoryInsert {
                id: MEMORY_ID.to_string(),
                content: MEMORY_CONTENT.to_string(),
                vector,
                tags_vector: None,
                container_tag: PROJECT_TAG.to_string(),
                tags: Some("wiring,probe".to_string()),
                memory_type: Some("note".to_string()),
                created_at: 1_800_000_000_000,
                updated_at: 1_800_000_000_000,
                metadata: None,
                session_id: None,
                display_name: None,
                user_name: None,
                user_email: None,
                project_path: Some("/repo/openapi-demo".to_string()),
                project_name: Some("openapi-demo".to_string()),
                git_repo_url: None,
            },
        )
        .await
        .expect("insert the seed row");
        manager
            .increment_vector_count(shard.id)
            .await
            .expect("bump the shard's vector count");
    });
    // `runtime` drops here: the seeded connection is closed before the spawn.
}

/// Spawns a standalone engine with its stdout/stderr redirected to FILES.
fn spawn_engine(storage: &Path, web_port: u16, dist: &Path, stderr_path: &Path) -> Guarded {
    let stderr = std::fs::File::create(stderr_path).expect("create stderr file");
    let stdout =
        std::fs::File::create(stderr_path.with_extension("out")).expect("create stdout file");
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .arg("--mode=standalone")
        .arg(format!("--storage-root={}", storage.display()))
        .arg(format!("--web-port={web_port}"))
        .arg(format!("--web-dist={}", dist.display()))
        // The dist is named by the CLI flag; an inherited env var must not
        // shadow it in either direction.
        .env_remove(opencode_agents_memory::web_dist::ENV_VAR)
        // Isolate the process-global file log from the real home.
        .env("OPENCODE_MEM_LOG_FILE", storage.join("opencode-mem.log"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    Guarded {
        child: command.spawn().expect("the engine binary spawns"),
    }
}

// ---------------------------------------------------------------------------
// AC5 — the composition-root wiring oracle
// ---------------------------------------------------------------------------

#[test]
fn test_production_wiring_serves_a_seeded_memory() {
    let _lock = serial();
    let root = scratch_dir("serves-seeded");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);

    // The same resolved config the child loads; only the storage path differs.
    let core_config =
        mem_core::config::Config::load_default(&ProcessEnv).expect("the resolved config loads");
    let mut store_config = mem_store::config::from_core(&core_config);
    store_config.storage_path = storage.clone();
    seed_row(&storage, &store_config);

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine(&storage, port, &dist, &stderr_path);
    wait_for_port(port);

    // Decision 4: the token does not exist until the gate's step 4 generates
    // it, so the FIRST request is deliberately token-less and must be 401.
    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );

    let token = wait_for_token(&storage.join(".auth-token"));

    let (status, raw) = http_get(port, "/api/memories", Some(&token));
    assert_eq!(
        status, 200,
        "the binary's own composition root must serve /api/memories, not the \
         stub's 500 — body: {raw}"
    );
    let body: serde_json::Value = serde_json::from_str(
        raw.split("\r\n\r\n")
            .nth(1)
            .unwrap_or_else(|| panic!("no body in {raw}")),
    )
    .expect("the listing answers JSON");
    assert_eq!(body["success"], serde_json::Value::Bool(true), "{body}");
    assert_eq!(
        body["data"]["total"], 1,
        "the seeded row is the only tier-1 row: {body}"
    );
    let items = body["data"]["items"]
        .as_array()
        .expect("the listing carries an items array");
    let seeded = items
        .iter()
        .find(|item| item["id"] == MEMORY_ID)
        .unwrap_or_else(|| panic!("the seeded row is listed: {body}"));
    assert_eq!(seeded["content"], MEMORY_CONTENT, "{body}");
    assert_eq!(
        seeded["tags"],
        serde_json::json!(["wiring", "probe"]),
        "{body}"
    );
    assert_eq!(seeded["type"], "memory", "{body}");

    // The other three core-screen endpoints answer 200 (not 500).
    for path in ["/api/tags", "/api/user-profile", "/api/stats"] {
        let (status, raw) = http_get(port, path, Some(&token));
        assert_eq!(
            status, 200,
            "{path} must be wired to the production ports — body: {raw}"
        );
        assert!(
            raw.contains("\"success\":true"),
            "{path} answers a success envelope — body: {raw}"
        );
    }
}

#[test]
fn test_no_mem_bridge_to_mem_web_edge() {
    // AC4 / ADR-0007:584: mem-bridge may NEVER depend on mem-web — the root
    // crate is the composition root for that edge.
    let manifest = std::fs::read_to_string("crates/mem-bridge/Cargo.toml")
        .expect("read the mem-bridge manifest");
    assert!(
        !manifest.contains("mem-web"),
        "crates/mem-bridge/Cargo.toml must carry no mem-web edge: {manifest}"
    );

    let engine = std::fs::read_to_string("crates/mem-bridge/src/engine.rs")
        .expect("read the mem-bridge engine module");
    assert!(
        !engine.contains("mem_web"),
        "crates/mem-bridge/src/engine.rs must never import mem_web"
    );

    let composition = std::fs::read_to_string("crates/mem-bridge/src/composition.rs")
        .expect("read the mem-bridge composition module");
    assert!(
        !composition.contains("mem_web"),
        "crates/mem-bridge/src/composition.rs must never import mem_web"
    );
}
