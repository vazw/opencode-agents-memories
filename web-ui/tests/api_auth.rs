//! The auth seam: the server-injected local token and the optional Basic
//! credential. No header ever carries `Bearer`.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api::auth::{
    AuthContext, BasicCredential, MarkerReader, TOKEN_MARKER, build_headers, read_marker_token_with,
};
use web_ui::api_request::RequestSpec;

fn headers_for(spec: &RequestSpec, auth: &AuthContext) -> Vec<(String, String)> {
    build_headers(spec, auth).expect("header construction never fails for a well-formed spec")
}

fn value_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name == name)
        .map(|(_, value)| value.as_str())
}

struct FakeMarkerReader(Option<&'static str>);

impl MarkerReader for FakeMarkerReader {
    fn marker(&self) -> Option<String> {
        self.0.map(str::to_string)
    }
}

#[wasm_bindgen_test]
fn test_auth_headers_attach_the_local_token_when_present() {
    let auth = AuthContext {
        local_token: Some("local-token-value".to_string()),
        basic: None,
    };
    let headers = headers_for(&RequestSpec::get("/api/stats"), &auth);
    assert_eq!(
        value_of(&headers, "x-opencode-mem-token"),
        Some("local-token-value")
    );
    assert_eq!(value_of(&headers, "accept"), Some("application/json"));
}

#[wasm_bindgen_test]
fn test_auth_headers_omit_the_token_when_absent() {
    let headers = headers_for(&RequestSpec::get("/api/stats"), &AuthContext::default());
    assert_eq!(value_of(&headers, "x-opencode-mem-token"), None);
    assert_eq!(value_of(&headers, "accept"), Some("application/json"));
}

#[wasm_bindgen_test]
fn test_auth_headers_attach_the_basic_credential_when_provided() {
    let auth = AuthContext {
        local_token: None,
        basic: Some(BasicCredential::new("user", "password")),
    };
    let headers = headers_for(&RequestSpec::get("/api/stats"), &auth);
    assert_eq!(
        value_of(&headers, "authorization"),
        Some("Basic dXNlcjpwYXNzd29yZA==")
    );
}

#[wasm_bindgen_test]
fn test_auth_headers_never_use_the_bearer_scheme() {
    let auth = AuthContext {
        local_token: Some("local-token-value".to_string()),
        basic: Some(BasicCredential::new("user", "password")),
    };
    let headers = headers_for(&RequestSpec::post("/api/memories"), &auth);
    for (name, value) in &headers {
        assert!(
            !value.starts_with("Bearer "),
            "{name} must not carry a Bearer credential"
        );
    }
    assert!(headers.iter().all(|(_, value)| !value.contains("Bearer")));
}

#[wasm_bindgen_test]
fn test_auth_headers_set_the_json_content_type_only_for_a_body() {
    let without_body = headers_for(&RequestSpec::post("/api/memories"), &AuthContext::default());
    assert_eq!(value_of(&without_body, "content-type"), None);

    let with_body =
        RequestSpec::post("/api/memories").with_body(serde_json::json!({ "text": "hello" }));
    let headers = headers_for(&with_body, &AuthContext::default());
    assert_eq!(value_of(&headers, "content-type"), Some("application/json"));
}

#[wasm_bindgen_test]
fn test_basic_credential_encodes_user_and_password_as_base64() {
    assert_eq!(
        BasicCredential::new("user", "password").authorization_value(),
        "Basic dXNlcjpwYXNzd29yZA=="
    );
    assert_eq!(
        BasicCredential::new("alice", "s3cr3t").authorization_value(),
        "Basic YWxpY2U6czNjcjN0"
    );
}

#[wasm_bindgen_test]
fn test_marker_token_is_read_from_the_injected_window_marker() {
    let reader = FakeMarkerReader(Some("injected-token"));
    assert_eq!(
        read_marker_token_with(&reader),
        Some("injected-token".to_string())
    );
    assert_eq!(TOKEN_MARKER, "__OPENCODE_MEM_TOKEN__");
}

#[wasm_bindgen_test]
fn test_marker_token_is_absent_when_the_marker_is_unset() {
    assert_eq!(read_marker_token_with(&FakeMarkerReader(None)), None);
    assert_eq!(read_marker_token_with(&FakeMarkerReader(Some(""))), None);
}
