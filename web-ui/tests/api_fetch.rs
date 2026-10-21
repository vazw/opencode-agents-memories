//! The fetch seam: the pure `prepare`/`finish`/`map_transport` seams and the
//! `init_for` shape. The live `fetch` itself is the adapter exercised by the
//! Step 9 browser check.
#![cfg(target_arch = "wasm32")]

use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;
use web_sys::{Headers, RequestCredentials, RequestInit, RequestMode};
use web_ui::api::auth::AuthContext;
use web_ui::api::envelope::ApiError;
use web_ui::api_request::{RequestSpec, finish, init_for, map_transport, prepare};

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Item {
    id: u32,
}

fn prepared(spec: &RequestSpec, auth: &AuthContext) -> (String, RequestInit) {
    prepare::<Item>(spec, auth).expect("prepare succeeds for a well-formed spec")
}

#[wasm_bindgen_test]
fn test_request_uses_same_origin_mode() {
    let (_, init) = prepared(&RequestSpec::get("/api/stats"), &AuthContext::anonymous());
    assert_eq!(init.get_mode(), Some(RequestMode::SameOrigin));
}

#[wasm_bindgen_test]
fn test_request_uses_same_origin_credentials() {
    let (_, init) = prepared(&RequestSpec::get("/api/stats"), &AuthContext::anonymous());
    assert_eq!(init.get_credentials(), Some(RequestCredentials::SameOrigin));
}

#[wasm_bindgen_test]
fn test_request_attaches_auth_headers() {
    let auth = AuthContext {
        local_token: Some("local-token-value".to_string()),
        basic: None,
    };
    let (_, init) = prepared(&RequestSpec::get("/api/stats"), &auth);
    let headers: Headers = init
        .get_headers()
        .dyn_into()
        .expect("prepare attaches a Headers object");
    assert_eq!(
        headers.get("x-opencode-mem-token").unwrap().as_deref(),
        Some("local-token-value")
    );
    assert_eq!(
        headers.get("accept").unwrap().as_deref(),
        Some("application/json")
    );
}

#[wasm_bindgen_test]
fn test_request_serializes_the_json_body() {
    let spec = RequestSpec::post("/api/memories").with_body(serde_json::json!({ "text": "hello" }));
    let (_, init) = prepared(&spec, &AuthContext::anonymous());

    let body = init
        .get_body()
        .as_string()
        .expect("the body is a JSON string");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("the body is valid JSON");
    assert_eq!(parsed, serde_json::json!({ "text": "hello" }));
    assert_eq!(init.get_method().as_deref(), Some("POST"));

    let headers: Headers = init
        .get_headers()
        .dyn_into()
        .expect("prepare attaches a Headers object");
    assert_eq!(
        headers.get("content-type").unwrap().as_deref(),
        Some("application/json")
    );
}

#[wasm_bindgen_test]
fn test_request_parses_a_success_envelope() {
    let body = br#"{"success":true,"data":{"id":7}}"#;
    let item: Item = finish(200, body).expect("a 200 success envelope yields the data");
    assert_eq!(item, Item { id: 7 });
}

#[wasm_bindgen_test]
fn test_request_parses_a_server_error_envelope() {
    let body = br#"{"success":false,"error":"Unauthorized"}"#;
    let error = finish::<Item>(401, body).expect_err("a failure envelope is an error");
    assert_eq!(
        error,
        ApiError::Server {
            status: 401,
            message: "Unauthorized".to_string()
        }
    );

    let error = finish::<Item>(503, br#"{"success":false,"error":"upstream down"}"#)
        .expect_err("a 5xx is an error");
    assert_eq!(
        error,
        ApiError::Server {
            status: 503,
            message: "upstream down".to_string()
        }
    );
}

#[wasm_bindgen_test]
fn test_request_maps_a_network_failure_to_transport() {
    let error = map_transport(JsValue::from_str("TypeError: Failed to fetch"));
    assert_eq!(error, ApiError::Transport);
    let rendered = error.to_string();
    assert_eq!(rendered, "network request failed");
    assert!(!rendered.contains("TypeError"));
    assert!(!rendered.contains("Failed"));
}

#[wasm_bindgen_test]
fn test_init_for_is_pure_and_carries_no_token() {
    let init = init_for(&RequestSpec::get("/api/stats"));
    assert_eq!(init.get_mode(), Some(RequestMode::SameOrigin));
    assert_eq!(init.get_credentials(), Some(RequestCredentials::SameOrigin));
    assert_eq!(init.get_method().as_deref(), Some("GET"));
    assert!(init.get_body().is_undefined() || init.get_body().is_null());

    if init.get_headers().is_object() {
        let headers: Headers = init.get_headers().unchecked_into();
        assert_eq!(headers.get("authorization").unwrap(), None);
        assert_eq!(headers.get("x-opencode-mem-token").unwrap(), None);
    }
}
