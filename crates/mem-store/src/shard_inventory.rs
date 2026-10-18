//! Shard inventory — port of upstream
//! `src/services/shard-inventory-service.ts` (blueprint W1.2 step 11).
//! Registry + disk-scan union listing, `deriveStatus` precedence,
//! `resolveMigrationSource` ordering with the upstream error fragments.
//!
//! Divergence (reviewer note): upstream `{success, error, candidates}`
//! envelopes become typed `Result<_, ResolveSourceError>` — JSON remap at
//! the tools/API wave. `ensureTursoReady` is NOT called here (the gate is
//! invoked by callers, mirroring the later wiring).

#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use mem_core::scope::{Scope, assert_safe_scope_hash, is_valid_scope_hash};

use crate::config::StoreConfig;
use crate::connection::ConnectionManager;
use crate::db::Db;
use crate::error::StoreError;
use crate::project_tags::ProjectTagSource;
use crate::shard_manager::{ShardInfo, ShardRegistry};
use crate::vector::{count_all_vectors, get_distinct_tags, get_project_path_counts};

/// Upstream `ShardInventoryStatus` (`missing-file` kebab spelling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShardInventoryStatus {
    Current,
    Linked,
    Orphaned,
    MissingFile,
    Empty,
    Ambiguous,
}

impl ShardInventoryStatus {
    fn as_key(self: &ShardInventoryStatus) -> &'static str {
        match self {
            ShardInventoryStatus::Current => "current",
            ShardInventoryStatus::Linked => "linked",
            ShardInventoryStatus::Orphaned => "orphaned",
            ShardInventoryStatus::MissingFile => "missingFile",
            ShardInventoryStatus::Empty => "empty",
            ShardInventoryStatus::Ambiguous => "ambiguous",
        }
    }
}

/// Upstream `ProjectShardDescriptor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectShardDescriptor {
    pub shard_id: Option<i64>,
    pub shard_index: u32,
    pub db_path: PathBuf,
    pub file_name: String,
    pub is_active: bool,
    pub vector_count: u64,
    pub memory_count: u64,
    pub file_exists: bool,
}

/// Upstream `ProjectShardGroup`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectShardGroup {
    pub scope: String,
    pub scope_hash: String,
    pub shard_indices: Vec<u32>,
    pub shards: Vec<ProjectShardDescriptor>,
    pub memory_count: u64,
    pub container_tag: Option<String>,
    pub project_path: Option<String>,
    pub project_path_candidates: Vec<ProjectPathCandidate>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
    pub path_exists: bool,
    pub status: ShardInventoryStatus,
    pub matches_current_project: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathCandidate {
    pub path: String,
    pub count: u64,
}

/// Upstream summary shape (camelCase JSON, counts + total).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardInventorySummary {
    pub total: usize,
    pub current: usize,
    pub linked: usize,
    pub orphaned: usize,
    pub missing_file: usize,
    pub empty: usize,
    pub ambiguous: usize,
}

/// Upstream `ListShardsResult` (without the `{success}` envelope).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListShardsResult {
    pub storage_path: PathBuf,
    pub current_project: CurrentProject,
    pub shards: Vec<ProjectShardGroup>,
    pub summary: ShardInventorySummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentProject {
    pub tag: String,
    pub scope_hash: String,
    pub project_path: Option<String>,
}

/// Upstream `ResolveMigrationSourceOptions`.
#[derive(Debug, Clone, Default)]
pub struct ResolveMigrationSourceOptions {
    pub from_path: Option<String>,
    pub from_hash: Option<String>,
}

/// Upstream `matchedBy` literal union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchedBy {
    FromHash,
    StoredProjectPath,
    LiveProjectIdentity,
}

/// Upstream `ResolvedMigrationSource`.
#[derive(Debug, Clone)]
pub struct ResolvedMigrationSource {
    pub scope_hash: String,
    pub matched_by: MatchedBy,
    pub group: ProjectShardGroup,
}

/// Upstream failure payload (typed error + matching candidates).
#[derive(Debug, Clone)]
pub struct ResolveSourceError {
    pub message: String,
    pub candidates: Vec<ProjectShardGroup>,
}

impl ResolveSourceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            candidates: Vec::new(),
        }
    }
}

/// Upstream `normalizeProjectPath` — absolute + forward slashes + no
/// trailing slash.
pub fn normalize_project_path(path: &str) -> String {
    let absolute = std::path::absolute(std::path::Path::new(path))
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| PathBuf::from(path));
    let normalized = lexical_clean(&absolute);
    let mut text = normalized.to_string_lossy().replace('\\', "/");
    if text.len() > 1 && text.ends_with('/') {
        while text.len() > 1 && text.ends_with('/') {
            text.pop();
        }
    }
    text
}

fn lexical_clean(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                output.pop();
            }
            std::path::Component::CurDir => {}
            other => output.push(other.as_os_str()),
        }
    }
    output
}

/// Upstream `SHARD_FILENAME_RE` — hand-rolled
/// `^(user|project)_([a-f0-9]{16})_shard_(\d+)\.db$`.
pub fn parse_project_shard_filename(file_name: &str) -> Option<(&'static str, String, u32)> {
    let stem = file_name.strip_suffix(".db")?;
    let parts: Vec<&str> = stem.split('_').collect();
    let [scope, hash, marker, index] = parts.as_slice() else {
        return None;
    };
    let scope = match *scope {
        "user" => "user",
        "project" => "project",
        _ => return None,
    };
    if hash.len() != 16
        || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        || *marker != "shard"
        || index.is_empty()
        || !index.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    // `\d+` accepts leading zeros — parse losslessly.
    let index: u64 = index.parse().ok()?;
    let index = u32::try_from(index).ok()?;
    Some((scope, (*hash).to_string(), index))
}

#[derive(Debug, Clone)]
struct ContentProbe {
    memory_count: u64,
    path_candidates: Vec<(String, u64, Option<String>)>,
    project_name: Option<String>,
    git_repo_url: Option<String>,
    container_tag: Option<String>,
}

impl ContentProbe {
    fn missing() -> Self {
        Self {
            memory_count: 0,
            path_candidates: Vec::new(),
            project_name: None,
            git_repo_url: None,
            container_tag: None,
        }
    }
}

/// Upstream `inspectShardContent` (missing file -> zeros; probe queries
/// are the step-7 vector functions).
async fn inspect_shard_content(connections: &ConnectionManager, db_path: &Path) -> ContentProbe {
    if !db_path.exists() {
        return ContentProbe::missing();
    }
    let Ok(db) = connections.get_connection(db_path).await else {
        return ContentProbe::missing();
    };
    let db = db.as_ref() as &dyn Db;
    let memory_count = count_all_vectors(db).await;

    // Collapse path+tag rows into path totals while keeping a representative
    // tag (upstream merge loop).
    let mut by_path: HashMap<String, (u64, Option<String>)> = HashMap::new();
    for count in get_project_path_counts(db).await {
        let key = normalize_project_path(&count.project_path);
        let entry = by_path.entry(key).or_insert((0, None));
        entry.0 += count.count;
        if entry.1.is_none() && count.container_tag.is_some() {
            entry.1 = count.container_tag;
        }
    }
    let mut path_candidates: Vec<(String, u64, Option<String>)> = by_path
        .into_iter()
        .map(|(path, (count, tag))| (path, count, tag))
        .collect();
    path_candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let tags = get_distinct_tags(db).await;
    let first = tags.first();
    let container_tag = path_candidates
        .first()
        .and_then(|(_, _, tag)| tag.clone())
        .or_else(|| first.and_then(|row| row.text("container_tag")));

    ContentProbe {
        memory_count,
        path_candidates,
        project_name: first.and_then(|row| row.text("project_name")),
        git_repo_url: first.and_then(|row| row.text("git_repo_url")),
        container_tag,
    }
}

/// Upstream `deriveStatus` — EXACT precedence.
fn derive_status(
    matches_current_project: bool,
    memory_count: u64,
    path_exists: bool,
    project_path: Option<&str>,
    path_candidates_len: usize,
    any_file_missing: bool,
    all_files_missing: bool,
) -> ShardInventoryStatus {
    if path_candidates_len > 1 {
        return ShardInventoryStatus::Ambiguous;
    }
    if all_files_missing {
        return ShardInventoryStatus::MissingFile;
    }
    if memory_count == 0 {
        return ShardInventoryStatus::Empty;
    }
    if matches_current_project {
        return ShardInventoryStatus::Current;
    }
    #[allow(clippy::unnecessary_map_or)]
    if project_path.is_some_and(|path| !path.is_empty()) && !path_exists {
        return ShardInventoryStatus::Orphaned;
    }
    if project_path.is_some_and(|path| !path.is_empty()) && path_exists {
        return ShardInventoryStatus::Linked;
    }
    if any_file_missing {
        return ShardInventoryStatus::MissingFile;
    }
    ShardInventoryStatus::Orphaned
}

#[derive(Debug, Default)]
struct GroupEntry {
    registered: Vec<ShardInfo>,
    file_paths: Vec<PathBuf>,
}

/// The inventory service.
pub struct ShardInventory {
    storage_path: PathBuf,
    config: StoreConfig,
    connections: Arc<ConnectionManager>,
    registry: Arc<dyn ShardRegistry>,
    tags: Arc<dyn ProjectTagSource>,
}

impl ShardInventory {
    pub fn new(
        storage_path: PathBuf,
        config: StoreConfig,
        connections: Arc<ConnectionManager>,
        registry: Arc<dyn ShardRegistry>,
        tags: Arc<dyn ProjectTagSource>,
    ) -> Self {
        Self {
            storage_path,
            config,
            connections,
            registry,
            tags,
        }
    }

    /// Upstream `listProjectShardDbPaths`.
    fn list_project_shard_db_paths(&self) -> Vec<PathBuf> {
        let dir = self.storage_path.join("projects");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_string();
            if file.ends_with(".db") && !file.contains(".bak") && !file.contains(".tmp") {
                paths.push(dir.join(entry.file_name()));
            }
        }
        paths.sort();
        paths
    }

    /// Upstream `listShards`.
    pub async fn list_shards(
        &self,
        current_directory: &Path,
    ) -> Result<ListShardsResult, StoreError> {
        let current = self.tags.project_tag_info(current_directory);
        let current_hash = current.tag.rsplit('_').next().unwrap_or("").to_string();
        assert_safe_scope_hash(&current_hash)
            .map(|_| ())
            .map_err(|error| StoreError::Core {
                message: error.to_string(),
            })?;

        let registered = self.registry.get_all_shards(Scope::Project, None).await?;

        let mut by_hash: HashMap<String, GroupEntry> = HashMap::new();
        for shard in registered {
            let entry = by_hash.entry(shard.scope_hash.clone()).or_default();
            entry.registered.push(shard.clone());
            entry.file_paths.push(shard.db_path.clone());
        }
        for path in self.list_project_shard_db_paths() {
            let Some((scope, scope_hash, _)) = parse_project_shard_filename(
                &path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ) else {
                continue;
            };
            if scope != "project" {
                continue;
            }
            let entry = by_hash.entry(scope_hash).or_default();
            entry.file_paths.push(path);
        }

        let mut groups: Vec<ProjectShardGroup> = Vec::new();
        for (scope_hash, entry) in by_hash {
            let mut shard_indices: BTreeSet<u32> = BTreeSet::new();
            for shard in &entry.registered {
                shard_indices.insert(shard.shard_index);
            }
            for path in &entry.file_paths {
                if let Some((_, _, index)) = parse_project_shard_filename(
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                ) {
                    shard_indices.insert(index);
                }
            }

            let scope_hash_for_fallback = scope_hash.clone();
            let mut descriptors: Vec<ProjectShardDescriptor> = Vec::new();
            let mut memory_count: u64 = 0;
            let mut path_count_map: HashMap<String, u64> = HashMap::new();
            let mut project_name: Option<String> = None;
            let mut git_repo_url: Option<String> = None;
            let mut container_tag: Option<String> = None;

            for shard_index in shard_indices.iter().copied() {
                let registered_shard = entry
                    .registered
                    .iter()
                    .find(|shard| shard.shard_index == shard_index);
                let db_path = match registered_shard {
                    Some(shard) => shard.db_path.clone(),
                    None => self
                        .storage_path
                        .join("projects")
                        .join(format!("project_{scope_hash}_shard_{shard_index}.db")),
                };
                let file_exists = db_path.exists();
                let content = inspect_shard_content(&self.connections, &db_path).await;
                memory_count += content.memory_count;

                for (path, count, tag) in &content.path_candidates {
                    *path_count_map.entry(path.clone()).or_insert(0) += count;
                    if container_tag.is_none() && tag.is_some() {
                        container_tag = tag.clone();
                    }
                }
                if project_name.is_none() {
                    project_name = content.project_name;
                }
                if git_repo_url.is_none() {
                    git_repo_url = content.git_repo_url;
                }
                if container_tag.is_none() {
                    container_tag = content.container_tag;
                }

                descriptors.push(ProjectShardDescriptor {
                    shard_id: registered_shard.map(|shard| shard.id),
                    shard_index,
                    db_path: db_path.clone(),
                    file_name: db_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    is_active: registered_shard
                        .map(|shard| shard.is_active)
                        .unwrap_or(shard_index == *shard_indices.last().expect("non-empty")),
                    vector_count: registered_shard
                        .map(|shard| shard.vector_count)
                        .unwrap_or(content.memory_count),
                    memory_count: content.memory_count,
                    file_exists,
                });
            }

            let mut project_path_candidates: Vec<ProjectPathCandidate> = path_count_map
                .into_iter()
                .map(|(path, count)| ProjectPathCandidate { path, count })
                .collect();
            project_path_candidates.sort_by(|a, b| b.count.cmp(&a.count).then(a.path.cmp(&b.path)));
            let project_path = project_path_candidates.first().map(|c| c.path.clone());
            let path_exists = project_path
                .as_deref()
                .map(std::path::Path::new)
                .map(std::path::Path::exists)
                .unwrap_or(false);
            let matches_current_project = scope_hash == current_hash;
            let any_file_missing = descriptors.iter().any(|shard| !shard.file_exists);
            let all_files_missing =
                !descriptors.is_empty() && descriptors.iter().all(|shard| !shard.file_exists);
            let status = derive_status(
                matches_current_project,
                memory_count,
                path_exists,
                project_path.as_deref(),
                project_path_candidates.len(),
                any_file_missing,
                all_files_missing,
            );

            groups.push(ProjectShardGroup {
                scope: "project".to_string(),
                scope_hash,
                shard_indices: descriptors.iter().map(|d| d.shard_index).collect(),
                shards: descriptors,
                memory_count,
                container_tag: Some(container_tag.unwrap_or_else(|| {
                    format!(
                        "{}_project_{scope_hash_for_fallback}",
                        self.config.container_tag_prefix
                    )
                })),
                project_path,
                project_path_candidates,
                project_name,
                git_repo_url,
                path_exists,
                status,
                matches_current_project,
            });
        }

        groups.sort_by(|a, b| {
            // current first, then memory count DESC, then hash ASC.
            b.matches_current_project
                .cmp(&a.matches_current_project)
                .then(b.memory_count.cmp(&a.memory_count))
                .then(a.scope_hash.cmp(&b.scope_hash))
        });

        let mut summary = ShardInventorySummary {
            total: groups.len(),
            ..Default::default()
        };
        for group in &groups {
            match group.status.as_key() {
                "current" => summary.current += 1,
                "linked" => summary.linked += 1,
                "orphaned" => summary.orphaned += 1,
                "missingFile" => summary.missing_file += 1,
                "empty" => summary.empty += 1,
                "ambiguous" => summary.ambiguous += 1,
                _ => unreachable!(),
            }
        }

        Ok(ListShardsResult {
            storage_path: self.storage_path.clone(),
            current_project: CurrentProject {
                tag: current.tag.clone(),
                scope_hash: current_hash,
                project_path: current
                    .project_path
                    .map(|p| p.to_string_lossy().to_string())
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some(current.display_name.clone())),
            },
            shards: groups,
            summary,
        })
    }

    /// Upstream `resolveMigrationSource`.
    pub async fn resolve_migration_source(
        &self,
        current_directory: &Path,
        options: &ResolveMigrationSourceOptions,
    ) -> Result<ResolvedMigrationSource, ResolveSourceError> {
        let from_hash = options
            .from_hash
            .as_deref()
            .filter(|h| !h.trim().is_empty());
        let from_path = options
            .from_path
            .as_deref()
            .filter(|p| !p.trim().is_empty());

        if let Some(from_hash) = from_hash {
            if !is_valid_scope_hash(from_hash) {
                return Err(ResolveSourceError::new(format!(
                    "Invalid fromHash: expected 16 lowercase hex characters, got \"{from_hash}\""
                )));
            }
            return match self.get_group_for_hash(from_hash).await {
                Some(group) if group.memory_count > 0 => Ok(ResolvedMigrationSource {
                    scope_hash: from_hash.to_string(),
                    matched_by: MatchedBy::FromHash,
                    group,
                }),
                Some(_) => Err(ResolveSourceError::new(format!(
                    "Shard hash {from_hash} exists but contains no memories."
                ))),
                None => Err(ResolveSourceError::new(format!(
                    "No project shards found for hash {from_hash}. \
                     Run memory list-shards to inspect available shards."
                ))),
            };
        }

        let Some(from_path) = from_path else {
            return Err(ResolveSourceError::new(
                "fromPath or fromHash is required for migrate",
            ));
        };

        let inventory = self
            .list_shards(current_directory)
            .await
            .map_err(|error| ResolveSourceError::new(error.message()))?;
        let normalized_from = normalize_project_path(from_path);
        let path_matches: Vec<ProjectShardGroup> = inventory
            .shards
            .iter()
            .filter(|group| member_of(&group.project_path_candidates, &normalized_from))
            .cloned()
            .collect();

        if path_matches.len() == 1 {
            let group = path_matches[0].clone();
            if group.memory_count == 0 {
                return Err(ResolveSourceError::new(format!(
                    "Matched project path {normalized_from} but the shard contains no memories."
                )));
            }
            return Ok(ResolvedMigrationSource {
                scope_hash: group.scope_hash.clone(),
                matched_by: MatchedBy::StoredProjectPath,
                group,
            });
        }
        if path_matches.len() > 1 {
            return Err(ResolveSourceError {
                message: format!(
                    "Ambiguous fromPath match for {normalized_from}. \
                     Use fromHash with memory list-shards to select one shard."
                ),
                candidates: path_matches,
            });
        }

        // If the old directory still exists, resolve its live identity.
        if std::path::Path::new(&normalized_from).exists() {
            let live = self.tags.project_tag_info(Path::new(&normalized_from));
            let live_hash = live.tag.rsplit('_').next().unwrap_or("").to_string();
            if is_valid_scope_hash(&live_hash)
                && let Some(group) = self.get_group_for_hash(&live_hash).await
                && group.memory_count > 0
            {
                return Ok(ResolvedMigrationSource {
                    scope_hash: live_hash,
                    matched_by: MatchedBy::LiveProjectIdentity,
                    group,
                });
            }
            // Fall through to the generic missing-source error.
        }

        Err(ResolveSourceError::new(format!(
            "No shard found for fromPath {normalized_from}. \
             If the old directory no longer exists, run memory list-shards \
             and migrate with fromHash."
        )))
    }

    async fn get_group_for_hash(&self, from_hash: &str) -> Option<ProjectShardGroup> {
        let Ok(inventory) = self.list_shards(&default_probe_directory()).await else {
            return None;
        };
        inventory
            .shards
            .into_iter()
            .find(|group| group.scope_hash == from_hash)
    }
}

fn member_of(candidates: &[ProjectPathCandidate], normalized_from: &str) -> bool {
    candidates.iter().any(|c| c.path == normalized_from)
}

/// The hash-branch looks up ONE hash without a `currentDirectory` in
/// upstream — it always receives one. Inventory listing needs the caller's
/// directory only for `matchesCurrentProject`, so the probe uses the
/// storage root as a neutral cwd stand-in (recorded divergence).
fn default_probe_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
