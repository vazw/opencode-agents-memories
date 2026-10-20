//! Port of `tests/profile-utils.test.ts` (4 upstream cases).
//!
//! Upstream's `safeArray`/`safeObject` from the same file are **not** ported as
//! free functions: this port's `ProfileItem::evidence` is `Option<Vec<String>>`
//! and `ProfileItem::evidence_or_empty()` already owns the "missing/invalid
//! evidence ⇒ empty" coercion, so a second path would be dead code
//! (registry disposition: `covered-by`).
//!
//! Tests:
//! - `test_strip_profile_embeddings_removes_centroid_and_anchor_from_every_section`
//! - `test_strip_profile_embeddings_tolerates_missing_malformed_and_non_object_sections`
//! - `test_to_public_profile_data_clones_before_stripping`
//! - `test_to_public_profile_data_serializes_without_embedding_keys`

use serde_json::{Value, json};

use mem_profile::profile_utils::{strip_profile_embeddings, to_public_profile_data};

fn sample_profile() -> Value {
    json!({
        "preferences": [
            {"description": "a", "centroid": [1, 2], "anchor": [3, 4], "confidence": 0.5}
        ],
        "patterns": [
            {"description": "b", "centroid": [1], "anchor": [2], "frequency": 3}
        ],
        "workflows": [
            {"description": "c", "centroid": [1], "anchor": [2], "steps": ["x"]}
        ]
    })
}

#[test]
fn test_strip_profile_embeddings_removes_centroid_and_anchor_from_every_section() {
    let mut data = sample_profile();

    let returned = strip_profile_embeddings(&mut data);

    assert!(
        std::ptr::eq(returned as *const Value, &data as *const Value),
        "stripping mutates in place and returns the same object"
    );
    for section in ["preferences", "patterns", "workflows"] {
        for item in data[section].as_array().expect("array section") {
            assert!(item.get("centroid").is_none(), "{section} centroid removed");
            assert!(item.get("anchor").is_none(), "{section} anchor removed");
        }
    }
    assert_eq!(data["preferences"][0]["confidence"], json!(0.5));
    assert_eq!(data["patterns"][0]["frequency"], json!(3));
    assert_eq!(data["workflows"][0]["steps"], json!(["x"]));
}

#[test]
fn test_strip_profile_embeddings_tolerates_missing_malformed_and_non_object_sections() {
    // A non-object root is a no-op (upstream `!data || typeof data !== "object"`).
    let mut null_root = Value::Null;
    assert_eq!(*strip_profile_embeddings(&mut null_root), Value::Null);

    let mut empty = json!({});
    assert_eq!(*strip_profile_embeddings(&mut empty), json!({}));

    let mut not_an_array = json!({"preferences": "not-an-array"});
    assert_eq!(
        *strip_profile_embeddings(&mut not_an_array),
        json!({"preferences": "not-an-array"})
    );

    // Non-object array elements are skipped silently and the object elements
    // around them are still stripped.
    let mut mixed = json!({
        "patterns": [
            null,
            1,
            "x",
            {"description": "kept", "centroid": [1], "anchor": [2], "frequency": 1}
        ]
    });
    let stripped = strip_profile_embeddings(&mut mixed);
    assert_eq!(stripped["patterns"][0], Value::Null);
    assert_eq!(stripped["patterns"][1], json!(1));
    assert_eq!(stripped["patterns"][2], json!("x"));
    assert!(stripped["patterns"][3].get("centroid").is_none());
    assert!(stripped["patterns"][3].get("anchor").is_none());
    assert_eq!(stripped["patterns"][3]["description"], json!("kept"));
}

#[test]
fn test_to_public_profile_data_clones_before_stripping() {
    let data = sample_profile();

    let result = to_public_profile_data(&data);

    assert_ne!(result, data, "the clone is a different object");
    assert_eq!(
        data["preferences"][0]["centroid"],
        json!([1, 2]),
        "the original keeps its embeddings"
    );
    assert_eq!(data["preferences"][0]["anchor"], json!([3, 4]));
    assert!(result["preferences"][0].get("centroid").is_none());
    assert!(result["preferences"][0].get("anchor").is_none());
    assert!(result["patterns"][0].get("centroid").is_none());
    assert!(result["workflows"][0].get("anchor").is_none());
    assert_eq!(result["preferences"][0]["confidence"], json!(0.5));
}

#[test]
fn test_to_public_profile_data_serializes_without_embedding_keys() {
    let serialized = serde_json::to_string(&to_public_profile_data(&sample_profile()))
        .expect("serialize public profile");

    assert!(!serialized.contains("\"centroid\""));
    assert!(!serialized.contains("\"anchor\""));
    assert!(serialized.contains("\"description\":\"a\""));
}
