//! Issue 41 — the capture hook fires for an ACCEPTED capture and for nothing
//! else.
//!
//! The positive arm ("a valid capture wakes the tick") is a process-level
//! oracle (`tests/cleanup_tick_capture.rs`), because the thing worth proving is
//! that the wake survives a real engine launch. The two branches here are the
//! ones a process oracle cannot flip cheaply: a refused capture and a draining
//! engine must both stay silent — a hook fired before the enqueue result would
//! wake the cleanup tick on traffic that was never stored.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::TestDir;
use mem_bridge::capture::CaptureService;
use mem_bridge::engine::{Engine, EngineConfig, EngineRuntime};
use mem_bridge::jsonrpc::{Response, parse_line};
use serde_json::{Value, json};

/// One engine over a real fixture store, plus the hook counter installed on it.
async fn engine_with_counter(
    storage_root: &std::path::Path,
    chat_capture_enabled: bool,
) -> (Engine, Arc<AtomicUsize>, Arc<CaptureService>) {
    let fixture =
        common::build_fixture_runtime_with_chat_capture(storage_root, chat_capture_enabled).await;
    let capture = fixture.capture.clone();
    let runtime = EngineRuntime {
        capture: fixture.capture,
        inject: fixture.inject,
        recall: fixture.recall,
        skills: None,
        tools: None,
        learning: None,
    };
    let mut engine = Engine::with_runtime(EngineConfig::default(), Arc::new(runtime));
    let fired = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fired);
    engine.set_capture_hook(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));
    (engine, fired, capture)
}

async fn dispatch(engine: &Engine, method: &str, params: Value) -> Response {
    let request = parse_line(
        &json!({"id": "t", "jsonrpc": "2.0", "method": method, "params": params}).to_string(),
    )
    .expect("request line parses");
    engine.dispatch(&request).await
}

/// A payload that passes every gate downstream of the chat-capture gate.
fn valid_capture_payload() -> Value {
    json!({
        "session_id": "ses-hook",
        "capture_id": "019a0000-0000-7f00-8000-000000000041",
        "project_id": "/probe/project",
        "agent_id": "agent-1",
        "transcript": "fix the login bug",
        "metadata": { "promptId": "prompt-hook" }
    })
}

/// `chatMessage.enabled = false` refuses the capture BEFORE the enqueue, so the
/// hook must stay at zero. The discriminating mutant fires the hook on the
/// refusal path — a wake per idle event the user explicitly silenced.
#[tokio::test]
async fn test_refused_capture_never_fires_the_hook() {
    let dir = TestDir::new("capture-hook-refused");
    let (engine, fired, _capture) = engine_with_counter(dir.path(), false).await;

    let response = dispatch(&engine, "capture", valid_capture_payload()).await;
    let result = response.result.as_ref().expect("the refusal is a result");
    assert_eq!(
        result.get("accepted"),
        Some(&Value::Bool(false)),
        "a gated capture answers accepted:false, got {result}"
    );
    assert_eq!(
        fired.load(Ordering::SeqCst),
        0,
        "a refused capture must not wake the cleanup tick"
    );
}

/// After `shutdown` the envelope gate refuses every method; a capture therefore
/// never reaches the hook. The mutant fires it outside the `shutting_down`
/// gate, which would let a shutdown race wake a tick against a dying runtime.
#[tokio::test]
async fn test_capture_after_shutdown_never_fires_the_hook() {
    let dir = TestDir::new("capture-hook-shutdown");
    let (engine, fired, _capture) = engine_with_counter(dir.path(), true).await;

    let stopped = dispatch(&engine, "shutdown", json!({ "session_id": "sess-hook" })).await;
    assert!(
        stopped.error.is_none(),
        "the shutdown request itself answers ok"
    );

    let response = dispatch(&engine, "capture", valid_capture_payload()).await;
    assert!(
        response.error.is_some(),
        "a capture after shutdown answers an error, got {:?}",
        response.result
    );
    assert_eq!(
        fired.load(Ordering::SeqCst),
        0,
        "a refused-after-shutdown capture must not wake the cleanup tick"
    );
}
