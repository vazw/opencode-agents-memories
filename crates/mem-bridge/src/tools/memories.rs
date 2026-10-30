//! Issue 70 wave 1 — the `add`, `list`, `search` and `forget` wire methods
//! over the shard store: one scope expansion, one read per shard, one total
//! order per merge.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mem_core::scope::{Scope, ScopeSelection};
use mem_embed::embedder::Embedder;
use mem_store::db::{BatchMode, Row};
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{MemoryInsert, insert_vector_statement};
use serde_json::{Value, json};

use crate::capture::format_tags_for_embedding;
use crate::composition::ComposedRuntime;
use crate::jsonrpc::Request;
use crate::scope_resolver::ProjectScopeResolver;

use super::{ToolError, optional_string, required_string, resolve_scope};

/// The four memory-tier methods' shared service.
pub struct MemoriesService {
    shards: Arc<ShardManager>,
    embedder: Arc<dyn Embedder>,
    resolver: Arc<dyn ProjectScopeResolver>,
    /// Upstream `args.limit || 20` for `list`.
    list_default_limit: usize,
    /// The resolved `config.max_memories` for `search`.
    search_default_limit: usize,
}

impl MemoriesService {
    /// Explicit constructor (the test seam: no `ComposedRuntime` needed).
    pub fn new(
        shards: Arc<ShardManager>,
        embedder: Arc<dyn Embedder>,
        resolver: Arc<dyn ProjectScopeResolver>,
        list_default_limit: usize,
        search_default_limit: usize,
    ) -> Self {
        Self {
            shards,
            embedder,
            resolver,
            list_default_limit,
            search_default_limit,
        }
    }

    /// Production: every dependency from the composed runtime or the
    /// resolved config, nothing re-derived.
    pub fn from_composed(composed: &ComposedRuntime, core: &mem_core::config::Config) -> Self {
        Self::new(
            composed.shards.clone(),
            composed.embedder.clone(),
            composed.scope_resolver.clone(),
            20,
            core.max_memories as usize,
        )
    }
}

/// Process-wide salt so two adds in the same millisecond cannot collide.
static ADD_SEQ: AtomicU32 = AtomicU32::new(0);

/// Upstream's `mem_<millis>_<9 hex>` id: uniqueness only, no new dependency.
fn new_memory_id(now_millis: i64, now_nanos: u128) -> String {
    let seq = ADD_SEQ.fetch_add(1, Ordering::Relaxed);
    let salt = (now_nanos as u32) ^ seq;
    format!("mem_{now_millis}_{salt:09x}")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

impl MemoriesService {
    /// `add`: strip → block → embed → one counted insert in one transaction.
    ///
    /// The order is the security contract: private text is never embedded,
    /// and a fully private message is refused before any write.
    pub async fn add(&self, request: &Request) -> Result<Value, ToolError> {
        let project_id = required_string(request, "project_id", "project_id")?;
        let content = required_string(request, "content", "content")?;
        let memory_type = optional_string(request, "type", "type")?;
        let tags_text = optional_string(request, "tags", "tags")?;
        let metadata = match request.param("metadata") {
            None | Some(Value::Null) => None,
            Some(Value::Object(map)) => Some(Value::Object(map.clone())),
            Some(_) => return Err(ToolError::invalid("metadata")),
        };

        // (1) required and non-blank AFTER stripping; (2) fully private is
        // refused before anything is embedded or written.
        let stripped = mem_ai::privacy::strip_private_content(content);
        if stripped.trim().is_empty() {
            return Err(ToolError::invalid("content"));
        }
        if mem_ai::privacy::is_fully_private(content) {
            return Err(ToolError::invalid("private"));
        }

        let tags: Vec<String> = tags_text
            .map(|text| {
                text.split(',')
                    .map(|tag| tag.trim().to_lowercase())
                    .filter(|tag| !tag.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // (3) the SANITIZED content and the shared tags formatter are what
        // reach the model.
        let vector = self
            .embedder
            .embed(&stripped)
            .await
            .map_err(|_error| ToolError::internal())?;
        let tags_vector = if tags.is_empty() {
            None
        } else {
            Some(
                self.embedder
                    .embed(&format_tags_for_embedding(&tags))
                    .await
                    .map_err(|_error| ToolError::internal())?,
            )
        };

        let scope = resolve_scope(self.resolver.as_ref(), project_id)?;
        let now = now_millis();
        let id = new_memory_id(now, now_nanos());
        let metadata_json = match metadata {
            Some(metadata) => {
                Some(serde_json::to_string(&metadata).map_err(|_error| ToolError::internal())?)
            }
            None => None,
        };
        let insert = MemoryInsert {
            id: id.clone(),
            content: stripped,
            vector,
            tags_vector,
            container_tag: scope.container_tag.clone(),
            tags: if tags.is_empty() {
                None
            } else {
                Some(tags.join(","))
            },
            memory_type: memory_type.map(str::to_string),
            created_at: now,
            updated_at: now,
            metadata: metadata_json,
            session_id: request
                .param("session_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            display_name: Some(scope.display_name.clone()),
            user_name: scope.user_name.clone(),
            user_email: scope.user_email.clone(),
            project_path: scope
                .project_path
                .as_ref()
                .map(|path| path.display().to_string()),
            project_name: scope.project_name.clone(),
            git_repo_url: scope.git_repo_url.clone(),
        };

        // (4) one `BatchMode::Write` of the insert and the statement-measured
        // absolute count; the registry adopts the shard's own value.
        let shards = self.shards.clone();
        let lock_hash = scope.scope_hash.clone();
        let scope_hash = scope.scope_hash.clone();
        self.shards
            .with_scope_write_lock(Scope::Project, &lock_hash, || {
                Box::pin(async move {
                    let info = match shards.get_write_shard(Scope::Project, &scope_hash).await {
                        Ok(info) => info,
                        // Our own consolidation's lock is tolerated; a foreign
                        // live lock refuses the write.
                        Err(mem_store::error::StoreError::MigrationLocked { pid, .. })
                            if pid == std::process::id() =>
                        {
                            shards
                                .get_write_shard_allowing_own_lock(Scope::Project, &scope_hash)
                                .await?
                        }
                        Err(error) => return Err(error),
                    };
                    let db = shards.open_shard(&info.db_path).await?;
                    let statement = insert_vector_statement(&insert)?;
                    db.batch(
                        vec![statement, mem_store::shard_facts::sync_count_statement()],
                        BatchMode::Write,
                    )
                    .await?;
                    shards.adopt_stored_count(db.as_ref(), info.id).await?;
                    Ok(())
                })
            })
            .await
            .map_err(|error| match error {
                mem_store::error::StoreError::MigrationLocked { .. } => {
                    ToolError::internal_data(json!({ "reason": "locked" }))
                }
                _ => ToolError::internal(),
            })?;

        Ok(json!({ "id": id }))
    }
}

impl MemoriesService {
    /// `list`: one scope expansion, one read per shard, one total order
    /// (`created_at DESC, id ASC`), one truncate.
    pub async fn list(&self, request: &Request) -> Result<Value, ToolError> {
        let project_id = required_string(request, "project_id", "project_id")?;
        let selection = super::parse_scope(request, false, ScopeSelection::Project)?;
        let limit = super::parse_limit(request, self.list_default_limit)?;
        let scope = resolve_scope(self.resolver.as_ref(), project_id)?;
        // The project tag for `project`; the wildcard's own tag selection for
        // `all-projects` (a user row never carries a project tag).
        let filter = match selection {
            ScopeSelection::Project => scope.container_tag.clone(),
            ScopeSelection::AllProjects => String::new(),
        };
        let refs = mem_core::scope::resolve_memory_scope(selection, &scope.container_tag)
            .map_err(|_error| ToolError::invalid("scope"))?;

        let mut rows: Vec<(i64, String, Value)> = Vec::new();
        for shard_ref in refs {
            let hash = shard_ref
                .hash
                .as_ref()
                .map(|hash| hash.as_str().to_string());
            let shards = self
                .shards
                .get_all_shards(shard_ref.scope, hash.as_deref())
                .await
                .map_err(|_error| ToolError::internal())?;
            for shard in shards {
                // A registered-but-missing file is an empty shard, never a
                // file to resurrect (opening would create it).
                if std::fs::metadata(&shard.db_path).is_err() {
                    continue;
                }
                let db = self
                    .shards
                    .open_shard(&shard.db_path)
                    .await
                    .map_err(|_error| ToolError::internal())?;
                let fetched =
                    mem_store::vector::list_memories_by_container_tag(db.as_ref(), &filter, limit)
                        .await
                        .map_err(|_error| ToolError::internal())?;
                for row in &fetched {
                    rows.push((
                        row.i64("created_at").unwrap_or(0),
                        row.text("id").unwrap_or_default(),
                        row_value(row),
                    ));
                }
            }
        }

        // ONE total order: created_at DESC, then id ASC (the SQL's
        // `is_pinned DESC` is per-shard and the upstream merge drops it).
        rows.sort_by(|(left_created, left_id, _), (right_created, right_id, _)| {
            right_created
                .cmp(left_created)
                .then_with(|| left_id.cmp(right_id))
        });
        rows.truncate(limit);
        let items: Vec<Value> = rows.into_iter().map(|(_, _, value)| value).collect();
        Ok(json!({ "count": items.len(), "items": items }))
    }
}

/// One stored row as the 16-key wire object (D-3).
pub(crate) fn row_value(row: &Row) -> Value {
    let metadata = row
        .text("metadata")
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or(Value::Null);
    json!({
        "container_tag": row.text("container_tag").unwrap_or_default(),
        "content": row.text("content").unwrap_or_default(),
        "created_at": row.i64("created_at").unwrap_or(0),
        "display_name": row.text("display_name"),
        "git_repo_url": row.text("git_repo_url"),
        "id": row.text("id").unwrap_or_default(),
        "is_pinned": row.i64("is_pinned").unwrap_or(0) != 0,
        "memory_type": row.text("type"),
        "metadata": metadata,
        "project_name": row.text("project_name"),
        "project_path": row.text("project_path"),
        "session_id": row.text("session_id"),
        "tags": tags_from_column(row.text("tags")),
        "updated_at": row.i64("updated_at"),
        "user_email": row.text("user_email"),
        "user_name": row.text("user_name"),
    })
}

/// The comma-separated `tags` column as the wire array (always an array).
pub(crate) fn tags_from_column(tags: Option<String>) -> Vec<String> {
    tags.map(|text| {
        text.split(',')
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

impl MemoriesService {
    /// `search`: embed the query once, then per shard scan → hydrate the full
    /// row in the same shard → merge under one total order → truncate → rank.
    ///
    /// `SearchResult` is not the wire item: it lacks `memory_type`,
    /// `session_id` and `updated_at`, so each hit is re-read with the frozen
    /// `SELECT *` (`get_memory_by_id`). A hit whose row vanished between the
    /// scan and the hydrate is dropped — never rendered from the stale scan.
    pub async fn search(&self, request: &Request) -> Result<Value, ToolError> {
        let project_id = required_string(request, "project_id", "project_id")?;
        let selection = super::parse_scope(request, true, ScopeSelection::Project)?;
        let query = required_string(request, "query", "query")?;
        let limit = super::parse_limit(request, self.search_default_limit)?;
        let scope = resolve_scope(self.resolver.as_ref(), project_id)?;
        let filter = match selection {
            ScopeSelection::Project => scope.container_tag.clone(),
            ScopeSelection::AllProjects => String::new(),
        };
        let refs = mem_core::scope::resolve_memory_scope(selection, &scope.container_tag)
            .map_err(|_error| ToolError::invalid("scope"))?;

        let query_vector = self
            .embedder
            .embed(query)
            .await
            .map_err(|_error| ToolError::internal())?;

        // (similarity, created_at, id, row)
        let mut hits: Vec<(f64, i64, String, Value)> = Vec::new();
        for shard_ref in refs {
            let hash = shard_ref
                .hash
                .as_ref()
                .map(|hash| hash.as_str().to_string());
            let shards = self
                .shards
                .get_all_shards(shard_ref.scope, hash.as_deref())
                .await
                .map_err(|_error| ToolError::internal())?;
            for shard in shards {
                if std::fs::metadata(&shard.db_path).is_err() {
                    continue;
                }
                let db = self
                    .shards
                    .open_shard(&shard.db_path)
                    .await
                    .map_err(|_error| ToolError::internal())?;
                let found = mem_store::vector::search_in_shard(
                    db.as_ref(),
                    &query_vector,
                    &filter,
                    limit,
                    Some(query),
                )
                .await
                .map_err(|_error| ToolError::internal())?;
                for hit in found {
                    let row = mem_store::vector::get_memory_by_id(db.as_ref(), &hit.id)
                        .await
                        .map_err(|_error| ToolError::internal())?;
                    let Some(row) = row else {
                        // Vanished between the scan and the hydrate: dropped.
                        continue;
                    };
                    hits.push((
                        hit.similarity,
                        row.i64("created_at").unwrap_or(0),
                        hit.id.clone(),
                        row_value(&row),
                    ));
                }
            }
        }

        // ONE total order: similarity DESC, created_at DESC, id ASC.
        hits.sort_by(
            |(left_similarity, left_created, left_id, _),
             (right_similarity, right_created, right_id, _)| {
                right_similarity
                    .partial_cmp(left_similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| right_created.cmp(left_created))
                    .then_with(|| left_id.cmp(right_id))
            },
        );
        hits.truncate(limit);
        // `rank` is assigned AFTER the truncate: 1..=n over the page.
        let items: Vec<Value> = hits
            .into_iter()
            .enumerate()
            .map(|(index, (similarity, _, _, value))| {
                let mut value = value;
                if let Value::Object(map) = &mut value {
                    map.insert("similarity".to_string(), Value::from(similarity));
                    map.insert("rank".to_string(), json!(index + 1));
                }
                value
            })
            .collect();
        Ok(json!({ "count": items.len(), "items": items }))
    }
}

impl MemoriesService {
    /// `forget`: `project_id` selects the tenant, then the 43.1 seam deletes
    /// every copy inside each concrete `(scope, hash)` tuple.
    ///
    /// `all-projects` expands only to hashes found in the registry — never a
    /// wildcard, and never `""` (the seam rejects that as a store fault).
    pub async fn forget(&self, request: &Request) -> Result<Value, ToolError> {
        let project_id = required_string(request, "project_id", "project_id")?;
        let memory_id = required_string(request, "memoryId", "memoryId")?;
        let selection = super::parse_scope(request, false, ScopeSelection::Project)?;
        let scope = resolve_scope(self.resolver.as_ref(), project_id)?;

        let tuples: Vec<(Scope, String)> = match selection {
            ScopeSelection::Project => vec![(Scope::Project, scope.scope_hash.clone())],
            ScopeSelection::AllProjects => {
                let mut tuples: Vec<(Scope, String)> = Vec::new();
                for scope_kind in [Scope::User, Scope::Project] {
                    for shard in self
                        .shards
                        .get_all_shards(scope_kind, None)
                        .await
                        .map_err(|_error| ToolError::internal())?
                    {
                        let known = tuples.iter().any(|(known_scope, known_hash)| {
                            *known_scope == scope_kind && known_hash == &shard.scope_hash
                        });
                        if !known {
                            tuples.push((scope_kind, shard.scope_hash.clone()));
                        }
                    }
                }
                tuples
            }
        };

        let mut deleted = 0usize;
        for (scope_kind, scope_hash) in tuples {
            deleted += self
                .shards
                .delete_memory_in_scope(scope_kind, &scope_hash, memory_id)
                .await
                .map_err(|_error| ToolError::internal())?;
        }
        Ok(json!({ "deleted": deleted }))
    }
}
