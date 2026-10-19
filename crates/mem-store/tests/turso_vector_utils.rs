//! Vector-utils battery (blueprint W1.3 step 2): port of upstream
//! `tests/turso-vector-utils.test.ts` (3 cases + 3 boundary extras) over
//! `src/vector_utils.rs` — the pure helper half of upstream
//! `src/services/turso/vector-utils.ts`.

mod common;

use mem_store::db::Value;
use mem_store::vector_utils::{
    blob_to_float32_array, distance_to_similarity, escape_like_pattern, format_tags_for_embedding,
    parse_extracted_vector, parse_session_id_from_metadata, tokenize_query_text,
};

// ---------------------------------------------------------------------------
// 1/6
// ---------------------------------------------------------------------------

#[test]
fn test_clamps_cosine_distance_float_artifacts_into_unit_similarity() {
    assert_eq!(distance_to_similarity(0.0), 1.0);
    assert_eq!(distance_to_similarity(1.0), 0.0);
    assert_eq!(distance_to_similarity(1.0000001), 0.0);
    assert_eq!(distance_to_similarity(-1e-9), 1.0);
    assert_eq!(distance_to_similarity(f64::NAN), 0.0);
    assert_eq!(distance_to_similarity(f64::INFINITY), 0.0);
}

// ---------------------------------------------------------------------------
// 2/6
// ---------------------------------------------------------------------------

#[test]
fn test_formats_tag_embedding_text_as_topics_list() {
    assert_eq!(
        format_tags_for_embedding(&["a".to_string(), "b".to_string()]),
        "Topics: a, b"
    );
    assert_eq!(format_tags_for_embedding(&[]), "Topics: ");
}

// ---------------------------------------------------------------------------
// 3/6
// ---------------------------------------------------------------------------

#[test]
fn test_tokenizes_query_text_and_caps_at_sixteen_tokens() {
    assert_eq!(
        tokenize_query_text(Some("Hello, world_test")),
        vec!["hello".to_string(), "world".to_string(), "test".to_string()]
    );
    assert_eq!(tokenize_query_text(None), Vec::<String>::new());
    assert_eq!(tokenize_query_text(Some("")), Vec::<String>::new());
    // Separators: whitespace `,` `.` `_` `/` `-` collapse; case folds; single chars drop.
    assert_eq!(
        tokenize_query_text(Some("Foo bar.baz_qux/quux-corge")),
        vec![
            "foo".to_string(),
            "bar".to_string(),
            "baz".to_string(),
            "qux".to_string(),
            "quux".to_string(),
            "corge".to_string()
        ]
    );
    assert_eq!(tokenize_query_text(Some("a I 1 x")), Vec::<String>::new());
    // Cap at 16 tokens.
    let many = tokenize_query_text(Some(
        "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen",
    ));
    assert_eq!(many.len(), 16);
    assert_eq!(many[0], "one");
    assert_eq!(many[15], "sixteen");
    // Upstream `parseSessionIdFromMetadata` moved into vector_utils (single home).
    assert_eq!(
        parse_session_id_from_metadata(Some(r#"{"sessionID":"s1"}"#)),
        Some("s1".to_string())
    );
    assert_eq!(parse_session_id_from_metadata(Some("{bad")), None);
    assert_eq!(parse_session_id_from_metadata(None), None);
}

// ---------------------------------------------------------------------------
// 4/6
// ---------------------------------------------------------------------------

#[test]
fn test_escapes_like_wildcards_in_backslash_first_order() {
    assert_eq!(escape_like_pattern(r"a\b%c_d"), r"a\\b\%c\_d");
    // A lone `%` / `_` escape cleanly; a pre-escaped sequence is not double-escaped.
    assert_eq!(escape_like_pattern("100%_done"), r"100\%\_done");
    assert_eq!(escape_like_pattern("plain"), "plain");
}

// ---------------------------------------------------------------------------
// 5/6
// ---------------------------------------------------------------------------

#[test]
fn test_parses_extracted_vector_from_text_and_blob_fallbacks() {
    // Text JSON array parses losslessly into f32.
    let parsed =
        parse_extracted_vector(&Value::Text("[1,0.5]".to_string())).expect("text JSON parses");
    assert_eq!(parsed, vec![1.0f32, 0.5]);
    // Empty array is rejected.
    assert!(parse_extracted_vector(&Value::Text("[]".to_string())).is_none());
    // Non-JSON text is rejected.
    assert!(parse_extracted_vector(&Value::Text("nope".to_string())).is_none());

    // Raw F32 LE blob round-trip: [1.0, 0.5] as little-endian bytes.
    let mut bytes = Vec::new();
    for value in [1.0f32, 0.5] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let from_blob = parse_extracted_vector(&Value::Blob(bytes)).expect("LE blob parses");
    assert_eq!(from_blob, vec![1.0f32, 0.5]);

    // A 3-byte blob (not a multiple of 4) is rejected.
    assert!(parse_extracted_vector(&Value::Blob(vec![1, 2, 3])).is_none());
}

// ---------------------------------------------------------------------------
// 6/6
// ---------------------------------------------------------------------------

#[test]
fn test_blob_to_float32_array_rejects_null_integer_and_empty() {
    assert!(blob_to_float32_array(&Value::Null).is_none());
    assert!(blob_to_float32_array(&Value::Integer(42)).is_none());
    assert!(blob_to_float32_array(&Value::Blob(Vec::new())).is_none());
    // A blob whose length is not a multiple of 4 is rejected.
    assert!(blob_to_float32_array(&Value::Blob(vec![0; 6])).is_none());

    let mut bytes = Vec::new();
    for value in [1.0f32, -2.5, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(
        blob_to_float32_array(&Value::Blob(bytes)),
        Some(vec![1.0f32, -2.5, 0.0])
    );
}
