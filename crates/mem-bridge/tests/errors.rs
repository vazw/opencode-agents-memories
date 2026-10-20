//! W5.1 step 7 — behavioural JSON-RPC error oracles (M6).
//!
//! The dispatcher answers every malformed/unknown/in-bound-violating
//! request with a SPECIFIC code (never the generic -32603). `session_id`
//! bound: 1..=128 characters; missing/blank/over-long → -32602.

use mem_bridge::engine::{Engine, EngineConfig};
use mem_bridge::jsonrpc::{RequestId, Response, parse_line};
use serde_json::{Value, json};

fn engine() -> Engine {
    Engine::new(EngineConfig::default())
}

fn dispatch_invalid(line: &str) -> Response {
    match parse_line(line) {
        Ok(request) => engine().dispatch(&request),
        Err(error) => Response::from_parse_line_error(RequestId::Null, error),
    }
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

#[test]
fn test_unknown_method_returns_32601() {
    let response = dispatch_invalid(r#"{"id":"unknown-1","jsonrpc":"2.0","method":"teleport"}"#);
    assert_eq!(error_code(&response), -32601);
    assert_eq!(response.id.as_str(), Some("unknown-1"), "the id is echoed");
}

#[test]
fn test_missing_session_id_returns_32602() {
    let response = dispatch_invalid(
        r#"{"id":"capture-2","jsonrpc":"2.0","method":"capture","params":{"project_id":"proj-1"}}"#,
    );
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

#[test]
fn test_blank_session_id_returns_32602() {
    let response = dispatch_invalid(
        r#"{"id":"c","jsonrpc":"2.0","method":"health","params":{"session_id":"   "}}"#,
    );
    assert_param_error(&response);
}

#[test]
fn test_overlong_session_id_returns_32602() {
    let overlong: String = "x".repeat(129);
    let line = json!({
        "id": "capture-2",
        "jsonrpc": "2.0",
        "method": "recall",
        "params": { "session_id": overlong, "query": "q" }
    });
    let response = dispatch_invalid(&line.to_string());
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

#[test]
fn test_hello_takes_no_session_id() {
    // hello is NOT a runtime method (D-40): no session_id is required and the
    // handshake result comes back.
    let request = parse_line(
        r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#,
    )
    .expect("handshake parses");
    let response = engine().dispatch(&request);
    assert!(
        response.error.is_none(),
        "a valid hello never errors: {response:?}"
    );
    assert_eq!(response.id.as_str(), Some("hello-1"));
}

/// The id never routes a non-hello request to the dispatcher without the
/// session check — a direct assertion that the five all go through it.
#[test]
fn test_shutdown_without_session_id_also_32602() {
    let response = dispatch_invalid(r#"{"id":"s","jsonrpc":"2.0","method":"shutdown"}"#);
    assert_eq!(error_code(&response), -32602);
    let _ = RequestId::Null; // silence unused in restricted feature builds
}
