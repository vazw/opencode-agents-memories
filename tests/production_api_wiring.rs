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
use mem_core::tier::MemoryTier;
use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_episodic::timeline::TimelineFilter;
use mem_facts::store::{FactInput, FactSource, FactsStore};
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{Statement, TursoConnectFactory, Value};
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
/// The W6.6 fixture project (the milestone route is project-scoped).
const MILESTONE_PROJECT: &str = "project-alpha";
/// The W6.6 milestone id asserted on the wire.
const MILESTONE_ID: &str = "ms_w66_wiring_probe";
/// The W6.6 promoted fact's content asserted on the wire.
const MILESTONE_FACT_CONTENT: &str = "W6.6 composition-root wiring probe";
/// The seeded milestone's contributor (one source session, one agent).
const MILESTONE_CONTRIBUTORS: &str = r#"[{"sessionRowId":"row-w66","sessionId":"session-w66","agentId":"agent-w66","agentName":"Agent W66","createdAt":1800000000000,"summary":"wiring contribution"}]"#;
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

/// A unique, gitignored scratch directory under the repo's `.tmp/scratch`
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
        .join(".tmp")
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
        }
        let count = mem_store::shard_facts::sync_count(db.as_ref())
            .await
            .expect("sync the shard count");
        manager
            .set_vector_count(shard.id, count)
            .await
            .expect("adopt the shard's vector count");
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

/// The W6.6 fixture: one milestone (`episodic.db`) plus one fact promoted by
/// it (`facts.db`), written through the REAL store paths and read back as a
/// precondition.
struct SeededMilestone {
    milestone_id: String,
    fact_id: String,
    fact_content: String,
}

fn seed_milestone_and_fact(storage: &Path, store_config: &StoreConfig) -> SeededMilestone {
    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the seed's own current_thread runtime builds");
    runtime.block_on(async {
        let store = EpisodicStore::new(storage.to_path_buf(), store_config.clone());
        store.ready().await.expect("the episodic store opens");
        let embedding = mem_store::vector::vector_to_text(&vec![
            0.5_f32;
            store_config.embedding_dimensions as usize
        ])
        .expect("the vector serializes");
        store
            .write_batch(vec![Statement::new(
                "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
                vec![
                    Value::Text(MILESTONE_ID.to_string()),
                    Value::Text(MILESTONE_PROJECT.to_string()),
                    Value::Text("agent-w66".to_string()),
                    Value::Text("wiring milestone".to_string()),
                    Value::Text(MILESTONE_CONTRIBUTORS.to_string()),
                    Value::Integer(1),
                    Value::Integer(1_800_000_000_000),
                    Value::Integer(1_800_000_000_000),
                    Value::Text(embedding),
                    Value::Integer(1_800_000_000_000),
                ],
            )])
            .await
            .expect("insert the milestone");
        let milestones = store
            .list_milestones(MILESTONE_PROJECT, 10)
            .await
            .expect("the seeded milestone reads back");
        assert_eq!(milestones.len(), 1, "the fixture seeded one milestone");
        assert_eq!(milestones[0].id, MILESTONE_ID);

        let facts = FactsStore::new(store_config.clone());
        facts
            .insert(FactInput {
                project_id: MILESTONE_PROJECT.to_string(),
                tier: MemoryTier::Semantic,
                content: MILESTONE_FACT_CONTENT.to_string(),
                key: None,
                source_ref: Some(MILESTONE_ID.to_string()),
                confidence: None,
                metadata: None,
                embedding: None,
                model_id: None,
                sources: vec![FactSource {
                    project_id: MILESTONE_PROJECT.to_string(),
                    session_id: "session-w66".to_string(),
                    agent_id: "agent-w66".to_string(),
                    milestone_id: MILESTONE_ID.to_string(),
                }],
            })
            .await
            .expect("insert the promoted fact");

        SeededMilestone {
            milestone_id: MILESTONE_ID.to_string(),
            fact_id: mem_facts::schema::fact_id(
                MILESTONE_PROJECT,
                MemoryTier::Semantic,
                MILESTONE_FACT_CONTENT,
            ),
            fact_content: MILESTONE_FACT_CONTENT.to_string(),
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
// W6.6 — the composition-root milestone-projection oracle
// ---------------------------------------------------------------------------

/// W6.6 / AC30 — the binary's own composition root attaches the milestone
/// projection over the SAME stores it composed, and the promoted-facts join
/// reads the seeded `facts.db`.
///
/// `ComposedRuntime.facts` is `Some(..)` only when `featureFlags
/// .promotionPipeline` is true (the default is `false`), and the child resolves
/// that flag from the sandbox `HOME`, so the fixture writes it and asserts the
/// resolved value before spawning (a config-key typo fails loudly here rather
/// than as a mystery `[]`).
///
/// Wrong implementations it kills, with the bytes that differ:
/// - no `.with_milestones(..)` in `src/main.rs` → `501`
///   `{"success":false,"error":"the endpoint is not available in this build"}`
///   instead of the milestone row;
/// - the flag false / `WebStoreDeps.facts` unplumbed → `200` with an empty
///   `promotedFacts` instead of the seeded fact (a fact-less 200 is the
///   mutant);
/// - a milestone projection over a different storage root → `200 []`.
#[test]
fn test_production_wiring_serves_the_seeded_milestones() {
    let _lock = serial();
    let root = scratch_dir("milestones-live");
    let home = root.join("home");
    let storage = root.join("storage");
    let dist = root.join("dist");
    std::fs::create_dir_all(&home).expect("create the sandbox home");
    std::fs::create_dir_all(&storage).expect("create storage root");
    write_shell(&dist);
    write_config(&home, "{\"featureFlags\":{\"promotionPipeline\":true}}");

    let config = resolve_sandbox_config(&home);
    assert!(
        config.feature_flags.promotion_pipeline,
        "the sandbox config's promotionPipeline flag resolved (a key typo fails here)"
    );
    let mut store_config = mem_store::config::from_core(&config);
    store_config.storage_path = storage.clone();
    let seeded = seed_milestone_and_fact(&storage, &store_config);

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

    let (status, raw) = http_get(
        port,
        &format!("/api/projects/{MILESTONE_PROJECT}/milestones"),
        Some(&token),
    );
    assert_eq!(
        status, 200,
        "the binary's own composition root must attach the milestone projection, \
         not the 501 — body: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(response_body(&raw)).expect("the milestone list answers JSON");
    assert_eq!(body["success"], serde_json::Value::Bool(true), "{body}");
    let rows = body["data"]
        .as_array()
        .expect("the milestone list carries a data array");
    assert_eq!(rows.len(), 1, "exactly the seeded milestone: {body}");
    assert_eq!(rows[0]["id"], seeded.milestone_id, "{body}");
    assert_eq!(
        rows[0]["projectId"], MILESTONE_PROJECT,
        "the row is project-scoped: {body}"
    );
    let facts = rows[0]["promotedFacts"]
        .as_array()
        .expect("the milestone carries promotedFacts");
    assert_eq!(facts.len(), 1, "the seeded fact is joined: {body}");
    assert_eq!(
        facts[0]["id"], seeded.fact_id,
        "the join carries the fact id: {body}"
    );
    assert_eq!(
        facts[0]["content"], seeded.fact_content,
        "the join carries the fact content: {body}"
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

// ---------------------------------------------------------------------------
// Step 3 — the startup storage-root audit (issue 129)
// ---------------------------------------------------------------------------

/// The hostile parent-directory marker the sanitization test hunts for.
const HOSTILE_MARKER: &str = "HOSTILE_MARK_7f3a";

/// The fixed prefix `build_composed_runtime` puts in front of the episodic
/// error (`composition.rs`). Excluded from the 12-byte leak oracle: it is OUR
/// text, not the store's.
const COMPOSITION_ERROR_PREFIX: &str = "episodic store init failed: ";

/// The expected escaped basename of the hostile root (`root-\n\t-x`), written
/// as a LITERAL so the oracle does not call the production escaper.
const HOSTILE_ROOT_NAME: &str = "root-\\n\\t-x";

/// A no-I/O embedder for the in-process composition call.
struct NoopEmbedder;

impl Embedder for NoopEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![0.0; 8]) })
    }
}

/// The one directory the sanitization test poisons: the basename carries a real
/// newline and a real tab.
fn hostile_root(scratch: &Path) -> PathBuf {
    scratch.join(HOSTILE_MARKER).join("root-\n\t-x")
}

/// Loads the same resolved config the child loads from `home`.
fn audit_config(home: &Path) -> Config {
    resolve_sandbox_config(home)
}

/// The `Err(String)` text `build_composed_runtime` returns for `root`, captured
/// IN PROCESS (the child process cannot see it: `engine.rs` sends it to
/// `tracing`, whose only subscriber drops non-`mem_core::logging` targets).
///
/// The fixture root is shared with the child launch, so the text is the SAME
/// error the child hits — not an injected string.
fn captured_composition_error(root: &Path, home: &Path) -> String {
    let config = audit_config(home);
    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the capture's own current_thread runtime builds");
    runtime
        .block_on(mem_bridge::composition::build_composed_runtime(
            root,
            &config,
            Arc::new(NoopEmbedder),
            None,
        ))
        .err()
        .expect("the poisoned root must fail composition")
}

/// Polls `path` until at least one `event` line matching `event` appears, and
/// returns the parsed events (bounded by `STARTUP_BOUND`).
fn wait_for_events(path: &Path, event: &str) -> Vec<serde_json::Value> {
    let started = Instant::now();
    loop {
        let stderr = std::fs::read_to_string(path).unwrap_or_default();
        let events: Vec<serde_json::Value> = stderr
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
            .filter(|value| value["event"] == event)
            .collect();
        if !events.is_empty() {
            return events;
        }
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "no `{event}` event in {path:?} within {STARTUP_BOUND:?}; stderr so far:\n{stderr}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The single audit event, asserting there is EXACTLY one.
fn single_audit(stderr_path: &Path) -> serde_json::Value {
    let events = wait_for_events(stderr_path, "storage.root_audit");
    assert_eq!(
        events.len(),
        1,
        "exactly one storage.root_audit per process (composition runs once): {events:?}"
    );
    events.into_iter().next().expect("one event")
}

/// The single `entries` element for `store`.
fn audit_entry<'a>(event: &'a serde_json::Value, store: &str) -> &'a serde_json::Value {
    let entries = event["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("the audit carries an entries array: {event}"));
    let matches: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|entry| entry["store"] == store)
        .collect();
    assert_eq!(matches.len(), 1, "exactly one `{store}` entry: {event}");
    matches.into_iter().next().expect("one entry")
}

/// The entry count the matrix fixes (Step 2b, `N`).
const AUDIT_ENTRY_COUNT: usize = 8;

/// Asserts the fixed shape: `entries.len() == N`, each expected store exactly
/// once, and no top-level `store`/`outcome`/`reason`.
fn assert_audit_shape(event: &serde_json::Value) {
    for key in ["store", "outcome", "reason"] {
        assert!(
            event.get(key).is_none(),
            "the event has no top-level `{key}`: {event}"
        );
    }
    assert!(
        event["root_name"].is_string(),
        "root_name is a string: {event}"
    );
    let entries = event["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries is an array: {event}"));
    assert_eq!(
        entries.len(),
        AUDIT_ENTRY_COUNT,
        "the audit is TOTAL over the Step 2b matrix: {event}"
    );
    for store in [
        "episodic.db",
        "user-profiles.db",
        "user-prompts.db",
        "facts.db",
        "skills.db",
        "metadata.db",
        "vault.db",
        "shard.db",
    ] {
        audit_entry(event, store);
    }
}

/// Every string value of the event, recursively (entries included).
fn audit_strings(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                out.extend(audit_strings(item));
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                out.extend(audit_strings(item));
            }
        }
        _ => {}
    }
    out
}

/// The 12-byte leak oracle windows of the captured error text (fixed prefix
/// excluded).
fn leak_windows(captured: &str) -> Vec<&str> {
    let text = captured
        .strip_prefix(COMPOSITION_ERROR_PREFIX)
        .unwrap_or(captured);
    let bytes = text.as_bytes();
    if bytes.len() < 12 {
        return Vec::new();
    }
    (0..=bytes.len() - 12)
        .map(|start| &text[start..start + 12])
        .collect()
}

/// Collects every sanitization violation across the WHOLE event.
fn audit_violations(event: &serde_json::Value, captured: Option<&str>) -> Vec<(String, String)> {
    let mut violations = Vec::new();
    let windows = captured.map(leak_windows).unwrap_or_default();
    for text in audit_strings(event) {
        if text.contains(HOSTILE_MARKER) || text.contains('/') {
            violations.push(("[G1]".to_string(), text.clone()));
        }
        if windows.iter().any(|window| text.contains(window)) {
            violations.push(("[G2]".to_string(), text.clone()));
        }
        if text.chars().any(char::is_control) {
            violations.push(("[G3]".to_string(), text.clone()));
        }
        if let Some(id) = text.strip_prefix("shard-")
            && (id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()))
        {
            violations.push(("[G4]".to_string(), text.clone()));
        }
    }
    let root_name = event["root_name"].as_str().unwrap_or_default();
    if root_name != HOSTILE_ROOT_NAME {
        violations.push((
            "[EQ]".to_string(),
            format!("root_name {root_name:?} != {HOSTILE_ROOT_NAME:?}"),
        ));
    }
    violations
}

fn assert_no_violations(event: &serde_json::Value, captured: Option<&str>) {
    let violations = audit_violations(event, captured);
    let joined: Vec<String> = violations
        .iter()
        .map(|(label, text)| format!("{label} {text:?}"))
        .collect();
    assert!(
        violations.is_empty(),
        "sanitization violations:
{}",
        joined.join(
            "
"
        )
    );
}

/// Recursive listing (relative paths, sorted) of `root`.
fn listing(root: &Path) -> Vec<String> {
    fn walk(base: &Path, current: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .display()
                .to_string();
            out.push(relative);
            if path.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// The MEASURED fresh-root file set of a default-flag run on the pre-change tree
/// (`docs/evidence/issue129-residuals/sanitation-actor-error.md` records the
/// run and the class of every entry). `opencode-mem.log` is the harness's own
/// log sink: `spawn_engine_inner` points `OPENCODE_MEM_LOG_FILE` inside the
/// root, so it appears here and not in the bare-binary measurement.
const FRESH_ROOT_LISTING: [&str; 10] = [
    ".engine.lock",
    ".wal-mode",
    "decay-sweep.json",
    "episodic.db",
    "episodic.db-tshm",
    "episodic.db-wal",
    "opencode-mem.log",
    "user-profiles.db",
    "user-profiles.db-tshm",
    "user-profiles.db-wal",
];

/// Polls until the root's listing STABILIZES (two identical readings 200 ms
/// apart), then returns it. The decay sweep runs asynchronously after
/// composition, so the set settles a moment after the audit event.
fn settled_listing(root: &Path) -> Vec<String> {
    let started = Instant::now();
    let mut previous = listing(root);
    let mut stable = 0;
    while stable < 2 {
        assert!(
            started.elapsed() < STARTUP_BOUND,
            "the root listing never settled within {STARTUP_BOUND:?}: {previous:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
        let current = listing(root);
        stable = if current == previous { stable + 1 } else { 0 };
        previous = current;
    }
    previous
}

// ---------------------------------------------------------------------------
// Step 3a — Found: a valid database from a previous boot
// ---------------------------------------------------------------------------

/// Old behaviour: composition succeeded and nobody said what the store did. New:
/// exactly one audit event, and the `episodic.db` entry says `found` — NOT
/// `created` (the file predates this boot) and NOT `refused`.
#[test]
fn test_audit_classifies_found_missing_and_refused() {
    let _lock = serial();
    let scratch = scratch_dir("audit-found");
    let storage = scratch.join("storage");
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&storage).expect("create storage");
    write_shell(&dist);

    // First boot creates a REAL database (no handcrafted empty file).
    let first_stderr = scratch.join("first.err");
    let port = free_port();
    {
        let _child = spawn_engine_with_home(&storage, port, &dist, &first_stderr, &home);
        wait_for_port(port);
        let event = single_audit(&first_stderr);
        assert_eq!(
            audit_entry(&event, "episodic.db")["outcome"],
            "created",
            "the first boot creates the store: {event}"
        );
    }
    let db = storage.join("episodic.db");
    assert!(
        db.is_file(),
        "the first boot left a regular episodic.db: {:?}",
        listing(&storage)
    );

    // Restart on the same root: composition succeeds again.
    let second_stderr = scratch.join("second.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&storage, port, &dist, &second_stderr, &home);
    wait_for_port(port);
    let event = single_audit(&second_stderr);
    assert_audit_shape(&event);
    let entry = audit_entry(&event, "episodic.db");
    assert_eq!(
        entry["outcome"], "found",
        "a pre-existing database is `found`, never `created`/`refused`: {event}"
    );
    assert!(
        entry["reason"].is_string(),
        "the entry carries a fixed reason token: {event}"
    );
}

// ---------------------------------------------------------------------------
// Step 3b — Created: absent before, present after, actor reached
// ---------------------------------------------------------------------------

/// Wrong implementations it kills: omitting the event; reporting `found` for a
/// store that did not exist before the boot; claiming `created` without the
/// before/after observation (the fixture asserts the file is absent first).
#[test]
fn test_audit_is_total_over_expected_list() {
    let _lock = serial();
    let scratch = scratch_dir("audit-created");
    let storage = scratch.join("storage");
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&storage).expect("create storage");
    write_shell(&dist);

    let db = storage.join("episodic.db");
    assert!(
        std::fs::symlink_metadata(&db).is_err(),
        "fixture precondition: episodic.db is absent before the boot"
    );

    let stderr_path = scratch.join("engine.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    let event = single_audit(&stderr_path);
    assert_audit_shape(&event);
    assert!(
        db.is_file(),
        "the production composition created a regular episodic.db: {:?}",
        listing(&storage)
    );
    assert_eq!(
        audit_entry(&event, "episodic.db")["outcome"],
        "created",
        "absent before + present after = created: {event}"
    );
}

// ---------------------------------------------------------------------------
// Step 3c — Refused: the poisoned `episodic.db` directory
// ---------------------------------------------------------------------------

/// The legacy name denotes a refused TARGET, not an unreadable root: the root
/// is writable and the failing actor is the episodic open.
#[test]
fn test_audit_refuses_when_root_unreadable() {
    let _lock = serial();
    let scratch = scratch_dir("audit-refused");
    let storage = scratch.join("storage");
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&storage).expect("create storage");
    write_shell(&dist);

    let poisoned = storage.join("episodic.db");
    std::fs::create_dir_all(&poisoned).expect("create the poisoned episodic entry");
    assert!(
        poisoned.is_dir(),
        "fixture: a directory sits at episodic.db"
    );

    let stderr_path = scratch.join("engine.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    // The premise: a REAL composition failure, not a wired server.
    wait_for_events(&stderr_path, "engine.composition_failed");

    let event = single_audit(&stderr_path);
    assert_audit_shape(&event);
    let entry = audit_entry(&event, "episodic.db");
    assert_eq!(
        entry["outcome"], "refused",
        "the failed eager actor is `refused`: {event}"
    );
    assert!(
        entry["reason"].is_string(),
        "the refusal is a fixed token, never the store text: {event}"
    );
    assert!(
        poisoned.is_dir(),
        "the audit neither repairs nor replaces the target: {:?}",
        listing(&storage)
    );
}

// ---------------------------------------------------------------------------
// Step 3d — sanitation, sequenced on one shared hostile root
// ---------------------------------------------------------------------------

/// Guards G1 (root redaction), G2 (raw-error redaction) and G3 (control
/// escaping), measured — G4 is ABSENT BY CONSTRUCTION (Step 2b measured that
/// startup never reaches the shard registry, so no shard entry exists).
///
/// Sequence (binding): S0 poison -> S1 in-process capture T -> S2 failure arm
/// with T as the leak oracle -> S3 cleanup -> S4 success arm without G2.
#[test]
fn test_startup_audit_sanitizes_names() {
    let _lock = serial();
    let scratch = scratch_dir("audit-sanitize");
    let root = hostile_root(&scratch);
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    // The marker lives in the PARENT only; the basename holds the controls.
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(root.parent().expect("hostile root has a parent"))
        .expect("create the hostile parent");
    write_shell(&dist);

    // S0: poison the target without touching the (absent) root itself.
    let poisoned = root.join("episodic.db");
    std::fs::create_dir_all(&poisoned).expect("create the poisoned episodic entry");
    assert!(poisoned.is_dir(), "S0: a directory sits at episodic.db");
    assert_eq!(
        listing(&poisoned),
        Vec::<String>::new(),
        "S0: the poisoned directory is empty"
    );

    // S1: capture the REAL error text in process, before any child spawn.
    let captured = captured_composition_error(&root, &home);
    assert!(
        captured.starts_with(COMPOSITION_ERROR_PREFIX),
        "S1: the captured text is the composition error: {captured:?}"
    );
    assert_eq!(
        listing(&poisoned),
        Vec::<String>::new(),
        "S1: the capture is read-only; the poisoned directory is still empty"
    );
    eprintln!("S1 captured composition error: {captured:?}");

    // S2: failure arm, G2 active.
    let fail_stderr = scratch.join("fail.err");
    let port = free_port();
    {
        let _child = spawn_engine_with_home(&root, port, &dist, &fail_stderr, &home);
        wait_for_port(port);
        wait_for_events(&fail_stderr, "engine.composition_failed");
        let stderr = std::fs::read_to_string(&fail_stderr).expect("read the child's stderr");
        let event = single_audit(&fail_stderr);
        assert_audit_shape(&event);
        assert_eq!(
            audit_entry(&event, "episodic.db")["outcome"],
            "refused",
            "S2: the failure arm reports the refusal: {event}"
        );
        assert_no_violations(&event, Some(&captured));
        assert!(
            stderr.contains("storage.root_audit"),
            "S2: the audit is on the same stderr stream as the failure"
        );
    }
    assert!(
        poisoned.is_dir(),
        "S2: post-state is still the poisoned directory"
    );

    // S3: cleanup the poison (fails loudly if something wrote into it).
    std::fs::remove_dir(&poisoned).expect("S3: the poisoned directory is empty and removable");
    assert!(
        std::fs::symlink_metadata(&poisoned).is_err(),
        "S3: the target is gone"
    );

    // S4: success arm, G2 NOT asserted (no captured text on this arm).
    let ok_stderr = scratch.join("ok.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&root, port, &dist, &ok_stderr, &home);
    wait_for_port(port);
    let event = single_audit(&ok_stderr);
    assert_audit_shape(&event);
    assert_eq!(
        audit_entry(&event, "episodic.db")["outcome"],
        "created",
        "S4: absent before + successful ready = created: {event}"
    );
    assert_eq!(
        event["root_name"], HOSTILE_ROOT_NAME,
        "S4: the root name is the escaped basename: {event}"
    );
    assert_no_violations(&event, None);
    assert!(
        poisoned.is_file(),
        "S4: post-state is a regular file: {:?}",
        listing(&root)
    );
}

// ---------------------------------------------------------------------------
// Step 3e — the inventory over the measured file set
// ---------------------------------------------------------------------------

/// Asserts one fixture's audit: shape, per-store outcome/reason, and that the
/// audit created no file beyond the real actors.
fn assert_inventory(event: &serde_json::Value, expected: &[(&str, &str, &str)]) {
    assert_audit_shape(event);
    for (store, outcome, reason) in expected {
        let entry = audit_entry(event, store);
        assert_eq!(entry["outcome"], *outcome, "{store} outcome: {event}");
        assert_eq!(entry["reason"], *reason, "{store} reason: {event}");
    }
}

#[test]
fn test_startup_audit_inventory() {
    let _lock = serial();
    let scratch = scratch_dir("audit-inventory");

    // F1 — fresh root, default flags.
    let f1 = scratch.join("f1");
    let f1_storage = f1.join("storage");
    let f1_home = f1.join("home");
    let f1_dist = f1.join("dist");
    std::fs::create_dir_all(&f1_home).expect("create f1 home");
    std::fs::create_dir_all(&f1_storage).expect("create f1 storage");
    write_shell(&f1_dist);
    assert_eq!(
        listing(&f1_storage),
        Vec::<String>::new(),
        "F1 pre-state: the fresh root is empty"
    );
    let f1_storage_err = f1.join("engine.err");
    let port = free_port();
    {
        let _child = spawn_engine_with_home(&f1_storage, port, &f1_dist, &f1_storage_err, &f1_home);
        wait_for_port(port);
        let event = single_audit(&f1_storage_err);
        assert_inventory(
            &event,
            &[
                ("episodic.db", "created", "observed_created"),
                ("user-profiles.db", "unknown", "not_opened"),
                ("user-prompts.db", "unknown", "not_opened"),
                ("facts.db", "unknown", "disabled"),
                ("skills.db", "unknown", "not_opened"),
                ("metadata.db", "unknown", "not_observed"),
                ("vault.db", "unknown", "not_opened"),
                ("shard.db", "unknown", "not_observed"),
            ],
        );
        let measured: Vec<String> = FRESH_ROOT_LISTING
            .iter()
            .map(|name| name.to_string())
            .collect();
        assert_eq!(
            settled_listing(&f1_storage),
            measured,
            "F1: the run's file set equals the Step 2b measured set"
        );
    }

    // F2 — restart of F1: episodic found, lazy rows still unknown, set unchanged.
    let f2_stderr = f1.join("restart.err");
    let port = free_port();
    {
        let _child = spawn_engine_with_home(&f1_storage, port, &f1_dist, &f2_stderr, &f1_home);
        wait_for_port(port);
        let event = single_audit(&f2_stderr);
        assert_inventory(
            &event,
            &[
                ("episodic.db", "found", "observed"),
                ("user-profiles.db", "unknown", "not_opened"),
                ("metadata.db", "unknown", "not_observed"),
            ],
        );
        let measured: Vec<String> = FRESH_ROOT_LISTING
            .iter()
            .map(|name| name.to_string())
            .collect();
        assert_eq!(
            settled_listing(&f1_storage),
            measured,
            "F2: a restart adds no file"
        );
    }

    // F3 — poisoned episodic.db: refused, later-composed rows not_attempted.
    let f3 = scratch.join("f3");
    let f3_storage = f3.join("storage");
    let f3_home = f3.join("home");
    let f3_dist = f3.join("dist");
    std::fs::create_dir_all(&f3_home).expect("create f3 home");
    std::fs::create_dir_all(f3_storage.join("episodic.db")).expect("poison f3");
    write_shell(&f3_dist);
    let f3_stderr = f3.join("engine.err");
    let port = free_port();
    {
        let _child = spawn_engine_with_home(&f3_storage, port, &f3_dist, &f3_stderr, &f3_home);
        wait_for_port(port);
        wait_for_events(&f3_stderr, "engine.composition_failed");
        let event = single_audit(&f3_stderr);
        assert_inventory(
            &event,
            &[
                ("episodic.db", "refused", "open_failed"),
                ("user-profiles.db", "unknown", "not_attempted"),
                ("user-prompts.db", "unknown", "not_attempted"),
                ("facts.db", "unknown", "not_attempted"),
                ("skills.db", "unknown", "not_attempted"),
                ("metadata.db", "unknown", "not_attempted"),
                ("vault.db", "unknown", "not_attempted"),
                ("shard.db", "unknown", "not_attempted"),
            ],
        );
        assert!(
            f3_storage.join("episodic.db").is_dir(),
            "F3: the degraded launch repairs nothing: {:?}",
            listing(&f3_storage)
        );
        assert!(
            !f3_storage.join("user-profiles.db").exists(),
            "F3: no lazy store is created on the failure path: {:?}",
            listing(&f3_storage)
        );
    }

    // F4 is dropped: Step 2b measured that a flag-ON startup creates no
    // `facts.db` (the store stays lazy), so the row cannot be told apart.
    // F5 — a pre-seeded lazy file stays unknown and changes no store status.
    let f5 = scratch.join("f5");
    let f5_storage = f5.join("storage");
    let f5_home = f5.join("home");
    let f5_dist = f5.join("dist");
    std::fs::create_dir_all(&f5_home).expect("create f5 home");
    std::fs::create_dir_all(&f5_storage).expect("create f5 storage");
    write_shell(&f5_dist);
    std::fs::write(f5_storage.join("skills.db"), b"").expect("pre-seed skills.db");
    let f5_stderr = f5.join("engine.err");
    let port = free_port();
    {
        let _child = spawn_engine_with_home(&f5_storage, port, &f5_dist, &f5_stderr, &f5_home);
        wait_for_port(port);
        let event = single_audit(&f5_stderr);
        assert_inventory(
            &event,
            &[
                ("episodic.db", "created", "observed_created"),
                ("skills.db", "unknown", "not_opened"),
            ],
        );
        let mut measured: Vec<String> = FRESH_ROOT_LISTING
            .iter()
            .map(|name| name.to_string())
            .collect();
        measured.push("skills.db".to_string());
        measured.sort();
        assert_eq!(
            settled_listing(&f5_storage),
            measured,
            "F5: the audit opens nothing; the seeded file is untouched"
        );
    }
    assert_eq!(
        std::fs::read(f5_storage.join("skills.db")).expect("the seeded file is readable"),
        Vec::<u8>::new(),
        "F5: the seeded lazy file was never opened (still empty)"
    );
}

// ---------------------------------------------------------------------------
// Step 2b flag-ON facts.db row — the token the matrix fixes (debug-1 F5)
// ---------------------------------------------------------------------------

/// The Step 2b matrix row for `facts.db`: "Flag off = Unknown `disabled`; flag
/// on construction lazy … otherwise Unknown `not_observed`". F4 was dropped
/// because the flag-ON row cannot be told apart by FILE CREATION (no
/// `facts.db` appears) — but the emitted TOKEN is observable, so this fixture
/// pins it. Wrong implementation it kills: emitting `not_opened` on the
/// flag-ON arm (the genuinely-lazy token), which mislabels a
/// feature-constructed store as merely unopened.
#[test]
fn test_audit_facts_flag_on_reports_not_observed() {
    let _lock = serial();
    let scratch = scratch_dir("audit-facts-on");
    let storage = scratch.join("storage");
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&storage).expect("create storage");
    write_shell(&dist);
    write_config(&home, "{\"featureFlags\":{\"promotionPipeline\":true}}");
    let config = resolve_sandbox_config(&home);
    assert!(
        config.feature_flags.promotion_pipeline,
        "the sandbox config's promotionPipeline flag resolved (a key typo fails here)"
    );

    let stderr_path = scratch.join("engine.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    let event = single_audit(&stderr_path);
    assert_inventory(
        &event,
        &[
            ("episodic.db", "created", "observed_created"),
            ("facts.db", "unknown", "not_observed"),
        ],
    );
    assert!(
        !storage.join("facts.db").exists(),
        "the flag-ON startup still creates no facts.db: {:?}",
        listing(&storage)
    );
}

// ---------------------------------------------------------------------------
// The symlink boundary — a DANGLING link is the M2 oracle (debug-1 F1)
// ---------------------------------------------------------------------------

/// The symlink boundary (Step 2b: "Metadata errors and symlinks are Unknown
/// unless attributed"). A DANGLING symlink at episodic.db is the
/// distinguishing input: `symlink_metadata` reads the link itself (Other →
/// unknown), while a link-following read hits NotFound → Absent and lets the
/// eager actor create through the link and label the boot `created`.
#[test]
fn test_audit_dangling_symlink_at_episodic_is_unknown() {
    let _lock = serial();
    let scratch = scratch_dir("audit-symlink-dangling");
    let storage = scratch.join("storage");
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&storage).expect("create storage");
    write_shell(&dist);

    let db = storage.join("episodic.db");
    std::os::unix::fs::symlink(scratch.join("gone.db"), &db).expect("seed the dangling symlink");
    // Precondition, discriminating BOTH calls: the link exists (symlink_metadata
    // is Ok) AND its target does not (metadata is Err(NotFound)).
    assert!(
        std::fs::metadata(&db).is_err() && std::fs::symlink_metadata(&db).is_ok(),
        "fixture precondition: the link dangles (metadata NotFound, symlink_metadata Ok)"
    );

    let stderr_path = scratch.join("engine.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    let event = single_audit(&stderr_path);
    assert_audit_shape(&event);
    let entry = audit_entry(&event, "episodic.db");
    assert_eq!(
        entry["outcome"], "unknown",
        "a dangling symlink is never `created` (and its `after` reading is Other, \
         not Regular): {event}"
    );
    assert_eq!(entry["reason"], "not_observed", "{event}");
    assert!(
        std::fs::symlink_metadata(&db)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false),
        "post-state: the audit repaired nothing (the link is still a link)"
    );
}

/// Companion fixture: a symlink at `episodic.db` pointing at an EXISTING
/// regular file outside the root. The store's own open refuses through the
/// link, so the entry is `refused/open_failed` on both the correct tree and
/// under the M2 mutant (`presence` following the link) — this test is
/// explicitly NOT the M2 oracle (collinear); it pins that the real actor
/// refuses symlinks and that the audit records the refusal honestly.
#[test]
fn test_audit_escaping_symlink_at_episodic_is_refused() {
    let _lock = serial();
    let scratch = scratch_dir("audit-symlink-escaping");
    let storage = scratch.join("storage");
    let home = scratch.join("home");
    let dist = scratch.join("dist");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&storage).expect("create storage");
    write_shell(&dist);

    let outside = scratch.join("outside.db");
    std::fs::write(&outside, b"not a database").expect("seed the outside file");
    let db = storage.join("episodic.db");
    std::os::unix::fs::symlink(&outside, &db).expect("seed the escaping symlink");
    assert!(
        std::fs::metadata(&db).is_ok() && std::fs::symlink_metadata(&db).is_ok(),
        "fixture precondition: the link resolves (both stat calls Ok)"
    );

    let stderr_path = scratch.join("engine.err");
    let port = free_port();
    let _child = spawn_engine_with_home(&storage, port, &dist, &stderr_path, &home);
    wait_for_port(port);
    let event = single_audit(&stderr_path);
    assert_audit_shape(&event);
    let entry = audit_entry(&event, "episodic.db");
    assert_eq!(
        entry["outcome"], "refused",
        "the store refuses to open through a symlink: {event}"
    );
    assert_eq!(entry["reason"], "open_failed", "{event}");
    assert!(
        std::fs::symlink_metadata(&db)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false),
        "post-state: the audit repaired nothing (the link is still a link)"
    );
}
