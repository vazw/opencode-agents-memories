//! W6.7 Step 2 — the composition-root production-wiring oracle (AC5).
//!
//! The bug pattern this file exists for (`mem_1792561576490_y40u5vlk8`): a
//! crate-level test that assembles `ApiServices::production(...)` itself can
//! never see a composition root that fails to call it. So this oracle runs the
//! BINARY's own wiring path — the real `src/main.rs` → `RootWebLauncher` →
//! `ApiServices::production(...)` chain — against a storage root seeded with
//! exactly one tier-1 memory row, and asserts a store-backed endpoint answers
//! `200` with that row. Revert `src/main.rs` to `ApiServices::unavailable()`
//! and every assertion here goes RED with the documented `501`
//! `{"success":false,"error":"the endpoint is not available in this build"}`.
//!
//! House rules obeyed (same as `tests/web_dist_wiring.rs`): child stdout/stderr
//! go to FILES (never pipes) and every wait is bounded; `std::env::set_var` is
//! never called — a test that needs a different config home supplies `HOME` to
//! the child through `Command::env`, so the process environment is never
//! mutated; the child gets its own `--storage-root` and a free port, and a
//! `Guarded` drop kills AND reaps it so no engine is orphaned by a panic.
//!
//! The token is acquired the only honest way (Decision 4): one token-less
//! request answers 401 and is what makes the server write `.auth-token`; then a
//! bounded 20 ms / 5 s poll reads it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mem_core::config::{Config, ConfigPaths};
use mem_core::env::ProcessEnv;
use mem_core::scope::Scope;
use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_episodic::timeline::TimelineFilter;
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
/// The `project-beta` session summary: the positive control that proves the
/// session timeline's project filter selected beta's own rows rather than
/// hiding every foreign row.
const FOREIGN_PROJECT_SUMMARY: &str = "AC2-FOREIGN-PROJECT-SESSION";
/// The older of two byte-identical rows (inserted with `created_at` 2_000).
const DEDUP_OLD_ID: &str = "mem_54_dedup_old";
/// The newer of two byte-identical rows (inserted with `created_at` 3_000): the
/// exact pass keeps it and deletes the older one.
const DEDUP_NEW_ID: &str = "mem_54_dedup_new";
/// The byte-identical content both exact-duplicate rows share.
const DEDUP_CONTENT: &str = "issue 54 exact duplicate probe";
/// The first of two different rows whose vectors sit at cosine 0.6: the
/// representative of the single near-duplicate group.
const NEAR_REF_ID: &str = "mem_54_near_ref";
/// The second cosine-0.6 row: reported as the near duplicate of [`NEAR_REF_ID`].
const NEAR_OTHER_ID: &str = "mem_54_near_other";
/// A content distinct from [`NEAR_OTHER_CONTENT`]: no exact group forms.
const NEAR_REF_CONTENT: &str = "issue 54 near reference";
/// A content distinct from [`NEAR_REF_CONTENT`]: no exact group forms.
const NEAR_OTHER_CONTENT: &str = "issue 54 near neighbour";
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

/// Writes the sandbox's resolved config file. `ConfigPaths::standard` lists
/// `opencode-mem.jsonc` then `opencode-mem.json`; the sandbox writes no
/// `.jsonc`, so the `.json` is the first candidate that exists.
fn write_config(home: &Path, json: &str) {
    let config_dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&config_dir).expect("create the sandbox config dir");
    std::fs::write(config_dir.join("opencode-mem.json"), json).expect("write the sandbox config");
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

/// One bounded HTTP/1.1 request. `token` is attached as the local-token header
/// (`x-opencode-mem-token`), which is the only header the gate accepts. A POST
/// carries no body, so it sends `Content-Length: 0`.
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

/// One bounded HTTP/1.1 GET.
fn http_get(port: u16, path: &str, token: Option<&str>) -> (u16, String) {
    http_request(port, "GET", path, token)
}

/// The body half of a raw HTTP response.
fn response_body(raw: &str) -> &str {
    raw.split("\r\n\r\n")
        .nth(1)
        .unwrap_or_else(|| panic!("no body in the raw response {raw:?}"))
}

/// A vector of `dims` with the named components set (every other component is
/// zero).
fn vector_with(dims: usize, components: &[(usize, f32)]) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dims];
    for (index, value) in components {
        vector[*index] = *value;
    }
    vector
}

/// A vector whose cosine similarity to `vector_with(dims, [(0, 1.0)])` is
/// `cosine`: `[cosine, sqrt(1 - cosine^2), 0, ..]`.
fn near_vector(dims: usize, cosine: f32) -> Vec<f32> {
    vector_with(dims, &[(0, cosine), (1, (1.0 - cosine * cosine).sqrt())])
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

/// One row to seed, with an explicit vector (no embedder or ONNX model is
/// needed: neither the list path nor the exact pass embeds).
struct SeedRow {
    id: &'static str,
    content: &'static str,
    created_at: i64,
    vector: Vec<f32>,
}

/// Seeds every row into the SAME project shard, through the REAL store
/// primitives.
///
/// The `StoreConfig` is DERIVED from the same resolved config the child loads,
/// with only `storage_path` overridden (Decision 5): a hand-built literal risks
/// a dims/encryption mismatch that makes the seeded DB unopenable or the row
/// invisible. The seed runs in its OWN `current_thread` runtime and BOTH the
/// manager and the runtime are dropped before the child is spawned, so no open
/// connection survives into the engine.
fn seed_rows(storage: &Path, store_config: &StoreConfig, rows: &[SeedRow]) {
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
            None,
        );
        let shard = manager
            .create_shard(Scope::Project, PROJECT_HASH, 0)
            .await
            .expect("create the project shard");
        let db = manager
            .open_shard(&shard.db_path)
            .await
            .expect("open the project shard");
        for row in rows {
            vector::insert_vector(
                db.as_ref(),
                &MemoryInsert {
                    id: row.id.to_string(),
                    content: row.content.to_string(),
                    vector: row.vector.clone(),
                    tags_vector: None,
                    container_tag: PROJECT_TAG.to_string(),
                    tags: Some("wiring,probe".to_string()),
                    memory_type: Some("note".to_string()),
                    created_at: row.created_at,
                    updated_at: row.created_at,
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
        }
    });
    // `runtime` drops here: the seeded connection is closed before the spawn.
}

/// Seeds one tier-1 project memory row (the composition-root smoke fixture).
fn seed_row(storage: &Path, store_config: &StoreConfig) {
    seed_rows(
        storage,
        store_config,
        &[SeedRow {
            id: MEMORY_ID,
            content: MEMORY_CONTENT,
            created_at: 1_800_000_000_000,
            vector: vec![0.25_f32; store_config.embedding_dimensions as usize],
        }],
    );
}

/// A fixed-vector embedder sized to the store config: the episodic store's
/// dimension check passes and no ONNX model or network is involved.
struct FixedEmbedder {
    dimensions: usize,
}

impl Embedder for FixedEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async move { Ok(vec![0.1; self.dimensions]) })
    }
}

/// The row ids minted by [`seed_sessions`].
struct SeededSessions {
    project_a: Vec<String>,
    project_b: String,
}

/// Seeds two `project-alpha` rows and one `project-beta` row through the REAL
/// episodic store the engine will open, then re-reads both timelines as a
/// fixture precondition.
///
/// The triples are distinct on `agent_sessions`' UNIQUE
/// `(project_id, session_id, agent_id)` key, so no insert takes the store's
/// `Conflict` arm. The beta row's summary is [`FOREIGN_PROJECT_SUMMARY`], the
/// positive control the readers assert on. The store and its runtime drop
/// before the child is spawned, exactly as [`seed_rows`] does.
fn seed_sessions(storage: &Path, store_config: &StoreConfig) -> SeededSessions {
    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the seed's own current_thread runtime builds");
    runtime.block_on(async {
        let store = EpisodicStore::new(storage.to_path_buf(), store_config.clone());
        store.ready().await.expect("the episodic store opens");
        let embedder = FixedEmbedder {
            dimensions: store_config.embedding_dimensions as usize,
        };
        let rows = [
            (
                "project-alpha",
                "session-alpha-1",
                "agent-alpha-1",
                "Alpha session one",
                "Agent Alpha One",
            ),
            (
                "project-alpha",
                "session-alpha-2",
                "agent-alpha-2",
                "Alpha session two",
                "Agent Alpha Two",
            ),
            (
                "project-beta",
                "session-beta-1",
                "agent-beta-1",
                FOREIGN_PROJECT_SUMMARY,
                "Agent Beta One",
            ),
        ];
        let mut ids = Vec::with_capacity(rows.len());
        for (project_id, session_id, agent_id, summary, agent_name) in rows {
            let id = store
                .insert_session(
                    NewAgentSession {
                        project_id: project_id.to_string(),
                        session_id: session_id.to_string(),
                        agent_id: agent_id.to_string(),
                        summary: summary.to_string(),
                        agent_name: agent_name.to_string(),
                        ..Default::default()
                    },
                    &embedder,
                )
                .await
                .expect("the fixture row must insert");
            ids.push(id);
        }
        let alpha = store
            .timeline("project-alpha", &TimelineFilter::default())
            .await
            .expect("the seeded alpha timeline reads back");
        assert_eq!(alpha.len(), 2, "the fixture seeded both alpha rows");
        let beta = store
            .timeline("project-beta", &TimelineFilter::default())
            .await
            .expect("the seeded beta timeline reads back");
        assert_eq!(beta.len(), 1, "the fixture seeded the beta row");
        SeededSessions {
            project_a: vec![ids[0].clone(), ids[1].clone()],
            project_b: ids[2].clone(),
        }
    })
}

/// Spawns a standalone engine with its stdout/stderr redirected to FILES,
/// inheriting the process environment.
fn spawn_engine(storage: &Path, web_port: u16, dist: &Path, stderr_path: &Path) -> Guarded {
    spawn_engine_inner(storage, web_port, dist, stderr_path, None)
}

/// Spawns a standalone engine whose config home is the sandbox `home` (the
/// child's `Config::load_default(&ProcessEnv)` reads `HOME` through
/// `Command::env`; the test process environment is never mutated).
fn spawn_engine_with_home(
    storage: &Path,
    web_port: u16,
    dist: &Path,
    stderr_path: &Path,
    home: &Path,
) -> Guarded {
    spawn_engine_inner(storage, web_port, dist, stderr_path, Some(home))
}

fn spawn_engine_inner(
    storage: &Path,
    web_port: u16,
    dist: &Path,
    stderr_path: &Path,
    home: Option<&Path>,
) -> Guarded {
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
    if let Some(home) = home {
        command.env("HOME", home);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    Guarded {
        child: command.spawn().expect("the engine binary spawns"),
    }
}

/// Resolves the same config the child resolves from the sandbox `home`: the
/// child runs `Config::load_default(&ProcessEnv)` against the `HOME` injected
/// through `Command::env`, which is exactly `ConfigPaths::standard(home)`.
fn resolve_sandbox_config(home: &Path) -> Config {
    let env: BTreeMap<String, String> =
        BTreeMap::from([("HOME".to_string(), home.display().to_string())]);
    Config::load(&ConfigPaths::standard(home), &env).expect("resolve the sandbox config")
}

// ---------------------------------------------------------------------------
// AC54.1/54.2 — the deduplication wiring oracles
// ---------------------------------------------------------------------------

/// AC54.1a / AC54.2b / AC54.3 — the `Some(deps)` arm injects the deduplication
/// port and it runs over the live shards.
///
/// Two byte-identical rows in one project shard: the pass must delete the
/// older one (newest wins) and report no near-duplicate group. With no config
/// file at all the resolved `deduplicationEnabled` is the default `true`.
#[test]
fn test_production_wiring_runs_deduplicate_over_the_live_shards() {
    let _lock = serial();
    let root = scratch_dir("dedup-live");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);

    let config = resolve_sandbox_config(&home);
    assert!(
        config.deduplication_enabled,
        "the absent key resolves to the default true"
    );
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = storage.clone();
    let vector = vec![0.5_f32; store_config.embedding_dimensions as usize];
    seed_rows(
        &storage,
        &store_config,
        &[
            SeedRow {
                id: DEDUP_OLD_ID,
                content: DEDUP_CONTENT,
                created_at: 2_000,
                vector: vector.clone(),
            },
            SeedRow {
                id: DEDUP_NEW_ID,
                content: DEDUP_CONTENT,
                created_at: 3_000,
                vector,
            },
        ],
    );

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);

    // The token is minted by the first token-less request (never by
    // /api/health, which is token-exempt).
    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&storage.join(".auth-token"));

    let (status, raw) = http_request(port, "POST", "/api/deduplicate", Some(&token));
    assert_eq!(
        status, 200,
        "the binary's own composition root must run the deduplication pass: {raw}"
    );
    assert!(
        !raw.contains("deduplicate is not available in this build"),
        "the composition root must inject the deduplication port: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the dedup report answers JSON");
    assert_eq!(body["success"], serde_json::Value::Bool(true), "{body}");
    assert_eq!(
        body["data"]["exactDuplicatesDeleted"], 1,
        "the exact pass deletes the older of two byte-identical rows: {body}"
    );
    assert_eq!(
        body["data"]["nearDuplicateGroups"],
        serde_json::json!([]),
        "identical vectors are collapsed by the exact pass: {body}"
    );

    let (status, raw) = http_get(port, "/api/memories", Some(&token));
    assert_eq!(status, 200, "the listing after the pass: {raw}");
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the listing answers JSON");
    assert_eq!(
        body["data"]["total"], 1,
        "the pass deleted the older duplicate: {body}"
    );
    let items = body["data"]["items"]
        .as_array()
        .expect("the listing carries an items array");
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(
        items[0]["id"], DEDUP_NEW_ID,
        "newest wins the exact group: {body}"
    );
}

/// AC54.1b / AC54.2a — `deduplicationEnabled: false` from the resolved config
/// reaches the factory: the pass answers the 200 business envelope and deletes
/// nothing.
#[test]
fn test_production_wiring_reads_deduplication_enabled_from_the_resolved_config() {
    let _lock = serial();
    let root = scratch_dir("dedup-disabled");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);
    write_config(&home, "{\"deduplicationEnabled\": false}");

    let config = resolve_sandbox_config(&home);
    assert!(
        !config.deduplication_enabled,
        "the sandbox config's key resolved"
    );
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = storage.clone();
    let vector = vec![0.5_f32; store_config.embedding_dimensions as usize];
    seed_rows(
        &storage,
        &store_config,
        &[
            SeedRow {
                id: DEDUP_OLD_ID,
                content: DEDUP_CONTENT,
                created_at: 2_000,
                vector: vector.clone(),
            },
            SeedRow {
                id: DEDUP_NEW_ID,
                content: DEDUP_CONTENT,
                created_at: 3_000,
                vector,
            },
        ],
    );

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&storage.join(".auth-token"));

    let (status, raw) = http_request(port, "POST", "/api/deduplicate", Some(&token));
    assert_eq!(
        status, 200,
        "a disabled pass answers the upstream business envelope: {raw}"
    );
    assert_eq!(
        response_body(&raw).trim(),
        r#"{"success":false,"error":"Deduplication is disabled in config"}"#,
        "the resolved deduplicationEnabled:false reaches DedupService::new"
    );

    let (status, raw) = http_get(port, "/api/memories", Some(&token));
    assert_eq!(status, 200, "the listing after the disabled pass: {raw}");
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the listing answers JSON");
    assert_eq!(
        body["data"]["total"], 2,
        "a disabled pass deletes nothing: {body}"
    );
}

/// AC54.1b / AC54.2c — `deduplicationSimilarityThreshold: 0.5` from the
/// resolved config reaches the factory: two different rows whose vectors sit at
/// cosine 0.6 are reported as one near-duplicate group (and never deleted).
///
/// A hardcoded `0.9` reports no group; a pre-wiring build answers 501. Both
/// directions are covered by the same assertions.
#[test]
fn test_production_wiring_reads_deduplication_similarity_threshold_from_the_resolved_config() {
    let _lock = serial();
    let root = scratch_dir("dedup-threshold");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);
    write_config(&home, "{\"deduplicationSimilarityThreshold\": 0.5}");

    let config = resolve_sandbox_config(&home);
    assert!(
        (config.deduplication_similarity_threshold - 0.5).abs() < f64::EPSILON,
        "the sandbox config's threshold resolved: {}",
        config.deduplication_similarity_threshold
    );
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = storage.clone();
    let dims = store_config.embedding_dimensions as usize;
    seed_rows(
        &storage,
        &store_config,
        &[
            SeedRow {
                id: NEAR_REF_ID,
                content: NEAR_REF_CONTENT,
                created_at: 2_000,
                vector: vector_with(dims, &[(0, 1.0)]),
            },
            SeedRow {
                id: NEAR_OTHER_ID,
                content: NEAR_OTHER_CONTENT,
                created_at: 1_000,
                vector: near_vector(dims, 0.6),
            },
        ],
    );

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&storage.join(".auth-token"));

    let (status, raw) = http_request(port, "POST", "/api/deduplicate", Some(&token));
    assert_eq!(
        status, 200,
        "the binary's own composition root must run the deduplication pass: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the dedup report answers JSON");
    assert_eq!(body["success"], serde_json::Value::Bool(true), "{body}");
    assert_eq!(
        body["data"]["exactDuplicatesDeleted"], 0,
        "the two rows differ byte for byte: {body}"
    );
    let groups = body["data"]["nearDuplicateGroups"]
        .as_array()
        .expect("the report carries a nearDuplicateGroups array");
    assert_eq!(
        groups.len(),
        1,
        "cosine 0.6 is inside a 0.5 threshold band (a hardcoded 0.9 yields no group): {body}"
    );
    assert_eq!(
        groups[0]["representative"]["id"], NEAR_REF_ID,
        "the newest row is the representative: {body}"
    );
    assert_eq!(
        groups[0]["duplicates"][0]["id"], NEAR_OTHER_ID,
        "the older row is the reported near duplicate: {body}"
    );
    let similarity = groups[0]["duplicates"][0]["similarity"]
        .as_f64()
        .expect("the duplicate carries a similarity");
    assert!(
        (0.59..=0.61).contains(&similarity),
        "the reported similarity is the measured cosine 0.6: {similarity}"
    );

    let (status, raw) = http_get(port, "/api/memories", Some(&token));
    assert_eq!(status, 200, "the listing after the pass: {raw}");
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the listing answers JSON");
    assert_eq!(
        body["data"]["total"], 2,
        "near duplicates are reported, never deleted: {body}"
    );
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
         stub's 501 — body: {raw}"
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

// ---------------------------------------------------------------------------
// AC6.1/AC6.3 — the composition-root session-history oracles
// ---------------------------------------------------------------------------

/// AC6.1 / B1 + B2 (read half) — the binary's own composition root attaches the
/// episodic session port, so the project timeline serves the seeded rows and
/// scopes them in both directions.
///
/// Wrong implementations it kills, with the bytes that differ:
/// - no `.with_sessions(..)` → `501`
///   `{"success":false,"error":"Session history endpoints land with W6.3"}`
///   instead of `200 {"success":true,"data":[..]}`;
/// - a port that drops the project predicate → alpha lists 3 rows and the
///   unseeded project lists 3 instead of 2 / 0;
/// - a port hardcoded to one project → beta lists 2 rows without a marker and
///   the unseeded project lists 2 instead of 1 / 0;
/// - a store opened over a different storage root → 0 rows everywhere.
#[test]
fn test_production_wiring_serves_the_seeded_session_history() {
    let _lock = serial();
    let root = scratch_dir("sessions-live");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);

    // The same resolved config the child loads; only the storage path differs.
    let config = resolve_sandbox_config(&home);
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = storage.clone();
    let seeded = seed_sessions(&storage, &store_config);

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);

    // Decision 4: the token is minted by the first token-less request.
    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&storage.join(".auth-token"));

    // project-alpha: exactly its two rows, none of the beta marker.
    let (status, raw) = http_get(port, "/api/projects/project-alpha/sessions", Some(&token));
    assert_eq!(
        status, 200,
        "the binary's own composition root must attach the session port, not \
         the stub's 501 — body: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the session list answers JSON");
    assert_eq!(body["success"], serde_json::Value::Bool(true), "{body}");
    let sessions = body["data"]
        .as_array()
        .expect("the session list carries a data array");
    assert_eq!(
        sessions.len(),
        2,
        "alpha lists exactly its two rows: {body}"
    );
    let listed: std::collections::BTreeSet<&str> = sessions
        .iter()
        .map(|session| {
            session["id"]
                .as_str()
                .unwrap_or_else(|| panic!("every row carries an id: {body}"))
        })
        .collect();
    let expected: std::collections::BTreeSet<&str> =
        seeded.project_a.iter().map(String::as_str).collect();
    assert_eq!(
        listed, expected,
        "alpha lists exactly the seeded row ids: {body}"
    );
    assert!(
        !raw.contains(FOREIGN_PROJECT_SUMMARY),
        "no beta row leaks into the alpha timeline: {raw}"
    );

    // project-beta: exactly its one row — the positive control that makes the
    // alpha absence a filter rather than an empty store.
    let (status, raw) = http_get(port, "/api/projects/project-beta/sessions", Some(&token));
    assert_eq!(status, 200, "beta lists its own timeline: {raw}");
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the beta list answers JSON");
    let sessions = body["data"]
        .as_array()
        .expect("the beta list carries a data array");
    assert_eq!(sessions.len(), 1, "beta lists exactly its one row: {body}");
    assert_eq!(
        sessions[0]["id"], seeded.project_b,
        "beta's row is the seeded one: {body}"
    );
    assert!(
        raw.contains(FOREIGN_PROJECT_SUMMARY),
        "beta's own row reaches the beta timeline: {raw}"
    );

    // An unseeded project: the filter is honoured in the empty direction too
    // (both "filter dropped" and "hardcoded project" answer 2+ here).
    let (status, raw) = http_get(
        port,
        "/api/projects/project-nobody-seeded/sessions",
        Some(&token),
    );
    assert_eq!(
        status, 200,
        "an unseeded project answers an empty list: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the empty list answers JSON");
    assert_eq!(
        body["data"],
        serde_json::json!([]),
        "an unseeded project has no rows: {body}"
    );
}

/// AC6.3 / B2 (write half) — DELETE reaches the same store, soft-deletes the
/// row, and is project-scoped.
///
/// Wrong implementations it kills, with the bytes that differ:
/// - no `.with_sessions(..)` → the first DELETE answers `501` instead of `200
///   {"success":true}`;
/// - a 200-without-writing handler → the follow-up alpha GET still lists 2
///   rows instead of 1 (the effect assertion is what kills it, not the status);
/// - an unscoped delete → `DELETE alpha/{beta row}` answers `200` instead of
///   the fixed `404 {"success":false,"error":"Session not found"}`, and the
///   beta timeline drops to 0 rows while the marker disappears.
#[test]
fn test_production_wiring_session_delete_is_project_scoped() {
    let _lock = serial();
    let root = scratch_dir("sessions-delete");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);

    let config = resolve_sandbox_config(&home);
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = storage.clone();
    let seeded = seed_sessions(&storage, &store_config);

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);

    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&storage.join(".auth-token"));

    // A scoped delete: 200, no data key, one of alpha's rows gone.
    let target = seeded.project_a[0].clone();
    let (status, raw) = http_request(
        port,
        "DELETE",
        &format!("/api/projects/project-alpha/sessions/{target}"),
        Some(&token),
    );
    assert_eq!(status, 200, "the wired DELETE answers 200: {raw}");
    assert_eq!(
        response_body(&raw).trim(),
        r#"{"success":true}"#,
        "a data-less success omits the data key"
    );

    let (status, raw) = http_get(port, "/api/projects/project-alpha/sessions", Some(&token));
    assert_eq!(status, 200, "the alpha timeline after the delete: {raw}");
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the alpha list answers JSON");
    let sessions = body["data"]
        .as_array()
        .expect("the alpha list carries a data array");
    assert_eq!(
        sessions.len(),
        1,
        "the delete removed exactly one row: {body}"
    );
    assert_eq!(
        sessions[0]["id"], seeded.project_a[1],
        "the surviving row is alpha's other seed: {body}"
    );
    assert!(
        !raw.contains(&target),
        "the deleted row id is gone from the timeline: {raw}"
    );

    // A cross-project delete: the beta row named under alpha must not be
    // deleted, and the answer is the fixed 404 (no existence leak).
    let (status, raw) = http_request(
        port,
        "DELETE",
        &format!("/api/projects/project-alpha/sessions/{}", seeded.project_b),
        Some(&token),
    );
    assert_eq!(status, 404, "a wrong-project delete answers 404: {raw}");
    assert_eq!(
        response_body(&raw).trim(),
        r#"{"success":false,"error":"Session not found"}"#,
        "the fixed not-found answer leaks no existence information"
    );

    let (status, raw) = http_get(port, "/api/projects/project-beta/sessions", Some(&token));
    assert_eq!(
        status, 200,
        "the beta timeline after the cross-project delete: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the beta list answers JSON");
    let sessions = body["data"]
        .as_array()
        .expect("the beta list carries a data array");
    assert_eq!(sessions.len(), 1, "the foreign row survives: {body}");
    assert_eq!(sessions[0]["id"], seeded.project_b, "{body}");
    assert!(
        raw.contains(FOREIGN_PROJECT_SUMMARY),
        "the foreign row's marker survives the cross-project delete: {raw}"
    );
}

// ---------------------------------------------------------------------------
// The degrade arm — a pin, not a RED
// ---------------------------------------------------------------------------

/// B5 + AC6.3 — when composition fails, the root launches the documented
/// unavailable service set: sessions stay on the W6.3 stub, and the whole
/// service set is unavailable (memories 501, skills still 200 from its own
/// storage-root-only port).
///
/// This is a GUARD PIN: it is green before and after the wiring change and
/// exists to kill future regressions. Wrong implementations it kills:
/// - a crash instead of a degrade → no server binds (`wait_for_port` fails
///   even though the dist shell and storage root exist);
/// - the `None` arm building production services over a store it does not have
///   → `/api/memories` answers 200 with an empty listing and sessions answer
///   200, instead of 501/501;
/// - the `None` arm losing `.with_skills` → `/api/skills` answers 501 instead
///   of 200;
/// - a sessions-only stub that leaves memories wired → `/api/memories` answers
///   200 instead of 501.
///
/// The poisoned `storage/episodic.db` is a DIRECTORY inside an otherwise valid
/// storage root, so only the store open fails (a missing root would be a
/// different, indistinguishable failure).
#[test]
fn test_sessions_stay_on_the_documented_stub_when_composition_fails() {
    let _lock = serial();
    let root = scratch_dir("degrade");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    // The poisoned entry lives INSIDE the existing storage root: the launch must
    // fail at the episodic store open, not at a missing directory.
    std::fs::create_dir_all(storage.join("episodic.db"))
        .expect("create the poisoned episodic entry");
    // The dist shell is required: with `ResolvedDist::Missing` the web server is
    // never launched and `wait_for_port` would die on a fixture fact.
    write_shell(&dist);

    let port = free_port();
    let stderr_path = root.join("engine.err");
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);

    let (status, raw) = http_get(port, "/api/memories", None);
    assert_eq!(
        status, 401,
        "a token-less /api/memories request must answer 401 (and write the token): {raw}"
    );
    let token = wait_for_token(&storage.join(".auth-token"));

    // The premise: a REAL composition failure, not a 501 on a wired server.
    let stderr = std::fs::read_to_string(&stderr_path).expect("read the child's stderr file");
    assert!(
        stderr.contains("engine.composition_failed"),
        "the launch must have failed composition, not skipped it: {stderr}"
    );

    let (status, raw) = http_get(port, "/api/health", None);
    assert_eq!(status, 200, "the liveness probe still answers: {raw}");

    // The structural discriminator: the whole service set is the unavailable
    // one. A wired root answers 200 over an empty listing here.
    let (status, raw) = http_get(port, "/api/memories", Some(&token));
    assert_eq!(
        status, 501,
        "the degraded root answers from the absent-service class: {raw}"
    );

    let (status, raw) = http_get(port, "/api/skills", Some(&token));
    assert_eq!(status, 200, "the skills port is wired in both arms: {raw}");
    assert!(
        raw.contains("\"success\":true"),
        "the skills listing answers a success envelope: {raw}"
    );

    let (status, raw) = http_get(port, "/api/projects/project-alpha/sessions", Some(&token));
    assert_eq!(
        status, 501,
        "sessions stay on the documented stub when composition failed: {raw}"
    );

    let (status, raw) = http_request(
        port,
        "DELETE",
        "/api/projects/project-alpha/sessions/session-alpha-1",
        Some(&token),
    );
    assert_eq!(
        status, 501,
        "the degraded DELETE stays on the documented stub: {raw}"
    );
}

#[test]
fn test_composition_root_passes_the_shutdown_cancel_flag_to_the_deduplication_port() {
    // AC10: a source pin over the composition root. Reaching this behaviour
    // end to end needs a SIGTERM mid-scan; the wiring itself is a text
    // property, and the mechanism is proven at the mem-web seams
    // (`dedup_offload.rs`, `dedup_service.rs`). House precedent:
    // `test_no_mem_bridge_to_mem_web_edge` below.
    let source = std::fs::read_to_string("src/main.rs").expect("read the composition root");

    assert!(
        source.contains("production_deduplication_port_with_cancel("),
        "the wired arm must use the cancellation-aware factory"
    );
    assert!(
        source.contains("Arc::clone(&deduplication_cancelled)"),
        "the port must receive a clone of the launcher's flag"
    );
    assert!(
        source.contains("use std::sync::atomic::"),
        "the launcher needs the atomic imports for its flag"
    );
    assert!(
        source.contains("deduplication_cancelled.store(true"),
        "the stop closure must set the flag"
    );
    assert!(
        !source.contains("with_maintenance(production_deduplication_port("),
        "the 3-arg factory (a fresh never-set flag) must be gone from the wired arm"
    );

    // Condition: both `find`s must succeed before the positional comparison —
    // `None < Some(..)` is `true`, so comparing raw `Option`s would pass on a
    // source that lost either statement.
    let store_at = source
        .find("deduplication_cancelled.store(true")
        .expect("the stop closure sets the deduplication cancel flag");
    let stop_at = source
        .find("handle.stop().await")
        .expect("the stop closure stops the web server handle");
    assert!(
        store_at < stop_at,
        "the flag must be set BEFORE the server is stopped, so the in-flight scan \
         is cancelled while the runtime is still alive"
    );
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
