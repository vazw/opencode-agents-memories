//! Issue 98 — `chatMessage.enabled` gates the chat-capture path.
//!
//! The engine half: `capture` refuses before payload validation and enqueue
//! when the resolved flag is off, `hello` advertises the same resolved value in
//! a top-level `config` object, and the refusal is announced exactly once per
//! engine process. The `add` boundary (manual writes stay ungated) is pinned
//! here too.
//!
//! Every case dispatches through the real [`Engine`] over a fixture runtime —
//! no service is called directly, so the envelope's own gates and the method
//! dispatch order are part of what each row observes.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::TestDir;
use common::{FakeEmbedder, MapScopeResolver};
use mem_bridge::capture::CaptureService;
use mem_bridge::engine::{Engine, EngineConfig, EngineRuntime};
use mem_bridge::jsonrpc::{Response, parse_line};
use serde_json::{Value, json};

/// One engine over a real fixture store, with the chat-capture flag set.
///
/// The capture service is returned alongside so a case can observe the inbox
/// depth; the engine holds the SAME `Arc`, so the flag under test and the flag
/// a case can inspect are one value.
async fn gated(storage_root: &Path, chat_capture_enabled: bool) -> (Engine, Arc<CaptureService>) {
    let fixture =
        common::build_fixture_runtime_with_chat_capture(storage_root, chat_capture_enabled).await;
    let capture = fixture.capture.clone();
    let runtime = EngineRuntime {
        capture: fixture.capture,
        inject: fixture.inject,
        recall: fixture.recall,
        skills: None,
        tools: None,
    };
    (
        Engine::with_runtime(EngineConfig::default(), Arc::new(runtime)),
        capture,
    )
}

async fn dispatch(engine: &Engine, method: &str, params: Value) -> Response {
    let request = parse_line(
        &json!({"id": "t", "jsonrpc": "2.0", "method": method, "params": params}).to_string(),
    )
    .expect("request line parses");
    engine.dispatch(&request).await
}

fn result(response: &Response) -> &Value {
    response.result.as_ref().expect("response carries a result")
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("response carries an error member")
}

/// A payload that passes every gate downstream of the chat-capture gate:
/// a session-bearing envelope, a valid `capture_id`, a non-blank
/// `project_id`, and the transcript/metadata pair a real idle capture sends.
fn valid_capture_payload() -> Value {
    json!({
        "session_id": "ses-gate",
        "capture_id": "019a0000-0000-7f00-8000-000000000098",
        "project_id": "/probe/project",
        "agent_id": "agent-1",
        "transcript": "fix the login bug",
        "metadata": { "promptId": "prompt-gate" }
    })
}

// ---------------------------------------------------------------------------
// C7 / AC4 — a flag-off engine refuses a valid capture and stores nothing
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_refused_when_chat_message_disabled() {
    let dir = TestDir::new("gate-disabled");
    let (engine, capture) = gated(dir.path(), false).await;

    let response = dispatch(&engine, "capture", valid_capture_payload()).await;

    assert_eq!(
        result(&response),
        &json!({"accepted": false, "reason": "chat_message_disabled"}),
        "a flag-off engine refuses on the normal result channel"
    );
    assert_eq!(
        capture.inbox().queued_count().await.expect("count"),
        0,
        "the refusal precedes the enqueue, so nothing is queued"
    );
}

// ---------------------------------------------------------------------------
// AC4b — the gate precedes PAYLOAD validation (the envelope gate still runs
// first: that one belongs to `dispatch`)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_chat_capture_refusal_precedes_project_id_validation() {
    let dir = TestDir::new("gate-order");
    let (engine, _capture) = gated(dir.path(), false).await;

    // Session-bearing, so `check_session_id` passes; no `project_id` and no
    // `capture_id`, so the payload-validation blocks are the ONLY thing
    // between this request and the gate.
    let response = dispatch(&engine, "capture", json!({"session_id": "ses-gate"})).await;

    assert!(
        response.error.is_none(),
        "the refusal is a RESULT, not an error: {:?}",
        response.error
    );
    assert_eq!(
        result(&response),
        &json!({"accepted": false, "reason": "chat_message_disabled"}),
        "a malformed payload still gets the refusal, never -32602"
    );
}

// ---------------------------------------------------------------------------
// C1 / AC5 — the hello result advertises the resolved value OUTSIDE
// `capabilities`, whose fourteen-key invariant is untouched
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_hello_advertises_chat_message_enabled_outside_capabilities() {
    let dir = TestDir::new("gate-hello");
    let (engine, _capture) = gated(dir.path(), false).await;

    let response = dispatch(&engine, "hello", json!({"client": "probe"})).await;
    let result = result(&response);

    assert_eq!(
        result["config"]["chat_message_enabled"],
        Value::Bool(false),
        "hello advertises the same resolved value `capture` enforces"
    );
    assert_eq!(
        result["capabilities"]
            .as_object()
            .expect("capabilities is an object")
            .len(),
        14,
        "the capability map keeps exactly its fourteen method keys"
    );
    assert!(
        result["capabilities"].get("chat_message_enabled").is_none(),
        "a config-derived value must never enter the capability map: {}",
        result["capabilities"]
    );
}

// ---------------------------------------------------------------------------
// R6 / AC11 — a runtime-less engine advertises `true` (the fail-open default)
// and still answers `capture` with the degraded error, never the refusal
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_runtime_less_engine_advertises_true_and_refuses_capture_as_degraded() {
    let engine = Engine::new(EngineConfig::default());

    let hello = dispatch(&engine, "hello", json!({"client": "probe"})).await;
    assert_eq!(
        result(&hello)["config"]["chat_message_enabled"],
        json!(true),
        "no runtime means no flag to read: the advertisement defaults to true"
    );

    let capture = dispatch(&engine, "capture", json!({"session_id": "ses-gate"})).await;
    assert_eq!(
        error_code(&capture),
        -32001,
        "a runtime-less engine degrades BEFORE the chat-capture gate"
    );
    assert!(
        capture.result.is_none(),
        "the degraded path never answers the refusal result: {:?}",
        capture.result
    );
}

// ---------------------------------------------------------------------------
// C5 / AC7 — exactly one `capture.refused` line per engine process
//
// The child runs in a CHILD of this test binary (the
// `profile_decay_lifecycle.rs` pattern) because `structured_stderr` writes to
// the raw descriptor, which libtest's capture never intercepts.
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
#[ignore = "runs only as the child of test_first_refused_capture_names_it_once"]
async fn chat_capture_refused_child() {
    let dir = TestDir::new("gate-refused-child");
    let (engine, _capture) = gated(dir.path(), false).await;
    // TWO refusals of a session-bearing payload: the envelope gate passes, so
    // every line the child writes comes from the gate under test.
    for _ in 0..2 {
        let response = dispatch(&engine, "capture", json!({"session_id": "ses-once"})).await;
        assert!(
            response.error.is_none(),
            "the refusal is a result, not an error: {:?}",
            response.error
        );
    }
}

#[test]
fn test_first_refused_capture_names_it_once() {
    let dir = TestDir::new("gate-refused-once");
    let text = common::run_captured("chat_capture_refused_child", &dir);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\"event\":\"capture.refused\""))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "two refusals must produce exactly one latched line:\n{text}"
    );
    assert!(
        lines[0].contains("\"reason\":\"chat_message_disabled\""),
        "the line names the reason: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("\"component\":\"capture\""),
        "the line names its component: {}",
        lines[0]
    );
}

// ---------------------------------------------------------------------------
// AC2 (engine half) — the gate is a gate: with the flag on, a valid capture is
// accepted exactly as before the wave. Characterization row; its mutant is a
// gate that ignores the getter and refuses unconditionally.
//
// No queue-depth assertion: `call_capture` spawns `drain_once()` fire-and-forget
// after the awaited enqueue, so the next `await` can observe the consumed
// queue. The ACK — emitted only after `enqueue` returns `Ok(())` — is the
// durable proof.
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_accepted_when_chat_messages_are_on() {
    let dir = TestDir::new("gate-enabled");
    let (engine, _capture) = gated(dir.path(), true).await;

    let response = dispatch(&engine, "capture", valid_capture_payload()).await;

    assert_eq!(
        result(&response),
        &json!({"accepted": true}),
        "the flag-on path keeps the pre-wave ACK exactly"
    );
}

// ---------------------------------------------------------------------------
// C8 / AC9 — `add` and manual writes stay ungated. The chat-capture gate covers
// the CHAT path only; a later wave "fixing" this by gating every write would
// break explicit memory writes, and this row is what fails.
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_add_and_manual_writes_ignore_the_chat_capture_gate() {
    let resolver = Arc::new(MapScopeResolver::new(
        vec![(
            "/probe/project",
            MapScopeResolver::project_tag("1111111111111111"),
        )],
        Some("probe@example.com"),
    ));
    let fixture = common::build_wire_fixture_gated(
        TestDir::new("gate-add-boundary"),
        resolver,
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        false,
    )
    .await;

    // The gate is on for this fixture: a chat capture is refused...
    let refused = dispatch(&fixture.engine, "capture", valid_capture_payload()).await;
    assert_eq!(
        result(&refused),
        &json!({"accepted": false, "reason": "chat_message_disabled"}),
        "the same engine refuses the chat path"
    );

    // ...while an explicit memory write still lands.
    let added = dispatch(
        &fixture.engine,
        "add",
        json!({
            "session_id": "session-add",
            "project_id": "/probe/project",
            "content": "manual write",
        }),
    )
    .await;
    assert!(
        added.error.is_none(),
        "`add` must not be gated by chatMessage.enabled: {:?}",
        added.error
    );
    let id = result(&added)["id"]
        .as_str()
        .expect("add returns its id")
        .to_string();
    assert!(
        id.starts_with("mem_") && id.len() == 4 + 13 + 1 + 9,
        "the upstream id shape mem_<millis>_<9hex>, got {id}"
    );

    let listed = dispatch(
        &fixture.engine,
        "list",
        json!({
            "session_id": "session-add",
            "project_id": "/probe/project",
            "scope": "project",
        }),
    )
    .await;
    assert_eq!(
        result(&listed)["count"],
        1,
        "the manual write is readable back"
    );
}
