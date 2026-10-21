//! W5.3 — the shared scope-resolution trait: ONE seam feeding capture
//! (write), inject (read) and the scheduler's `PromotionScope`.
use crate::project_scope::ResolvedProjectScope;

pub trait ProjectScopeResolver: Send + Sync {
    fn resolve(
        &self,
        project_id: &str,
    ) -> Result<ResolvedProjectScope, crate::project_scope::ProjectScopeError>;
}

/// The `query` text conversion for the semantic read (unchanged text — the
/// embedder owns the task prefix).
pub fn query_text(query: &str) -> String {
    query.to_string()
}
