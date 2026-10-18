//! Port of upstream `jsonc.test.ts` through the `parse_jsonc` seam.
//!
//! Upstream strips JSONC comments and then applies strict `JSON.parse`, so
//! only comments and trailing commas are tolerated. These tests assert
//! string/comment preservation plus the strictness guards.

use serde_json::{Value, json};

use mem_core::config::parse_jsonc;
mod common;

fn object_with(key: &str, value: &str) -> (Value, String) {
    let mut map = serde_json::Map::new();
    map.insert(key.to_owned(), Value::String(value.to_owned()));
    let expected = Value::Object(map);
    let input = serde_json::to_string(&expected).unwrap();
    (expected, input)
}

#[test]
fn preserves_comma_like_sequences_inside_string_values_and_keys() {
    for value in [",}", ",]", ", }", ", ]", "say \"a, }b\"", "\\path,]end"] {
        let (expected, input) = object_with(value, value);
        let parsed = parse_jsonc(&input)
            .unwrap_or_else(|e| panic!("value {value:?} must survive parsing: {e}"));
        assert_eq!(parsed, expected, "value {value:?} round-tripped");
    }
}

#[test]
fn handles_escaped_backslashes_before_closing_quote() {
    // Literal file text: {"path":"C:\\cache\\","next":"value"} — a backslash
    // immediately before the closing quote must not swallow the quote.
    let input = r#"{"path":"C:\\cache\\","next":"value"}"#;
    let expected = json!({ "path": "C:\\cache\\", "next": "value" });
    let parsed = parse_jsonc(input).unwrap();
    assert_eq!(parsed, expected);
}

#[test]
fn keeps_comment_like_text_inside_strings() {
    let expected = json!({
        "url": "https://example.invalid/a//b",
        "text": "/* keep, } */",
    });
    let input = serde_json::to_string(&expected).unwrap();
    let parsed = parse_jsonc(&input).unwrap();
    assert_eq!(parsed, expected);
}

#[test]
fn removes_structural_trailing_commas_separated_by_comments_and_whitespace() {
    let input = r#"{
        // A line comment
        "stop": [",}", ",]", /* before array end */ ],
        "nested": {"enabled": true, // before object end
        },
    }"#;
    let parsed = parse_jsonc(input).unwrap();
    assert_eq!(
        parsed,
        json!({
            "stop": [",}", ",]"],
            "nested": { "enabled": true },
        })
    );
}

#[test]
fn keeps_ordinary_separators_between_array_and_object_entries() {
    let expected = json!({
        "first": [1, 2, 3],
        "second": { "a": true, "b": false },
    });
    let input = serde_json::to_string(&expected).unwrap();
    let parsed = parse_jsonc(&input).unwrap();
    assert_eq!(parsed, expected);
}

#[test]
fn rejects_missing_commas_between_entries() {
    // Upstream tolerates only comments + trailing commas; missing separators
    // would fail `JSON.parse`.
    let error = parse_jsonc(r#"{"a": true, "b": false "c": 1}"#)
        .expect_err("missing commas must be rejected");
    assert!(
        error.to_string().contains("invalid JSONC:"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_single_quoted_strings() {
    let error = parse_jsonc("{'a': true}").expect_err("single quotes must be rejected");
    assert!(
        error.to_string().contains("invalid JSONC:"),
        "unexpected error: {error}"
    );
}

#[test]
fn rejects_empty_input_like_json_parse() {
    // serde_json + jsonc-parser deserialize empty input as `null`, i.e. not an
    // object — the same shape a config loader treats as "did not parse".
    let parsed = parse_jsonc("   \n  ").unwrap();
    assert_eq!(parsed, Value::Null);
}
