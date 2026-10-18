//! Port of `tests/tool-scope.test.ts` — scope precedence:
//! `argument ?? config_default` (upstream `args.scope ?? CONFIG.memory.defaultScope`;
//! upstream's second fallback — the client's `scope: MemoryScope = "project"`
//! default parameter — is inside `config_default`, because the crate's
//! `Config` is always resolved with `memory.defaultScope` defaulting to
//! `Project`, W0.2). Plugin/tool wiring is covered-by W5.4.

use mem_core::config::MemoryDefaultScope;
use mem_core::scope::{ScopeSelection, resolve_scope_selection};
use std::convert::From;

#[test]
fn test_scope_falls_back_to_config_default_when_argument_absent() {
    assert_eq!(
        resolve_scope_selection(None, ScopeSelection::AllProjects),
        ScopeSelection::AllProjects
    );
}

#[test]
fn test_explicit_argument_scope_overrides_config_default() {
    assert_eq!(
        resolve_scope_selection(Some(ScopeSelection::Project), ScopeSelection::AllProjects),
        ScopeSelection::Project
    );
}

#[test]
fn test_scope_falls_back_to_project_when_config_default_is_project() {
    assert_eq!(
        resolve_scope_selection(None, ScopeSelection::Project),
        ScopeSelection::Project
    );
    // The resolved config default arrives as `MemoryDefaultScope` and maps
    // 1:1 onto the request-level selection.
    assert_eq!(
        ScopeSelection::from(MemoryDefaultScope::Project),
        ScopeSelection::Project
    );
}
