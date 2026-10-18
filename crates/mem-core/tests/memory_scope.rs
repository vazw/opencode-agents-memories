//! Port of `tests/memory-scope.test.ts` — the scope-resolution half.
//!
//! Upstream's `memoryScope` tests walk shards through the store layer
//! (Turso result counts, shard-id ordering: "defaults to project scope"
//! queries one shard, "uses config defaultScope when provided" and
//! "queries across shards" touch 3 shards, "lets tool params override
//! config" switches the walk). Those store-walking assertions ride the
//! store layer and are covered-by W1.2/W1.3 store tests; what is ported
//! here is the scope algebra they drive: the default selection is
//! `Project`, `all-projects` expands to user-then-project wildcard refs,
//! and an explicit request scope beats the config default.
//! Store-layer covering waves: W1.2 (shard inventory), W1.3 (client walk).

use mem_core::scope::{
    Scope, ScopeHash, ScopeRef, ScopeSelection, resolve_memory_scope, resolve_scope_selection,
};

const PROJECT_TAG: &str = "opencode_project_a1b2c3d4e5f67890";

#[test]
fn test_default_scope_walks_only_the_tag_project_scope() {
    // Upstream "defaults to project scope": no explicit argument and the
    // resolved config default `Project` → exactly one ref, the tag's own
    // project scope + hash; no user walk.
    let selection = resolve_scope_selection(None, ScopeSelection::Project);
    assert_eq!(selection, ScopeSelection::Project);
    let refs = resolve_memory_scope(selection, PROJECT_TAG).unwrap();
    assert_eq!(
        refs,
        vec![ScopeRef {
            scope: Scope::Project,
            hash: Some(ScopeHash::parse("a1b2c3d4e5f67890").unwrap()),
        }]
    );
}

#[test]
fn test_all_projects_selection_walks_user_and_project_scopes() {
    // Upstream "uses config defaultScope when provided" / "queries across
    // shards": the all-projects expansion yields the user and project
    // wildcard refs (the 3-shard result count itself is covered-by
    // W1.2/W1.3).
    let refs = resolve_memory_scope(ScopeSelection::AllProjects, PROJECT_TAG).unwrap();
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
fn test_explicit_scope_argument_overrides_config_default() {
    // Upstream "lets tool params override config".
    assert_eq!(
        resolve_scope_selection(Some(ScopeSelection::AllProjects), ScopeSelection::Project),
        ScopeSelection::AllProjects
    );
}

#[test]
fn test_all_projects_ref_order_places_user_scope_first() {
    // Upstream "queries across shards for all-projects": shard ids
    // `[shard-current, shard-a, shard-b]` walk user before project; the
    // ref order pins that.
    let refs = resolve_memory_scope(ScopeSelection::AllProjects, PROJECT_TAG).unwrap();
    assert_eq!(refs[0].scope, Scope::User);
    assert_eq!(refs[1].scope, Scope::Project);
}
