//! Port of upstream `src/services/memory-scope.ts` (subject of
//! `tests/memory-scope-helper.test.ts`) plus justified boundary extras.
//!
//! The seam is the pure scope algebra: hash validation (`/^[a-f0-9]{16}$/`),
//! container-tag parsing (`{prefix}_{user|project}_{16hex}`), and scope
//! resolution (`project` vs `all-projects`, the latter expanding to a
//! user-then-project wildcard pair so user-scope memories are never
//! silently excluded). Error-message fragments are byte-pinned where the
//! upstream tests match on `throw new Error(...)` text.

use mem_core::error::ScopeError;
use mem_core::scope::{
    Scope, ScopeHash, ScopeRef, ScopeSelection, extract_scope_from_container_tag,
    is_valid_scope_hash, resolve_memory_scope, try_extract_scope_from_container_tag,
};
use std::str::FromStr;

const PROJECT_TAG: &str = "opencode_project_a1b2c3d4e5f67890";
const USER_TAG: &str = "opencode_user_b1b2c3d4e5f67890";
const PROJECT_HASH: &str = "a1b2c3d4e5f67890";

#[test]
fn test_extract_parses_project_container_tag() {
    let r = extract_scope_from_container_tag(PROJECT_TAG).unwrap();
    assert_eq!(r.scope, Scope::Project);
    assert_eq!(r.hash.as_ref().map(ScopeHash::as_str), Some(PROJECT_HASH));
}

#[test]
fn test_extract_parses_user_container_tag() {
    let r = extract_scope_from_container_tag(USER_TAG).unwrap();
    assert_eq!(r.scope, Scope::User);
    assert_eq!(
        r.hash.as_ref().map(ScopeHash::as_str),
        Some("b1b2c3d4e5f67890")
    );
}

#[test]
fn test_resolve_all_projects_returns_user_then_project_wildcards() {
    let refs = resolve_memory_scope(ScopeSelection::AllProjects, "irrelevant").unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(
        refs[0],
        ScopeRef {
            scope: Scope::User,
            hash: None
        }
    );
    assert_eq!(
        refs[1],
        ScopeRef {
            scope: Scope::Project,
            hash: None
        }
    );
}

#[test]
fn test_resolve_project_scope_returns_container_tag_ref() {
    let refs = resolve_memory_scope(ScopeSelection::Project, PROJECT_TAG).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(
        refs[0],
        ScopeRef {
            scope: Scope::Project,
            hash: Some(ScopeHash::parse(PROJECT_HASH).unwrap()),
        }
    );
}

#[test]
fn test_is_valid_scope_hash_accepts_16_lowercase_hex_only() {
    assert!(is_valid_scope_hash("a1b2c3d4e5f67890"));
    assert!(!is_valid_scope_hash("abc123"));
    assert!(!is_valid_scope_hash("gggggggggggggggg"));
}

#[test]
fn test_extract_rejects_invalid_hash_with_16_lowercase_hex_message() {
    let err = extract_scope_from_container_tag("opencode_project_abc123").unwrap_err();
    assert!(matches!(err, ScopeError::InvalidContainerTagHash { .. }));
    assert!(err.to_string().contains("16 lowercase hex"));
}

#[test]
fn test_extract_rejects_path_traversal_segment_in_hash() {
    let err = extract_scope_from_container_tag("opencode_project_abcd1234567890ef/../../evil")
        .unwrap_err();
    assert!(matches!(err, ScopeError::InvalidContainerTagHash { .. }));
    assert!(err.to_string().contains("16 lowercase hex"));
}

#[test]
fn test_assert_safe_scope_hash_rejects_traversal_string() {
    use mem_core::scope::assert_safe_scope_hash;
    let err = assert_safe_scope_hash("../outside").unwrap_err();
    assert!(matches!(err, ScopeError::InvalidScopeHash { .. }));
    assert!(err.to_string().contains("Invalid scope hash"));
}

#[test]
fn test_try_extract_returns_none_for_legacy_tag_and_some_for_valid_tag() {
    assert!(try_extract_scope_from_container_tag("opencode_project_abc").is_none());
    let r = try_extract_scope_from_container_tag(PROJECT_TAG);
    assert!(r.is_some());
    assert_eq!(r.unwrap().scope, Scope::Project);
}

#[test]
fn test_extract_accepts_prefix_containing_extra_underscores() {
    // Only the last two segments are read; the prefix may contain '_'.
    let r = extract_scope_from_container_tag("a_b_c_project_a1b2c3d4e5f67890").unwrap();
    assert_eq!(r.scope, Scope::Project);
    assert_eq!(r.hash.as_ref().map(ScopeHash::as_str), Some(PROJECT_HASH));
}

#[test]
fn test_extract_rejects_tags_with_fewer_than_three_segments() {
    for tag in ["opencode_project", ""] {
        let err = extract_scope_from_container_tag(tag).unwrap_err();
        assert!(matches!(err, ScopeError::InvalidContainerTag { .. }));
        assert!(err.to_string().contains("expected format"));
    }
}

#[test]
fn test_extract_rejects_unknown_scope_segment() {
    let err = extract_scope_from_container_tag("opencode_workspace_a1b2c3d4e5f67890").unwrap_err();
    assert!(matches!(err, ScopeError::InvalidContainerTagScope { .. }));
    assert!(err.to_string().contains("workspace"));
}

#[test]
fn test_is_valid_scope_hash_rejects_uppercase_wrong_lengths_and_empty() {
    assert!(!is_valid_scope_hash("A1B2C3D4E5F67890"));
    assert!(!is_valid_scope_hash("a1b2c3d4e5f6789")); // 15
    assert!(!is_valid_scope_hash("a1b2c3d4e5f678900")); // 17
    assert!(!is_valid_scope_hash(""));
}

#[test]
fn test_scope_hash_parse_roundtrips_through_display_and_from_str() {
    let parsed = ScopeHash::parse(PROJECT_HASH).unwrap();
    assert_eq!(parsed.as_str(), PROJECT_HASH);
    assert_eq!(parsed.to_string(), PROJECT_HASH);
    let roundtripped = ScopeHash::from_str(&parsed.to_string()).unwrap();
    assert_eq!(parsed, roundtripped);

    let err = ScopeHash::parse("NOT-VALID").unwrap_err();
    assert!(matches!(err, ScopeError::InvalidScopeHash { .. }));
}

#[test]
fn test_extract_rejects_separator_characters_in_hash() {
    // The two api-handlers traversal-tag shapes (upstream
    // "project_x_../../../../Temp/pwn" and "project_x_..\..\evil" families):
    // rejection happens at this seam, never downstream.
    assert!(extract_scope_from_container_tag("project_x_foo/bar").is_err());
    assert!(extract_scope_from_container_tag("project_x_..\\..\\evil").is_err());
}

#[test]
fn test_resolve_project_scope_propagates_container_tag_errors() {
    let err = resolve_memory_scope(ScopeSelection::Project, "not-a-tag").unwrap_err();
    assert!(matches!(err, ScopeError::InvalidContainerTag { .. }));
}
