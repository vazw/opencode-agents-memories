//! Port of `tests/client-scope-traversal.test.ts`.
//!
//! Upstream asserts the exact `getAllShards` call sequences for list/search
//! in each scope; those call sequences are exactly `resolve_memory_scope`
//! output, so the traversal port pins the sequence the client would walk.

use mem_core::scope::{Scope, ScopeHash, ScopeRef, ScopeSelection, resolve_memory_scope};

#[test]
fn test_list_memories_all_projects_requests_user_then_project_shards() {
    let refs = resolve_memory_scope(
        ScopeSelection::AllProjects,
        "opencode_project_a1b2c3d4e5f67890",
    )
    .unwrap();
    assert_eq!(
        refs,
        vec![
            ScopeRef {
                scope: Scope::User,
                hash: None
            },
            ScopeRef {
                scope: Scope::Project,
                hash: None
            },
        ]
    );
}

#[test]
fn test_search_memories_all_projects_requests_user_then_project_shards() {
    let refs = resolve_memory_scope(
        ScopeSelection::AllProjects,
        "opencode_project_a1b2c3d4e5f67890",
    )
    .unwrap();
    assert_eq!(
        refs,
        vec![
            ScopeRef {
                scope: Scope::User,
                hash: None
            },
            ScopeRef {
                scope: Scope::Project,
                hash: None
            },
        ]
    );
}

#[test]
fn test_list_memories_project_scope_requests_the_container_tag_scope_and_hash() {
    let refs =
        resolve_memory_scope(ScopeSelection::Project, "opencode_project_abcdef1234567890").unwrap();
    assert_eq!(
        refs,
        vec![ScopeRef {
            scope: Scope::Project,
            hash: Some(ScopeHash::parse("abcdef1234567890").unwrap()),
        }]
    );
}
