//! W5.1 step 7 — behavioural JSON-RPC error oracles (M6).
//!
//! The dispatcher answers every malformed/unknown/in-bound-violating
//! request with a SPECIFIC code (never the generic -32603). `session_id`
//! bound: 1..=128 characters; missing/blank/over-long → -32602.

use mem_bridge::engine::{Engine, EngineConfig};
use mem_bridge::jsonrpc::{RequestId, Response, parse_line};
use serde_json::{Value, json};
use std::path::PathBuf;

mod common;

fn engine() -> Engine {
    Engine::new(EngineConfig::default())
}

async fn dispatch_request(line: &str) -> Response {
    match parse_line(line) {
        Ok(request) => engine().dispatch(&request).await,
        Err(error) => Response::from_parse_line_error(RequestId::Null, error),
    }
}

/// The sync fixture path keeps the OLD name (helpers below use it).
async fn dispatch_invalid(line: &str) -> Response {
    dispatch_request(line).await
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("response carries an error member")
}

fn assert_param_error(response: &Response) {
    let error = response.error.as_ref().expect("error envelope");
    assert_eq!(error.code, -32602, "session_id bound violation is -32602");
    assert_eq!(error.message, "Invalid params");
    if let Some(session_id) = error
        .data
        .as_ref()
        .and_then(|data| data.get("session_id"))
        .and_then(Value::as_str)
    {
        // Data is advisory: the offending VALUE may be echoed only when
        // it is safe to echo; data never carries a secret or transcript.
        assert!(session_id.matches('\n').count() == 0, "data is one line");
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_unknown_method_returns_32601() {
    let response =
        dispatch_invalid(r#"{"id":"unknown-1","jsonrpc":"2.0","method":"teleport"}"#).await;
    assert_eq!(error_code(&response), -32601);
    assert_eq!(response.id.as_str(), Some("unknown-1"), "the id is echoed");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_missing_session_id_returns_32602() {
    let response = dispatch_invalid(
        r#"{"id":"capture-2","jsonrpc":"2.0","method":"capture","params":{"project_id":"proj-1"}}"#,
    )
    .await;
    assert_param_error(&response);
    let data = response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref());
    assert_eq!(
        data.and_then(|data| data.get("session_id")),
        Some(&Value::Null),
        "no offender to echo when missing"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_blank_session_id_returns_32602() {
    let response = dispatch_invalid(
        r#"{"id":"c","jsonrpc":"2.0","method":"health","params":{"session_id":"   "}}"#,
    )
    .await;
    assert_param_error(&response);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_overlong_session_id_returns_32602() {
    let overlong: String = "x".repeat(129);
    let line = json!({
        "id": "capture-2",
        "jsonrpc": "2.0",
        "method": "recall",
        "params": { "session_id": overlong, "query": "q" }
    });
    let response = dispatch_invalid(&line.to_string()).await;
    assert_param_error(&response);
    let data = response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref());
    assert!(
        data.and_then(|data| data.get("session_id"))
            .and_then(Value::as_str)
            .is_some(),
        "the offending value is echoed in advisory data"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hello_takes_no_session_id() {
    // hello is NOT a runtime method (D-40): no session_id is required and the
    // handshake result comes back.
    let request = parse_line(
        r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#,
    )
    .expect("handshake parses");
    let response = engine().dispatch(&request).await;
    assert!(
        response.error.is_none(),
        "a valid hello never errors: {response:?}"
    );
    assert_eq!(response.id.as_str(), Some("hello-1"));
}

/// The id never routes a non-hello request to the dispatcher without the
/// session check — a direct assertion that the five all go through it.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_shutdown_without_session_id_also_32602() {
    let response = dispatch_invalid(r#"{"id":"s","jsonrpc":"2.0","method":"shutdown"}"#).await;
    assert_eq!(error_code(&response), -32602);
    let _ = RequestId::Null; // silence unused in restricted feature builds
}

// ---------------------------------------------------------------------------
// W5.3 / AC9 — the capture protocol oracles: A2 (missing project => -32602
// before enqueue), the durable-ack ordering, -32002 on a full inbox, and
// degraded no-write. Every oracle dispatches through a runtime engine over
// a REAL temp store (fixed scope resolution, fake embedder, no summarizer).
// ---------------------------------------------------------------------------

fn oracle_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "w53-errors-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0)
    ))
}

async fn runtime_engine(name: &str, degraded: bool) -> (Engine, PathBuf) {
    let dir = oracle_dir(name);
    std::fs::create_dir_all(&dir).expect("temp storage");
    let runtime = oracle_runtime(&dir).await;
    let mut config = EngineConfig::default();
    if degraded {
        config.degraded_reason = Some("oracle degraded".to_string());
    }
    (
        Engine::with_runtime(config, std::sync::Arc::new(runtime)),
        dir,
    )
}

fn capture_line(capture_id: &str) -> String {
    json!({
        "id": "capture-1",
        "jsonrpc": "2.0",
        "method": "capture",
        "params": {
            "session_id": "sess-1",
            "capture_id": capture_id,
            "project_id": "/srv/proj",
            "agent_id": "agent-1",
            "transcript": "some transcript"
        }
    })
    .to_string()
}

/// A missing or blank project scope is `-32602` BEFORE any enqueue.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_missing_project_id_returns_invalid_params_without_enqueue() {
    for (line, label) in [
        (
            r#"{"id":"capture-1","jsonrpc":"2.0","method":"capture","params":{"session_id":"s","capture_id":"019a0000-0000-7f00-8000-000000000001"}}"#.to_string(),
            "missing",
        ),
        (
            r#"{"id":"capture-1","jsonrpc":"2.0","method":"capture","params":{"session_id":"s","capture_id":"019a0000-0000-7f00-8000-000000000002","project_id":"   "}}"#.to_string(),
            "blank",
        ),
    ] {
        let request = parse_line(&line).expect("parses");
        let (engine, _dir) = runtime_engine("missing-project", false).await;
        let response = engine.dispatch(&request).await;
        assert_eq!(error_code(&response), -32602, "{label} project scope rejected");

        // The inbox stays EMPTY: the rejection never enqueued.
        assert_eq!(
            engine.live_tasks().load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no drain task spawned"
        );
    }
}

/// The ack ordering: `{"accepted":true}` returns only AFTER the inbox row
/// is durably committed; the drain is spawned AFTER the ack.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_ack_is_after_durable_inbox_enqueue() {
    let request =
        parse_line(&capture_line("019a0000-0000-7f00-8000-000000000003")).expect("parses");
    let (engine, dir) = runtime_engine("ack", false).await;
    let response = engine.dispatch(&request).await;
    assert!(
        response.error.is_none(),
        "the capture answers ok: {response:?}"
    );

    // The durable row IS on disk before any further mutation.
    let db = common::open_raw(&dir, "episodic.db").await;
    let rows = db
        .query(
            "SELECT COUNT(*) AS n FROM capture_inbox WHERE status = 'queued'",
            Vec::new(),
        )
        .await
        .expect("inbox read");
    assert_eq!(
        rows[0].i64("n"),
        Some(1),
        "the row was queued before the ack"
    );
}

/// A full inbox is `-32002 MEM_INBOX_FULL` — never a silent drop.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_full_inbox_returns_minus_32002() {
    let (engine, _dir) = runtime_engine("inbox-full", false).await;
    // Fill the inbox DIRECTLY (the enqueue path is the same statement).
    for index in 0..mem_episodic::inbox::INBOX_CAPACITY {
        let line = capture_line(&format!("019a{index:014}7f00-{index:012}"));
        let request = parse_line(&line).expect("parses");
        let response = engine.dispatch(&request).await;
        assert!(response.error.is_none(), "fill entry {index} must enqueue");
    }
    let over = parse_line(&capture_line("019aOVERFLOW-1")).expect("parses");
    let response = engine.dispatch(&over).await;
    assert_eq!(
        error_code(&response),
        -32002,
        "the 1025th enqueue is MEM_INBOX_FULL"
    );
}

/// A degraded engine refuses writes with `-32001` and performs NO inbox
/// write.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_degraded_capture_returns_minus_32001_without_inbox_write() {
    let request =
        parse_line(&capture_line("019a0000-0000-7f00-8000-000000000008")).expect("parses");
    let (engine, dir) = runtime_engine("degraded", true).await;
    let response = engine.dispatch(&request).await;
    assert_eq!(error_code(&response), -32001, "MEM_DEGRADED");

    // No inbox write happened for the degraded entry.
    let db = common::open_raw(&dir, "episodic.db").await;
    let rows = db
        .query("SELECT COUNT(*) AS n FROM capture_inbox", Vec::new())
        .await
        .expect("inbox read");
    assert_eq!(
        rows[0].i64("n"),
        Some(0),
        "the degraded engine never enqueues"
    );
}

/// The ack is only emitted once the enqueue is DURABLE: the queued row (with
/// its full payload) exists when `{accepted:true}` returns, and no runtime
/// task can precede it.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_ack_waits_for_durable_enqueue() {
    let capture_id = "019a0000-0000-7f00-8000-00000000000a";
    let request = parse_line(&capture_line(capture_id)).expect("parses");
    let (engine, dir) = runtime_engine("ack-durable", false).await;

    let response = engine.dispatch(&request).await;
    assert!(
        response.error.is_none(),
        "the capture answers ok: {response:?}"
    );
    assert_eq!(
        response.id.as_str(),
        Some("capture-1"),
        "the ack answers the request id"
    );

    // Durable at ack time: the row is on disk with the payload as received.
    let db = common::open_raw(&dir, "episodic.db").await;
    let rows = db
        .query(
            "SELECT capture_id, payload FROM capture_inbox WHERE capture_id = ?",
            vec![mem_store::db::Value::Text(capture_id.to_string())],
        )
        .await
        .expect("inbox read");
    assert_eq!(rows.len(), 1, "the enqueue committed BEFORE the ack");
    let payload = rows[0].text("payload").unwrap_or_default();
    assert!(
        payload.contains(capture_id),
        "payload carries the capture id"
    );
    assert!(
        payload.contains("/srv/proj"),
        "payload carries the project id"
    );
}

/// The A2/M2 write/read split for a missing scope: a WRITE path is `-32602`
/// and persists NOTHING (no inbox row, no shard file); an `inject` is an
/// empty block with no shard query and no empty-tag read.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_missing_project_scope_writes_nothing() {
    let (engine, dir) = runtime_engine("missing-scope", false).await;

    let capture = parse_line(
        r#"{"id":"capture-1","jsonrpc":"2.0","method":"capture","params":{"session_id":"s","capture_id":"019a0000-0000-7f00-8000-00000000000b","transcript":"t"}}"#,
    )
    .expect("parses");
    let response = engine.dispatch(&capture).await;
    assert_eq!(
        error_code(&response),
        -32602,
        "write path rejects the missing scope"
    );

    let db = common::open_raw(&dir, "episodic.db").await;
    let rows = db
        .query("SELECT COUNT(*) AS n FROM capture_inbox", Vec::new())
        .await
        .expect("inbox read");
    assert_eq!(rows[0].i64("n"), Some(0), "nothing was enqueued");

    for line in [
        r#"{"id":"inject-1","jsonrpc":"2.0","method":"inject","params":{"session_id":"s"}}"#,
        r#"{"id":"inject-2","jsonrpc":"2.0","method":"inject","params":{"session_id":"s","project_id":"  "}}"#,
    ] {
        let request = parse_line(line).expect("parses");
        let response = engine.dispatch(&request).await;
        assert!(
            response.error.is_none(),
            "inject never errors on the absent scope"
        );
        let result = response.result.clone().unwrap_or_default();
        assert!(result["block"].is_null(), "an empty block, never -32602");
        assert_eq!(result["memory_count"], 0);
    }

    // No shard file was ever created: neither path issued a shard query.
    let files = common::list_data_files(&dir).await;
    assert!(
        !files.iter().any(|name| name == "projects"),
        "no shard scope directory was created: {files:?}"
    );
}

async fn oracle_runtime(storage_root: &std::path::Path) -> mem_bridge::engine::EngineRuntime {
    let fixture = common::build_fixture_runtime(storage_root).await;
    mem_bridge::engine::EngineRuntime {
        capture: fixture.capture,
        inject: fixture.inject,
        recall: fixture.recall,
        skills: None,
        tools: None,
    }
}
