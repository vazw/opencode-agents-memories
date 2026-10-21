//! W5.3 Step 2 — the one project-scope seam (ADR-0008 D-W5.3-1's
//! "recapture scope triple" anchor; security posture: `project_id` is a
//! selector, not a credential, and it is derived EXACTLY ONCE).
//!
//! Given the wrapper's host `project_id` (the immutable host directory),
//! this module derives `{container_tag, scope_hash}` through
//! `TagResolver::project_tag_info` once and validates the result through
//! `extract_scope_from_container_tag` once. No other code may re-hash the
//! path, hand-build a tag, fall back to an empty tag, or derive a second,
//! divergent hash: the empty tag means "all memories" to the public
//! `list_memories_by_container_tag`, so a fallback here would silently
//! widen scope to every row in the shard.

use std::path::{Path, PathBuf};

use mem_core::tags::{TagInfo, TagResolver};

/// The resolved scope every W5.3 consumer receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProjectScope {
    /// The original wire `project_id` (the episodic request key stays the
    /// host path; hashes are never substituted for it).
    pub project_id: String,
    /// The container tag exactly as `TagResolver` produced it.
    pub container_tag: String,
    /// The validated 16-hex scope hash (the tag's own last segment).
    pub scope_hash: String,
    /// The `TagInfo` extras the tier-1 writer copies verbatim.
    pub display_name: String,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<PathBuf>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
}

/// Why a scope resolution failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectScopeError {
    /// A missing or whitespace-only project scope is rejected, never
    /// coerced to a wildcard.
    BlankProjectId,
    /// The scope/tag machinery rejected the value (message text only —
    /// `ScopeError[non_exhaustive]` is not Clone across the crate seam).
    Core(String),
}

impl std::fmt::Display for ProjectScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlankProjectId => write!(f, "project scope must be a non-blank directory"),
            Self::Core(message) => write!(f, "project scope derivation failed: {message}"),
        }
    }
}

impl std::error::Error for ProjectScopeError {}

/// Test/composition helper: builds a `ResolvedProjectScope` from raw
/// pieces, running the same scope validation (used by fixtured resolvers).
pub fn apply_resolved_scope(
    project_id: &str,
    container_tag: &str,
    scope_hash: &str,
) -> Result<ResolvedProjectScope, ProjectScopeError> {
    if project_id.trim().is_empty() {
        return Err(ProjectScopeError::BlankProjectId);
    }
    let scope_ref = mem_core::scope::extract_scope_from_container_tag(container_tag)
        .map_err(|error| ProjectScopeError::Core(error.to_string()))?;
    let hash = match scope_ref.hash {
        Some(hash) if hash.as_str() == scope_hash => hash,
        _ => {
            return Err(ProjectScopeError::Core(format!(
                "resolved scope hash disagreement for tag {container_tag:?}"
            )));
        }
    };
    Ok(ResolvedProjectScope {
        project_id: project_id.to_string(),
        container_tag: container_tag.to_string(),
        scope_hash: hash.as_str().to_string(),
        display_name: project_id.to_string(),
        user_name: None,
        user_email: None,
        project_path: Some(PathBuf::from(project_id)),
        project_name: None,
        git_repo_url: None,
    })
}

/// `process_env` convenience resolver (the composition root's default).
pub struct EnvProjectScopeResolver {
    pub container_tag_prefix: String,
    pub user_email_override: Option<String>,
    pub user_name_override: Option<String>,
}

impl crate::scope_resolver::ProjectScopeResolver for EnvProjectScopeResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        let env = mem_core::env::ProcessEnv;
        let git = mem_core::tags::probe::SystemGitProbe::new(&env);
        let resolver = TagResolver::new(
            self.container_tag_prefix.as_str(),
            self.user_email_override.as_deref(),
            self.user_name_override.as_deref(),
            &env,
            &git,
        );
        resolve_project_scope(project_id, &resolver)
    }
}

/// Resolves the project scope: ONE `project_tag_info` call, then ONE scope
/// validation of the produced tag. The hash kept is the tag's own validated
/// hash segment — never a separately computed hash of `project_id`.
pub fn resolve_project_scope(
    project_id: &str,
    resolver: &TagResolver<'_>,
) -> Result<ResolvedProjectScope, ProjectScopeError> {
    if project_id.trim().is_empty() {
        return Err(ProjectScopeError::BlankProjectId);
    }
    let info: TagInfo = resolver.project_tag_info(Path::new(project_id));
    let scope_ref = match mem_core::scope::extract_scope_from_container_tag(&info.tag) {
        Ok(scope_ref) => scope_ref,
        Err(error) => return Err(ProjectScopeError::Core(error.to_string())),
    };
    let hash = match scope_ref.hash {
        Some(hash) => hash,
        None => {
            return Err(ProjectScopeError::Core(format!(
                "Invalid containerTag hash: expected 16 lowercase hex characters in \"{}\"",
                info.tag
            )));
        }
    };
    Ok(ResolvedProjectScope {
        project_id: project_id.to_string(),
        container_tag: info.tag,
        scope_hash: hash.as_str().to_string(),
        display_name: info.display_name,
        user_name: info.user_name,
        user_email: info.user_email,
        project_path: info.project_path,
        project_name: info.project_name,
        git_repo_url: info.git_repo_url,
    })
}
