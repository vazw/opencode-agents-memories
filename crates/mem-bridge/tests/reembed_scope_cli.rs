//! W3 — the `reembed-scope` CLI process contract (AC23/AC24).
//!
//! A REAL root is seeded, copied, and rewritten by the root binary against a
//! loopback OpenAI-compatible embedding stub; the second run short-circuits.
//! The invalid-scope run proves the tag is validated BEFORE anything opens.

mod common;

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::TestDir;
use mem_core::config::EmbeddingPooling;
use mem_core::embedding::EmbeddingIdentity;
use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{TursoConnectFactory, Value};
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::shard_facts;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert};

const SCOPE_HASH: &str = "a1b2c3d4e5f60008";
const TAG: &str = "opencode_project_a1b2c3d4e5f60008";
const CONTENT: &str = "migrated content";
const PREFIXED: &str = "passage: migrated content";
const FOREIGN_FINGERPRINT: &str = "foreign-fingerprint";

// ---------------------------------------------------------------------------
// The loopback embedding stub
// ---------------------------------------------------------------------------

/// The OpenAI-compatible stub: records every request body and answers with a
/// vector derived from the FINAL input (`[len, 7, 0, 0]`), so the raw
/// `Embedder` mutant (unprefixed input) stores a different vector.
struct Stub {
    base_url: String,
    bodies: Arc<Mutex<Vec<String>>>,
    requests: Arc<AtomicUsize>,
}

fn start_stub() -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
    let port = listener.local_addr().expect("stub addr").port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(AtomicUsize::new(0));
    let bodies_thread = bodies.clone();
    let requests_thread = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            requests_thread.fetch_add(1, Ordering::SeqCst);
            let request = read_request(&mut stream);
            let body = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body.to_string())
                .unwrap_or_default();
            let input = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|payload| {
                    payload
                        .get("input")
                        .and_then(|input| input.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_default();
            bodies_thread
                .lock()
                .expect("stub bodies lock")
                .push(body.clone());
            let derived = input.chars().count();
            let response = format!(r#"{{"data":[{{"embedding":[{derived},7.0,0.0,0.0]}}]}}"#);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                )
                .as_bytes(),
            );
            let _ = stream.flush();
        }
    });
    Stub {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        bodies,
        requests,
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buffer.len() >= header_end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

// ---------------------------------------------------------------------------
// The binary + the seeded root
// ---------------------------------------------------------------------------

fn resolve_binary_path(current_exe: &Path, target_dir: Option<&OsStr>) -> PathBuf {
    let name = format!("opencode-agents-memory{}", std::env::consts::EXE_SUFFIX);
    if let Some(target_dir) = target_dir {
        return Path::new(target_dir).join("debug").join(name);
    }
    let profile_dir = current_exe.parent().map(|parent| {
        if parent.file_name() == Some(OsStr::new("deps")) {
            parent.parent().unwrap_or(parent)
        } else {
            parent
        }
    });
    match profile_dir {
        Some(profile_dir) => profile_dir.join(name),
        None => PathBuf::from(name),
    }
}

fn binary_path() -> PathBuf {
    use std::sync::OnceLock;
    static BUILT: OnceLock<()> = OnceLock::new();
    let exe = std::env::current_exe().expect("current exe");
    let binary = resolve_binary_path(&exe, std::env::var_os("CARGO_TARGET_DIR").as_deref());
    BUILT.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "-q", "-p", "opencode-agents-memory"])
            .status()
            .expect("build the root binary");
        assert!(status.success(), "cargo build failed");
    });
    assert!(
        binary.exists(),
        "the root binary is missing at {}",
        binary.display()
    );
    binary
}

fn scratch_home(label: &str) -> PathBuf {
    let home = common::temp_dir(&format!("w3-reembed-home-{label}"));
    std::fs::create_dir_all(&home).expect("create the scratch home");
    home
}

fn write_config(home: &Path, base_url: &str) {
    let config_dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&config_dir).expect("create the config dir");
    let body = format!(
        r#"{{"embeddingModel":"test-model","embeddingDimensions":4,"embeddingApiUrl":"{base_url}","embeddingApiKey":"test-key","embeddingDocumentPrefix":"passage: "}}"#
    );
    std::fs::write(config_dir.join("opencode-mem.json"), body).expect("write the config");
}

fn run_child(home: &Path, root: &Path, scope: &str) -> std::process::Output {
    Command::new(binary_path())
        .args([
            "--mode=reembed-scope",
            "--storage-root",
            &root.display().to_string(),
            "--scope",
            scope,
        ])
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .expect("spawn the binary")
}

fn identity(document_prefix: Option<&str>) -> EmbeddingIdentity {
    EmbeddingIdentity {
        model: "test-model".to_string(),
        dimensions: 4,
        pooling: EmbeddingPooling::Mean,
        dtype: None,
        use_task_prefixes: false,
        query_prefix: None,
        document_prefix: document_prefix.map(str::to_string),
    }
}

fn seed_config(root: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: 4,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// Seeds one project shard with one row and a FOREIGN fingerprint.
async fn seed_root(root: &Path) {
    let config = seed_config(root);
    let connections = Arc::new(ConnectionManager::new(
        root.to_path_buf(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        root.to_path_buf(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
        Some(identity(Some("passage: "))),
    );
    let shard = manager
        .create_shard(Scope::Project, SCOPE_HASH, 0)
        .await
        .expect("create the project shard");
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the shard");
    vector::insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: "mem_seed".to_string(),
            content: CONTENT.to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            tags_vector: None,
            container_tag: TAG.to_string(),
            tags: None,
            memory_type: Some("note".to_string()),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        },
    )
    .await
    .expect("insert the seed row");
    let count = shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync the seed count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("adopt the registry count");
    for (key, value) in [
        ("embedding_fingerprint", FOREIGN_FINGERPRINT),
        ("embedding_model", "foreign-model"),
    ] {
        db.execute(
            "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES (?1, ?2)",
            vec![Value::Text(key.to_string()), Value::Text(value.to_string())],
        )
        .await
        .expect("seed the foreign identity");
    }
    drop(db);
    drop(manager);
    drop(connections);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create the target dir");
    for entry in std::fs::read_dir(from).expect("read the source dir") {
        let entry = entry.expect("read the source entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("entry type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy the file");
        }
    }
}

// ---------------------------------------------------------------------------
// The rewritten-root assertions
// ---------------------------------------------------------------------------

struct Rewritten {
    fingerprint: Option<String>,
    stored_model: Option<String>,
    stats: u64,
    rows: u64,
    vector: Vec<f32>,
    registry_count: u64,
}

async fn inspect(root: &Path) -> Rewritten {
    let config = seed_config(root);
    let connections = Arc::new(ConnectionManager::new(
        root.to_path_buf(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        root.to_path_buf(),
        config,
        connections.clone(),
        Arc::new(SystemProcessProbe),
        None,
    );
    let shard = manager
        .get_all_shards(Scope::Project, Some(SCOPE_HASH))
        .await
        .expect("registry read")
        .into_iter()
        .next()
        .expect("registry row");
    let registry_count = shard.vector_count;
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open the rewritten shard");
    let fingerprint = shard_facts::read_fingerprint(db.as_ref())
        .await
        .expect("fingerprint read");
    let stored_model = mem_store::db::query_one(
        db.as_ref(),
        "SELECT value FROM shard_metadata WHERE key = 'embedding_model'",
        Vec::new(),
    )
    .await
    .expect("model read")
    .and_then(|row| row.text("value"));
    let stats = shard_facts::read_stored_count(db.as_ref())
        .await
        .expect("stored count");
    let rows = shard_facts::read_count(db.as_ref()).await.expect("count");
    let vector = mem_store::db::query_one(
        db.as_ref(),
        "SELECT vector FROM memories WHERE id = 'mem_seed'",
        Vec::new(),
    )
    .await
    .expect("row read")
    .expect("the row survives")
    .get("vector")
    .and_then(mem_store::vector_utils::blob_to_float32_array)
    .expect("the stored vector decodes");
    drop(db);
    drop(connections);
    Rewritten {
        fingerprint,
        stored_model,
        stats,
        rows,
        vector,
        registry_count,
    }
}

// ---------------------------------------------------------------------------
// AC23 / AC24
// ---------------------------------------------------------------------------

/// AC23 — run 1 rewrites the copied scope through the PREFIXED embedding and
/// flips the fingerprint; run 2 short-circuits without a request.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_reembed_scope_rewrites_a_copied_scope_then_short_circuits() {
    let stub = start_stub();
    let seed = TestDir::new("w3-reembed-seed");
    seed_root(seed.path()).await;

    let child = TestDir::new("w3-reembed-child");
    copy_dir(seed.path(), child.path());
    let home = scratch_home("rewrite");
    write_config(&home, &stub.base_url);

    let output = run_child(&home, child.path(), TAG);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {stderr}\nstdout: {stdout}"
    );
    let completed: serde_json::Value =
        serde_json::from_str(stdout.lines().next().expect("one stdout line"))
            .expect("stdout is one JSON line");
    assert_eq!(completed["event"], "reembed.completed");
    assert_eq!(completed["success"], true);
    assert_eq!(completed["strategy"], "re-embed");
    assert_eq!(completed["re_embedded_memories"], 1);

    // The stub saw the PREFIXED document input and stored its derived vector.
    let expected_vector = vec![PREFIXED.chars().count() as f32, 7.0, 0.0, 0.0];
    {
        let bodies = stub.bodies.lock().expect("stub bodies lock");
        assert!(
            bodies.iter().any(|body| body.contains(PREFIXED)),
            "the request body carries the prefixed content: {bodies:?}"
        );
    }

    let rewritten = inspect(child.path()).await;
    assert_eq!(
        rewritten.fingerprint.as_deref(),
        Some(
            identity(Some("passage: "))
                .fingerprint()
                .expect("identity serializes")
                .as_str()
        ),
        "the foreign fingerprint was replaced by the configured one"
    );
    assert_eq!(rewritten.stored_model.as_deref(), Some("test-model"));
    assert_eq!(rewritten.stats, 1);
    assert_eq!(rewritten.rows, 1);
    assert_eq!(rewritten.registry_count, 1);
    assert_eq!(
        rewritten.vector, expected_vector,
        "the stored vector is f(prefixed input), not f(raw content)"
    );

    // Run 2: every shard is fingerprinted now -> zero embeds, no new request.
    let requests_after_first = stub.requests.load(Ordering::SeqCst);
    let output = run_child(&home, child.path(), TAG);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    let completed: serde_json::Value =
        serde_json::from_str(stdout.lines().next().expect("one stdout line"))
            .expect("stdout is one JSON line");
    assert_eq!(completed["success"], true);
    assert_eq!(completed["re_embedded_memories"], 0);
    assert_eq!(
        stub.requests.load(Ordering::SeqCst),
        requests_after_first,
        "the short-circuit issues no embedding request"
    );
}

/// AC24 — an invalid tag is refused BEFORE anything opens: exit 64, the
/// documented event, and the non-existent storage root stays absent.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_reembed_scope_refuses_an_invalid_scope() {
    let home = scratch_home("invalid");
    let missing_root = home.join("nonexistent-root");
    assert!(!missing_root.exists());

    let output = run_child(&home, &missing_root, "not-a-container-tag");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(64), "stderr: {stderr}");
    assert!(
        stderr.contains("reembed.invalid_scope"),
        "the documented event: {stderr}"
    );
    assert!(
        !missing_root.exists(),
        "validation happens before any open: the root must not be created"
    );
}
