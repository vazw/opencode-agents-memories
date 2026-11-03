//! `decode_component` — the inverse of the single encoder.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::paths::{decode_component, encode_component};

#[wasm_bindgen_test]
fn test_decode_component_cases() {
    assert_eq!(decode_component("architecture%20docs"), "architecture docs");
    assert_eq!(decode_component("%2Fhome%2Fvaz"), "/home/vaz");
    assert_eq!(decode_component("plain"), "plain");
    assert_eq!(decode_component("a%2fb"), "a/b");
    // Not a space: the encoder never emits `+`.
    assert_eq!(decode_component("a+b"), "a+b");
}

#[wasm_bindgen_test]
fn test_decode_component_keeps_bad_escapes_literally() {
    assert_eq!(decode_component("100%"), "100%");
    assert_eq!(decode_component("a%2"), "a%2");
    assert_eq!(decode_component("%zz"), "%zz");
    assert_eq!(decode_component("%2"), "%2");
    assert_eq!(decode_component(""), "");
}

#[wasm_bindgen_test]
fn test_decode_component_never_lossy() {
    // 0xFF is never valid UTF-8: it stays percent-encoded.
    assert_eq!(decode_component("a%FFb"), "a%FFb");
    assert!(!decode_component("%C3").contains('\u{FFFD}'));
    // A valid multi-byte sequence decodes.
    assert_eq!(decode_component("%C3%A9"), "é");
}

#[wasm_bindgen_test]
fn test_decode_inverts_encode() {
    for original in [
        "architecture docs",
        "a&b=c/d",
        "ünï cödé",
        "quote\"tag",
        "100%",
        "",
    ] {
        assert_eq!(decode_component(&encode_component(original)), original);
    }
}
