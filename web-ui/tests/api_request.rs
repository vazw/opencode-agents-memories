//! The fetch seam's URL construction. Root-relative only: the builder never
//! produces an absolute or protocol-relative URL, and it adds no scope,
//! project or session parameter of its own.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::api_request::{RequestSpec, build_url};

#[wasm_bindgen_test]
fn test_request_url_is_root_relative_and_same_origin() {
    let url = build_url(&RequestSpec::get("/api/stats")).expect("a valid path builds");
    assert!(url.starts_with("/api/"), "URL must be root-relative: {url}");
    assert!(
        !url.starts_with("//"),
        "URL must not be protocol-relative: {url}"
    );
    assert!(!url.contains("://"), "URL must not carry a scheme: {url}");
    assert!(
        !url.contains("evil.example"),
        "URL must not carry a host: {url}"
    );
}

#[wasm_bindgen_test]
fn test_request_url_rejects_an_absolute_url() {
    let result = build_url(&RequestSpec::get("https://evil.example/api/stats"));
    assert!(
        result.is_err(),
        "an absolute URL must be refused: {result:?}"
    );
    assert!(build_url(&RequestSpec::get("http://127.0.0.1:4747/api/stats")).is_err());
}

#[wasm_bindgen_test]
fn test_request_url_rejects_a_protocol_relative_url() {
    let result = build_url(&RequestSpec::get("//evil.example/api/stats"));
    assert!(
        result.is_err(),
        "a protocol-relative URL must be refused: {result:?}"
    );
}

#[wasm_bindgen_test]
fn test_request_url_encodes_the_query_values() {
    let spec = RequestSpec::get("/api/search").with_query("q", "a b&c");
    let url = build_url(&spec).expect("a valid path builds");
    assert_eq!(url, "/api/search?q=a%20b%26c");
    assert!(!url.contains(' '));
}

#[wasm_bindgen_test]
fn test_request_url_adds_no_scope_parameter() {
    let spec = RequestSpec::get("/api/memories").with_query("limit", "10");
    let url = build_url(&spec).expect("a valid path builds");
    assert_eq!(url, "/api/memories?limit=10");
    assert_eq!(url.matches('?').count(), 1);
    assert_eq!(url.matches('=').count(), 1);
    for injected in ["project_id", "session_id", "scope", "project", "session"] {
        assert!(
            !url.contains(injected),
            "the builder must not append {injected}: {url}"
        );
    }
}
