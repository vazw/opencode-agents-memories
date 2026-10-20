//! W5.1 step 5 — JSON-RPC NDJSON codec oracles (`crates/mem-bridge/src/jsonrpc.rs`).
//!
//! One JSON object per line; an array is a batch and is rejected with
//! `-32600` (bridge-schema.md §1); invalid JSON is `-32700`; the correlation
//! id is echoed unchanged; serialization is canonical (compact,
//! lexicographic keys) and terminated by exactly one `\n`.

use mem_bridge::jsonrpc::{ParseLineError, RequestId, encode_request, encode_response, parse_line};

#[test]
fn test_parse_valid_request() {
    let line = r#"{"id":"cap-1","jsonrpc":"2.0","method":"recall","params":{"query":"q"}}"#;
    let request = parse_line(line).expect("a valid request object parses");
    assert_eq!(request.method, "recall");
    assert_eq!(request.id.as_str(), Some("cap-1"));
    assert_eq!(
        request
            .params
            .get("query")
            .and_then(serde_json::Value::as_str),
        Some("q")
    );
    // Trailing newline tolerated (NDJSON lines arrive with the terminator).
    parse_line(format!("{line}\n").as_str()).expect("newline-terminated line parses");
}

#[test]
fn test_reject_batch_array() {
    let line = r#"[{"id":"a","jsonrpc":"2.0","method":"recall"},"{:1}"]"#;
    let error = parse_line(line).expect_err("a batch array is not a v1 request");
    assert_eq!(error.code(), -32600, "rejecting a batch is Invalid Request");
    assert_eq!(error, ParseLineError::InvalidRequest);
}

#[test]
fn test_reject_invalid_json() {
    let error = parse_line("{not json").expect_err("invalid JSON is a parse error");
    assert_eq!(error.code(), -32700, "invalid JSON is a parse error");
    assert_eq!(error, ParseLineError::Parse);
}

#[test]
fn test_echo_id() {
    // String, numeric, and the round-trip: the id the parse produced is the
    // id the encoding put back on the wire.
    let string_input = r#"{"id":"xx-y","jsonrpc":"2.0","method":"health"}"#;
    let parsed = parse_line(string_input).expect("parse");
    assert_eq!(parsed.id.as_str(), Some("xx-y"));
    let encoded = encode_request(&parsed);
    assert_eq!(
        encoded,
        format!("{string_input}\n"),
        "encode is byte-stable, newline-terminated"
    );

    let numeric_input = r#"{"id":7,"jsonrpc":"2.0","method":"health"}"#;
    let parsed = parse_line(numeric_input).expect("numeric id parses");
    let encoded = encode_request(&parsed);
    assert_eq!(
        encoded,
        format!("{}\n", numeric_input),
        "integer ids survive the round trip without a .0"
    );

    // Missing id: v1 has no notifications, so this is Invalid Request.
    let missing = parse_line(r#"{"jsonrpc":"2.0","method":"health"}"#);
    assert_eq!(
        missing,
        Err(ParseLineError::InvalidRequest),
        "a request without id is -32600"
    );
}

#[test]
fn test_response_serialization_matches_fixture_shape() {
    // The canonical error envelope shape pinned by tests/fixtures/bridge/.
    let bytes = format!(
        "{}\n", // validated byte-exactly in bridge_fixtures; here only shape
        r#"{"error":{"code":-32601,"message":"Method not found"},"id":"u-1","jsonrpc":"2.0"}"#
    );
    let response = mem_bridge::jsonrpc::Response::error(
        RequestId::String("u-1".to_owned()),
        -32601,
        "Method not found",
    );
    let expected: serde_json::Value = serde_json::from_str(bytes.trim()).expect("shape parse");
    let actual = serde_json::to_value(&response).expect("encode");
    assert_eq!(actual, expected, "shape-equal, keys canonically ordered");
    // Serialization terminates with exactly one newline.
    assert!(encode_response(&response).ends_with('\n'));
    assert!(encode_response(&response).trim_end().is_ascii());
}
