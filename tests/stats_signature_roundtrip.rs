//! AC5 — the in-process acceptance round trip: a REAL capture drain, then
//! `GET /api/stats`.
//!
//! This is the acceptance shape the whole wave exists for. It installs the
//! production logging path exactly once (`init_process_logging`), drives a real
//! `mem_bridge::capture::CaptureService` over a real episodic store with a
//! scripted tier-1 writer, then queries a REAL `mem_web::server::start`ed
//! server over loopback and asserts today's counters moved.
//!
//! Hardening (authorised by the Plan Gate): the value may land on either of the
//! LAST TWO days when the test straddles local midnight — the ~2 s window
//! between the writer's timestamp and the reader's clock read. So the assertion
//! is: the first twelve days are all zero, and for each cell the UNION of the
//! last two days equals the expected value with exactly ONE of the two days
//! non-zero. That preserves the blueprint's meaning (one line, one day, nothing
//! smeared) while removing the flake.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mem_bridge::capture::{
    CaptureParams, CaptureService, CaptureSummaryInput, Tier1WriteOutcome, Tier1Writer,
};
use mem_bridge::project_scope::{ProjectScopeError, ResolvedProjectScope};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::store::EpisodicStore;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::serde_json::{self, Value};
use mem_store::shard_manager::ShardManager;

/// The configured API token.
const TOKEN: &str = "roundtrip-token";

/// The fixed project container tag / scope hash every capture resolves to.
const PROJECT_TAG: &str = "opencode_project_0123456789abcdef";
const SCOPE_HASH: &str = "0123456789abcdef";

/// The loopback read/write bound.
const SOCKET_BOUND: Duration = Duration::from_secs(5);

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique scratch directory under `std::env::temp_dir()`
/// (TMPDIR-redirected into `.tmp/scratch` at the cargo invocation).
fn scratch_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "mem-roundtrip-{prefix}-{}-{nanos}-{serial}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("the scratch dir is creatable");
    dir
}

/// The 8-dimensional zero embedder (matches `embedding_dimensions: 8`).
struct ZeroEmbedder;

impl Embedder for ZeroEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![0.0_f32; 8]) })
    }
}

/// The fixed scope resolver (one valid project scope for every project id).
struct FixedResolver;

impl ProjectScopeResolver for FixedResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        mem_bridge::project_scope::apply_resolved_scope(project_id, PROJECT_TAG, SCOPE_HASH)
    }
}

/// A scripted tier-1 writer: pops queued outcomes, defaulting to `Written`.
#[derive(Default)]
struct ScriptedWriter {
    outcomes: Mutex<VecDeque<Tier1WriteOutcome>>,
}

impl Tier1Writer for ScriptedWriter {
    fn write<'a>(
        &'a self,
        _scope: &'a ResolvedProjectScope,
        _input: &'a CaptureSummaryInput,
    ) -> BoxFuture<'a, Result<Tier1WriteOutcome, mem_bridge::capture::CaptureWriterError>> {
        let outcome = self
            .outcomes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or(Tier1WriteOutcome::Written);
        Box::pin(async move { Ok(outcome) })
    }
}

/// One bounded raw HTTP/1.1 GET against the loopback server.
fn http_get(addr: SocketAddr, path: &str, token: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("the loopback server accepts");
    stream
        .set_read_timeout(Some(SOCKET_BOUND))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(SOCKET_BOUND))
        .expect("set write timeout");
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAccept: application/json\r\n\
         {}: {token}\r\nConnection: close\r\n\r\n",
        addr.port(),
        mem_web::AUTH_HEADER,
    );
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error) => panic!("HTTP read deadline on {path}: {error}"),
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

/// The response body after the first blank line.
fn body_of(raw: &str) -> &str {
    raw.split_once("\r\n\r\n")
        .map(|(_head, body)| body)
        .unwrap_or_else(|| panic!("no body in {raw:?}"))
}

/// One cell of a daily row.
fn cell(day: &Value, field: &str) -> u64 {
    day[field]
        .as_u64()
        .unwrap_or_else(|| panic!("{field} is a number: {day}"))
}

/// The error-bucket count of one day.
fn bucket(day: &Value, name: &str) -> u64 {
    day["errors"][name].as_u64().unwrap_or(0)
}

/// Asserts `field` lands on exactly one of the last two days, totalling
/// `expected`, while the first twelve days stay zero.
fn assert_cell_on_one_of_the_last_two_days(daily: &[Value], field: &str, expected: u64) {
    for day in &daily[..12] {
        assert_eq!(cell(day, field), 0, "{field} must be zero on {day}");
    }
    let earlier = cell(&daily[12], field);
    let today = cell(&daily[13], field);
    assert_eq!(
        earlier + today,
        expected,
        "{field} must land on exactly one day: {earlier} + {today}"
    );
    assert!(
        (earlier == 0) != (today == 0),
        "{field} must land on exactly ONE of the last two days: {earlier} / {today}"
    );
}

/// The same rule for one named error bucket.
fn assert_bucket_on_one_of_the_last_two_days(daily: &[Value], name: &str, expected: u64) {
    for day in &daily[..12] {
        assert_eq!(bucket(day, name), 0, "{name} must be zero on {day}");
    }
    let earlier = bucket(&daily[12], name);
    let today = bucket(&daily[13], name);
    assert_eq!(
        earlier + today,
        expected,
        "{name} total: {earlier} + {today}"
    );
    assert!(
        (earlier == 0) != (today == 0),
        "{name} must land on exactly ONE of the last two days: {earlier} / {today}"
    );
}

fn store_config(storage: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: storage.to_path_buf(),
        // Gate ruling: explicit 8 dims, matching `ZeroEmbedder`.
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

fn params(capture_id: &str, session_id: &str) -> CaptureParams {
    CaptureParams {
        session_id: session_id.to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/roundtrip".to_string(),
        agent_id: "agent-1".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_capture_round_trip_moves_todays_stats_counters() {
    let scratch = scratch_dir("acceptance");
    let storage = scratch.join("storage");
    std::fs::create_dir_all(&storage).expect("the storage root is creatable");

    // Install the production logging path exactly once for this binary.
    let sink = mem_core::logging::LogSink::new(scratch.join("opencode-mem.log"));
    mem_core::logging::init_process_logging(sink).expect("the scratch sink installs");

    let runtime = mem_store::tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the multi-thread runtime builds");

    let (handle, addr) = runtime.block_on(async {
        let config = store_config(&storage);
        let store = Arc::new(EpisodicStore::new(storage.clone(), config.clone()));
        store.ready().await.expect("the episodic store is ready");

        let embedder: Arc<dyn Embedder> = Arc::new(ZeroEmbedder);
        let capture = CaptureService::new(
            store.clone(),
            mem_episodic::inbox::CaptureInbox::new(store.clone()),
            Arc::new(ScriptedWriter::default()),
            embedder,
            None,
            Arc::new(FixedResolver),
            131_072,
        );

        // (a) a supplied summary → tier-1 `Written` → persisted.
        let mut persisted = params("019a0000-0000-7f00-8000-00000000a001", "sess-a");
        persisted.summary = Some("Fixed the login handler retry bug.".to_string());
        capture
            .enqueue(&persisted)
            .await
            .expect("enqueue persisted");

        // (b) no summary / metadata / transcript → `assemble_summary` → Skip.
        capture
            .enqueue(&params("019a0000-0000-7f00-8000-00000000a002", "sess-b"))
            .await
            .expect("enqueue skipped");

        // (c) an unknown agent role → `resolve_retention` → dead_letter_now.
        let mut dead = params("019a0000-0000-7f00-8000-00000000a003", "sess-c");
        dead.agent_role = Some("bogus".to_string());
        capture.enqueue(&dead).await.expect("enqueue dead");

        let report = capture.drain_once().await.expect("the drain runs");
        assert_eq!(
            report.done, 2,
            "the persisted + skipped entries complete: {report:?}"
        );
        assert_eq!(
            report.dead_lettered, 1,
            "the bogus role is terminal: {report:?}"
        );
        assert_eq!(
            report.retryable, 0,
            "no attempt is consumed by the skip: {report:?}"
        );
        assert_eq!(report.deferred, 0, "no lock is held: {report:?}");

        // The reader's seam: a fresh (empty) shard manager answers `Ok(vec![])`.
        let connections = Arc::new(ConnectionManager::new(
            storage.clone(),
            config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory),
        ));
        let shards = ShardManager::new(
            storage.clone(),
            config.clone(),
            connections,
            Arc::new(SystemProcessProbe),
        );
        let api = mem_web::api::ports::ApiServices::unavailable().with_memory(Arc::new(
            mem_web::api::store_adapter::ProductionMemory::new(shards),
        ));

        let state = mem_web::server::AppState {
            basic_auth: mem_web::web_auth::WebAuth::new(None, None, "engine"),
            api_token: Some(TOKEN.to_string()),
            local_token: Arc::new(mem_web::api_auth::FileLocalToken::new(
                storage.join(".auth-token"),
            )),
            dist_dir: scratch.join("dist"),
            log_dir: scratch.clone(),
            host_allowlist: mem_web::host_allowlist::HostAllowlist::for_bind_host("127.0.0.1"),
            api: Arc::new(api),
            user_id: "unknown".to_owned(),
            bind_host: "127.0.0.1".to_owned(),
        };
        // Gate ruling: all six fields spelled out (no `Default` impl exists and
        // `enabled: false` would make this test a silent no-op).
        let settings = mem_web::WebServerSettings {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 0,
            auth_password: None,
            auth_username: None,
            api_token: None,
        };
        let handle = mem_web::server::start(&settings, state)
            .await
            .expect("the server starts")
            .expect("the server is enabled");
        let addr = handle.local_addr();
        (handle, addr)
    });

    // The worker threads keep serving while this thread blocks on the socket.
    let (status, raw) = http_get(addr, "/api/stats", TOKEN);
    assert_eq!(status, 200, "GET /api/stats answers 200: {raw}");
    let body: Value = serde_json::from_str(body_of(&raw)).expect("the stats answer is JSON");
    let daily = body["data"]["daily"]
        .as_array()
        .expect("the stats answer carries the daily series");
    assert_eq!(daily.len(), 14, "the series is a 14-day window");

    // AC5: the three signatures land on today (or the immediately preceding day
    // across a local-midnight boundary), and nowhere else.
    assert_cell_on_one_of_the_last_two_days(daily, "captures", 1);
    assert_cell_on_one_of_the_last_two_days(daily, "captureSkipped", 1);
    assert_bucket_on_one_of_the_last_two_days(daily, "Auto-capture final error", 1);
    // The other counters never move in this wave.
    for day in daily {
        assert_eq!(cell(day, "profileInjections"), 0, "{day}");
        assert_eq!(cell(day, "compactionInjections"), 0, "{day}");
    }

    runtime
        .block_on(handle.stop())
        .expect("the server stops cleanly");
}
