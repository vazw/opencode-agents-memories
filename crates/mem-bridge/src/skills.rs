//! Issue 69 — the `skills` runtime method's service: the catalogue read over
//! the W6.4 [`SkillsManager`], shaped as the host's `Skill.Info` set.
//!
//! The engine owns `skills.db` and the `skills-content/{id}/SKILL.md` tree;
//! the plugin registers the catalogue on the host's skill domain. This module
//! is the wire-side adapter: one project-scoped, enabled-only read, mapped to
//! the exact field set `Skill.Info` declares (`docs/bridge-schema.md` §3).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_skills::{Skill, SkillsManager, skill_content_path};

use crate::project_scope::ProjectScopeError;
use crate::scope_resolver::ProjectScopeResolver;

/// The wire failure surface of the catalogue read.
pub enum CatalogueError {
    /// A blank `project_id` (the handler already answers `-32602`; kept so a
    /// future caller cannot bypass the validation).
    InvalidProject,
    /// The scope derivation or the store failed (`-32603`).
    Store(String),
}

/// `Path::is_absolute`, with `std::path::absolute` as the one fallback: the
/// storage root comes from config and MAY be relative (`--storage-root rel`),
/// while `Skill.Info.path` must be absolute. No filesystem access, no panic.
pub fn absolute_storage_root(root: &Path) -> PathBuf {
    if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf())
    }
}

/// The `skills` read: one manager, the shared scope resolver, and the storage
/// root absolutised ONCE in [`SkillsService::new`].
pub struct SkillsService {
    manager: Arc<SkillsManager>,
    resolver: Arc<dyn ProjectScopeResolver>,
    storage_root: PathBuf,
}

impl SkillsService {
    /// Builds the service over the composed manager and the one scope seam.
    pub fn new(manager: Arc<SkillsManager>, resolver: Arc<dyn ProjectScopeResolver>) -> Self {
        let storage_root = absolute_storage_root(manager.storage_path());
        Self {
            manager,
            resolver,
            storage_root,
        }
    }

    /// The project-scoped catalogue: enabled skills tagged with the project,
    /// plus the global (`container_tag IS NULL`) ones.
    pub async fn catalogue(
        &self,
        project_id: &str,
    ) -> Result<Vec<serde_json::Value>, CatalogueError> {
        let scope = self
            .resolver
            .resolve(project_id)
            .map_err(|error| match error {
                ProjectScopeError::BlankProjectId => CatalogueError::InvalidProject,
                ProjectScopeError::Core(_) => {
                    CatalogueError::Store("scope derivation failed".to_owned())
                }
            })?;
        let skills = self
            .manager
            .list(Some(&scope.container_tag), true)
            .await
            .map_err(|error| CatalogueError::Store(error.to_string()))?;
        Ok(skills
            .iter()
            .filter_map(|skill| self.entry(skill))
            .collect())
    }

    /// One `Skill.Info`-shaped entry. `None` for the two impossible-but-tolerated
    /// shapes: an id the content helper refuses (DB corruption; ids are validated
    /// on write) and a non-UTF-8 path (the wire is UTF-8 JSON).
    fn entry(&self, skill: &Skill) -> Option<serde_json::Value> {
        let path = skill_content_path(&self.storage_root, &skill.id).ok()?;
        let mut entry = serde_json::json!({
            "id": skill.id,
            "name": skill.name,
            "path": path.to_str()?,
            "content": skill.content,
            "autoinvoke": skill.autoinvoke,
        });
        if !skill.description.is_empty() {
            entry["description"] = serde_json::Value::String(skill.description.clone());
        }
        Some(entry)
    }
}
