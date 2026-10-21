//! The typed envelope + the bounded error. Every failure path is bounded: the
//! raw response body is never echoed into the error text.
#![cfg(target_arch = "wasm32")]

use serde::Deserialize;
use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api::envelope::{ApiError, MAX_ERROR_MESSAGE_CHARS, parse_envelope};

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Item {
    id: u32,
    text: String,
}

#[wasm_bindgen_test]
fn test_envelope_returns_the_data_when_the_call_succeeded() {
    let bytes = br#"{"success":true,"data":{"id":7,"text":"hello"}}"#;
    let item: Item = parse_envelope(200, bytes).expect("a success envelope yields the data");
    assert_eq!(
        item,
        Item {
            id: 7,
            text: "hello".to_string()
        }
    );
}

#[wasm_bindgen_test]
fn test_envelope_returns_a_bounded_server_error_when_the_call_failed() {
    let bytes = br#"{"success":false,"error":"Unauthorized"}"#;
    let error = parse_envelope::<Item>(401, bytes).expect_err("a failure envelope is an error");
    assert_eq!(
        error,
        ApiError::Server {
            status: 401,
            message: "Unauthorized".to_string()
        }
    );
}

#[wasm_bindgen_test]
fn test_envelope_bounds_an_overlong_server_error() {
    let long_error = "e".repeat(5_000);
    let body = format!(r#"{{"success":false,"error":"{long_error}"}}"#);
    let error =
        parse_envelope::<Item>(500, body.as_bytes()).expect_err("a failure envelope is an error");
    match error {
        ApiError::Server { status, message } => {
            assert_eq!(status, 500);
            assert_eq!(message.chars().count(), MAX_ERROR_MESSAGE_CHARS);
            assert_eq!(message.chars().count(), 200);
        }
        other => panic!("expected a bounded server error, got {other:?}"),
    }
}

#[wasm_bindgen_test]
fn test_envelope_rejects_a_malformed_body_without_echoing_it() {
    let body = b"<html>SECRET-BODY-BYTES</html>";
    let error = parse_envelope::<Item>(200, body).expect_err("a non-JSON body is malformed");
    assert_eq!(error, ApiError::Malformed);

    let rendered = error.to_string();
    assert!(!rendered.contains("SECRET-BODY-BYTES"));
    assert!(!rendered.contains("<html>"));
    assert!(!rendered.contains("200"));
}

#[wasm_bindgen_test]
fn test_transport_error_carries_a_fixed_message() {
    let transport = ApiError::Transport.to_string();
    assert_eq!(transport, "network request failed");
    assert!(!transport.contains("TypeError"));
    assert!(!transport.contains("fetch"));

    let malformed = ApiError::Malformed.to_string();
    assert_eq!(malformed, "malformed request or response body");
    assert!(!malformed.contains("SECRET"));
}
