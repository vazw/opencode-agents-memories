//! The production port adapters over `mem-store` / `mem-profile` /
//! `mem-embed`.
//!
//! This is the other side of the seven trait seams: the handlers only ever
//! see [`super::ports`], and this module maps those seams onto the real store
//! kernel — [`ShardManager`] (scope write lock, write shard, vector count),
//! the [`mem_store::vector`] primitives, [`ProfileStore`],
//! [`UserPromptManager`] and [`EmbeddingService`].
//!
//! **What is wired here.** `MemoryPort`, `PromptPort`, `ProfilePort`,
//! `SessionPort` ([`EpisodicSessionPort`], W6.3), `SkillPort`
//! ([`ProductionSkills`], W6.4) and `EmbedPort`. The embedder
//! is injected (`Arc<dyn EmbedPort>`): production wraps the real
//! [`EmbeddingService`] ([`ProductionEmbed`]), while the tests inject a
//! fixed-vector fake — exactly what upstream's
//! `turso-api-scope.test.ts` does when it monkey-patches
//! `embeddingService.embedWithTimeout`.
//!
//! **What is not.** `MaintenancePort` stays `unavailable()`: the
//! handlers, the D-B bridge and the sync `TagMigrator` wiring already exist
//! (Step 11), but a production `MigrationService` needs the W5.x readiness
//! gate and a provider for `generate_tags`; `ApiServices::with_maintenance`
//! is the injection point. With no AI provider in this build,
//! `ProfilePort::ai_cleanup` answers the upstream business message
//! [`NO_PROVIDER_MESSAGE`] at HTTP 200, and `apply_cleanup` answers the
//! documented "no pending cleanup" message (the pending-cleanup session is
//! in-memory state that only a provider-backed cleanup can populate).
//!
//! W6.4 also adds [`ProductionFacts`], the `mem_portability::facts::FactStore`
//! adapter over the same shard manager. It is why `mem-web` depends on
//! `mem-portability` (and the dependency is one-way: `mem-portability`
//! never depends on `mem-web`).

use std::sync::Arc;

use async_trait::async_trait;
use mem_ai::privacy::{is_fully_private, strip_private_content};
use mem_core::scope::Scope;
use mem_embed::embedder::EmbedTask;
use mem_embed::service::EmbeddingService;
use mem_episodic::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_episodic::timeline::{SessionSummary, TimelineFilter};
use mem_portability::PortabilityError;
use mem_portability::bundle::ExportedMemory;
use mem_portability::facts::FactStore;
use mem_profile::ai_cleanup::{
    ProfileCleanupGateway, ai_cleanup_profile_from_indexed, filter_profile_for_cleanup,
    index_profile,
};
use mem_profile::error::ProfileError;
use mem_profile::profile_store::{ProfileChangelog, ProfileData, ProfileStore};
use mem_profile::prompt::{UserPrompt, UserPromptManager};
use mem_skills::SkillsManager;
use mem_skills::schema::{
    Skill as MemSkill, SkillInput as MemSkillInput, SkillPatch as MemSkillPatch,
};
use mem_store::db::{BatchMode, DbHandle, Row, Statement, Value, query_one};
use mem_store::error::StoreError;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert, SearchResult};
use serde_json::{Value as JsonValue, json};

use super::ports::{
    CatalogEntry, CleanupApplied, CleanupPreview, EmbedPort, MemoryPort, MemoryRow, MemoryUpdate,
    NewMemory, NewSkill, PortError, ProfileChangelog as ChangelogDto, ProfilePort, ProfileRecord,
    PromptPort, PromptRow, SearchHit, SessionPort, ShardRef, Skill, SkillPatch, TagRow,
};

/// The dependencies the production wiring needs (the W5.x composition root
/// supplies them; the signature is provisional until then).
pub struct ProductionDeps {
    /// The shard manager (the scope write lock, the write shard, the counts).
    pub shards: ShardManager,
    /// The user-prompt store.
    pub prompts: Arc<UserPromptManager>,
    /// The user-profile store.
    pub profile: Arc<ProfileStore>,
    /// The embedder — production wraps [`EmbeddingService`]; tests inject a
    /// fixed-vector fake (upstream's monkey-patched `embedWithTimeout`).
    pub embed: Arc<dyn EmbedPort>,
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// A store fault: the detail is logged by the handler, never echoed (D-9).
fn store_failure(error: StoreError) -> PortError {
    PortError::Store(error.message())
}

/// A profile-store fault. An analysis failure keeps its provider message
/// (upstream business surface); everything else is an internal fault.
fn profile_failure(error: ProfileError) -> PortError {
    match error {
        ProfileError::Analysis(message) => PortError::Invalid(message),
        other => PortError::Store(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Row → port DTO mapping
// ---------------------------------------------------------------------------

/// One `memories` row (the column names mirror the frozen DDL).
fn memory_row(row: &Row) -> MemoryRow {
    MemoryRow {
        id: row.text("id").unwrap_or_default(),
        content: row.text("content").unwrap_or_default(),
        container_tag: row.text("container_tag").unwrap_or_default(),
        tags: row.text("tags"),
        memory_type: row.text("type"),
        created_at: row.i64("created_at").unwrap_or(0),
        updated_at: row.i64("updated_at"),
        metadata: row.text("metadata"),
        session_id: row.text("session_id"),
        display_name: row.text("display_name"),
        user_name: row.text("user_name"),
        user_email: row.text("user_email"),
        project_path: row.text("project_path"),
        project_name: row.text("project_name"),
        git_repo_url: row.text("git_repo_url"),
        is_pinned: row.i64("is_pinned").unwrap_or(0) != 0,
        has_tags_vector: !row.is_null("tags_vector"),
    }
}

/// One `getDistinctTags` row.
fn tag_row(row: &Row) -> TagRow {
    TagRow {
        container_tag: row.text("container_tag").unwrap_or_default(),
        display_name: row.text("display_name"),
        user_name: row.text("user_name"),
        user_email: row.text("user_email"),
        project_path: row.text("project_path"),
        project_name: row.text("project_name"),
        git_repo_url: row.text("git_repo_url"),
    }
}

/// One `searchInShard` hit.
fn search_hit(hit: SearchResult) -> SearchHit {
    SearchHit {
        id: hit.id,
        content: hit.memory,
        similarity: hit.similarity,
        created_at: hit.created_at,
        tags: hit.tags,
        metadata: hit.metadata,
        container_tag: hit.container_tag,
        display_name: hit.display_name,
        user_name: hit.user_name,
        user_email: hit.user_email,
        project_path: hit.project_path,
        project_name: hit.project_name,
        git_repo_url: hit.git_repo_url,
        is_pinned: hit.is_pinned.unwrap_or(0) != 0,
    }
}

/// One `user_prompts` row.
fn prompt_row(prompt: UserPrompt) -> PromptRow {
    PromptRow {
        id: prompt.id,
        session_id: prompt.session_id,
        content: prompt.content,
        created_at: prompt.created_at,
        project_path: prompt.project_path,
        linked_memory_id: prompt.linked_memory_id,
    }
}

/// One `user_profiles` row (the port drops the store's `is_active` flag).
fn profile_record(profile: mem_profile::profile_store::Profile) -> ProfileRecord {
    ProfileRecord {
        id: profile.id,
        user_id: profile.user_id,
        display_name: profile.display_name,
        user_name: profile.user_name,
        user_email: profile.user_email,
        profile_data: profile.profile_data,
        version: profile.version,
        created_at: profile.created_at,
        last_analyzed_at: profile.last_analyzed_at,
        total_prompts_analyzed: profile.total_prompts_analyzed,
    }
}

/// One `user_profile_changelogs` row.
fn changelog_record(changelog: ProfileChangelog) -> ChangelogDto {
    ChangelogDto {
        id: changelog.id,
        profile_id: changelog.profile_id,
        version: changelog.version,
        change_type: changelog.change_type,
        change_summary: changelog.change_summary,
        profile_data_snapshot: changelog.profile_data_snapshot,
        created_at: changelog.created_at,
    }
}

/// The insert shape of the frozen 17-parameter `INSERT`.
fn memory_insert(memory: NewMemory) -> MemoryInsert {
    MemoryInsert {
        id: memory.id,
        content: memory.content,
        vector: memory.vector,
        tags_vector: memory.tags_vector,
        container_tag: memory.container_tag,
        tags: memory.tags,
        memory_type: memory.memory_type,
        created_at: memory.created_at,
        updated_at: memory.updated_at,
        metadata: memory.metadata,
        session_id: memory.session_id,
        display_name: memory.display_name,
        user_name: memory.user_name,
        user_email: memory.user_email,
        project_path: memory.project_path,
        project_name: memory.project_name,
        git_repo_url: memory.git_repo_url,
    }
}

/// The current epoch in milliseconds (the store's timestamps are `i64` ms).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// MemoryPort — ShardManager + the vector primitives
// ---------------------------------------------------------------------------

/// The production memory seam.
pub struct ProductionMemory {
    manager: ShardManager,
}

impl ProductionMemory {
    pub fn new(manager: ShardManager) -> Self {
        Self { manager }
    }

    /// Opens the shard database through the manager's connection manager.
    async fn open(&self, shard: &ShardRef) -> Result<DbHandle, PortError> {
        self.manager
            .open_shard(&shard.db_path)
            .await
            .map_err(store_failure)
    }
}

#[async_trait]
impl MemoryPort for ProductionMemory {
    async fn shards(
        &self,
        scope: Option<Scope>,
        scope_hash: Option<&str>,
    ) -> Result<Vec<ShardRef>, PortError> {
        // Upstream `getAllMemoryShards`: project shards first, then user
        // shards; `None` is the upstream `""` wildcard.
        let scopes: &[Scope] = match scope {
            Some(Scope::Project) => &[Scope::Project],
            Some(Scope::User) => &[Scope::User],
            None => &[Scope::Project, Scope::User],
        };
        let mut refs = Vec::new();
        for scope in scopes {
            let shards = self
                .manager
                .get_all_shards(*scope, scope_hash)
                .await
                .map_err(store_failure)?;
            refs.extend(shards.into_iter().map(|shard| ShardRef {
                scope: shard.scope,
                scope_hash: shard.scope_hash,
                shard_index: shard.shard_index,
                db_path: shard.db_path,
            }));
        }
        Ok(refs)
    }

    async fn all_memories(&self, shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError> {
        let db = self.open(shard).await?;
        let rows = db
            .query(mem_store::sql::GET_ALL_MEMORIES, Vec::new())
            .await
            .map_err(store_failure)?;
        Ok(rows.iter().map(memory_row).collect())
    }

    async fn list_memories_by_container_tag(
        &self,
        shard: &ShardRef,
        container_tag: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRow>, PortError> {
        let db = self.open(shard).await?;
        let rows = vector::list_memories_by_container_tag(db.as_ref(), container_tag, limit)
            .await
            .map_err(store_failure)?;
        Ok(rows.iter().map(memory_row).collect())
    }

    async fn memory_by_id(
        &self,
        shard: &ShardRef,
        id: &str,
    ) -> Result<Option<MemoryRow>, PortError> {
        let db = self.open(shard).await?;
        let row = vector::get_memory_by_id(db.as_ref(), id)
            .await
            .map_err(store_failure)?;
        Ok(row.as_ref().map(memory_row))
    }

    async fn distinct_tags(&self, shard: &ShardRef) -> Result<Vec<TagRow>, PortError> {
        let db = self.open(shard).await?;
        let rows = db
            .query(mem_store::sql::GET_DISTINCT_TAGS, Vec::new())
            .await
            .map_err(store_failure)?;
        Ok(rows.iter().map(tag_row).collect())
    }

    async fn insert_memory(
        &self,
        scope: Scope,
        scope_hash: &str,
        memory: NewMemory,
    ) -> Result<(), PortError> {
        let insert = memory_insert(memory);
        let manager = self.manager.clone();
        let lock_manager = manager.clone();
        let outcome: Result<(), StoreError> = manager
            .with_scope_write_lock(scope, scope_hash, move || {
                let manager = lock_manager;
                Box::pin(async move {
                    // The write runs under the per-scope lock, resolves the
                    // active (or rotated) shard and bumps the registry count.
                    let shard = manager.get_write_shard(scope, scope_hash).await?;
                    let db = manager.open_shard(&shard.db_path).await?;
                    vector::insert_vector(db.as_ref(), &insert).await?;
                    manager.increment_vector_count(shard.id).await
                })
            })
            .await;
        outcome.map_err(store_failure)
    }

    async fn update_memory(&self, shard: &ShardRef, update: MemoryUpdate) -> Result<(), PortError> {
        let insert = memory_insert(update.into());
        let db = self.open(shard).await?;
        // Upstream's `db.transaction("write")`: delete + insert atomically.
        // `BatchMode::Write` is the transactional batch (IMMEDIATE).
        let statements = vec![
            Statement::new(
                mem_store::sql::DELETE_MEMORY_BY_ID,
                vec![Value::Text(insert.id.clone())],
            ),
            vector::insert_vector_statement(&insert),
        ];
        db.batch(statements, BatchMode::Write)
            .await
            .map_err(store_failure)?;
        Ok(())
    }

    async fn delete_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError> {
        let db = self.open(shard).await?;
        vector::delete_memory_by_id(db.as_ref(), id)
            .await
            .map_err(store_failure)?;
        // Upstream `decrementVectorCount` — the registry row is located by the
        // shard path (the port's `ShardRef` carries no registry id).
        if let Some(info) = self
            .manager
            .get_shard_by_path(&shard.db_path)
            .await
            .map_err(store_failure)?
        {
            self.manager
                .decrement_vector_count(info.id)
                .await
                .map_err(store_failure)?;
        }
        Ok(())
    }

    async fn pin_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError> {
        let db = self.open(shard).await?;
        vector::pin_memory(db.as_ref(), id)
            .await
            .map_err(store_failure)
    }

    async fn unpin_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError> {
        let db = self.open(shard).await?;
        vector::unpin_memory(db.as_ref(), id)
            .await
            .map_err(store_failure)
    }

    async fn search_in_shard(
        &self,
        shard: &ShardRef,
        query_vector: &[f32],
        container_tag: &str,
        limit: usize,
        query_text: Option<&str>,
    ) -> Result<Vec<SearchHit>, PortError> {
        let db = self.open(shard).await?;
        let hits =
            vector::search_in_shard(db.as_ref(), query_vector, container_tag, limit, query_text)
                .await
                .map_err(store_failure)?;
        Ok(hits.into_iter().map(search_hit).collect())
    }
}

// ---------------------------------------------------------------------------
// PromptPort — UserPromptManager
// ---------------------------------------------------------------------------

/// The production user-prompt seam.
pub struct ProductionPrompts {
    manager: Arc<UserPromptManager>,
}

impl ProductionPrompts {
    pub fn new(manager: Arc<UserPromptManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl PromptPort for ProductionPrompts {
    async fn captured_prompts(
        &self,
        project_path: Option<&str>,
    ) -> Result<Vec<PromptRow>, PortError> {
        self.manager
            .get_captured_prompts(project_path)
            .await
            .map(|prompts| prompts.into_iter().map(prompt_row).collect())
            .map_err(profile_failure)
    }

    async fn search_prompts(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PromptRow>, PortError> {
        self.manager
            .search_prompts(query, project_path, limit as i64)
            .await
            .map(|prompts| prompts.into_iter().map(prompt_row).collect())
            .map_err(profile_failure)
    }

    async fn prompts_by_ids(&self, ids: &[String]) -> Result<Vec<PromptRow>, PortError> {
        self.manager
            .get_prompts_by_ids(ids)
            .await
            .map(|prompts| prompts.into_iter().map(prompt_row).collect())
            .map_err(profile_failure)
    }

    async fn prompt_by_id(&self, id: &str) -> Result<Option<PromptRow>, PortError> {
        self.manager
            .get_prompt_by_id(id)
            .await
            .map(|prompt| prompt.map(prompt_row))
            .map_err(profile_failure)
    }

    async fn delete_prompt(&self, id: &str) -> Result<(), PortError> {
        self.manager
            .delete_prompt(id)
            .await
            .map_err(profile_failure)
    }

    async fn count_unanalyzed(&self) -> Result<i64, PortError> {
        self.manager
            .count_unanalyzed_for_user_learning()
            .await
            .map_err(profile_failure)
    }
}

// ---------------------------------------------------------------------------
// ProfilePort — ProfileStore (+ the AI-cleanup kernel)
// ---------------------------------------------------------------------------

/// The production user-profile seam.
pub struct ProductionProfile {
    store: Arc<ProfileStore>,
}

impl ProductionProfile {
    pub fn new(store: Arc<ProfileStore>) -> Self {
        Self { store }
    }

    /// Parses the stored/request profile JSON into the store's typed shape.
    fn parse_data(data: &JsonValue) -> Result<ProfileData, PortError> {
        serde_json::from_value(data.clone()).map_err(|error| PortError::Store(error.to_string()))
    }
}

/// The JSON form of the cleanup diff (`{kept, merged, removed}`), matching the
/// upstream `changes` payload the UI reads.
fn cleanup_changes(diff: &mem_profile::ai_cleanup::CleanupDiff) -> JsonValue {
    json!({
        "kept": diff.kept,
        "merged": diff
            .merged
            .iter()
            .map(|group| json!({ "ids": group.ids, "result": group.result }))
            .collect::<Vec<_>>(),
        "removed": diff
            .removed
            .iter()
            .map(|item| json!({ "id": item.id, "reason": item.reason }))
            .collect::<Vec<_>>(),
    })
}

#[async_trait]
impl ProfilePort for ProductionProfile {
    async fn active_profile(&self, user_id: &str) -> Result<Option<ProfileRecord>, PortError> {
        self.store
            .get_active_profile(user_id)
            .await
            .map(|profile| profile.map(profile_record))
            .map_err(profile_failure)
    }

    async fn changelogs(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<ChangelogDto>, PortError> {
        self.store
            .get_profile_changelogs(profile_id, limit as i64)
            .await
            .map(|changelogs| changelogs.into_iter().map(changelog_record).collect())
            .map_err(profile_failure)
    }

    async fn changelog_by_id(&self, id: &str) -> Result<Option<ChangelogDto>, PortError> {
        // `ProfileStore` exposes the changelog list but no by-id accessor; the
        // lookup stays a bound statement (the id is never interpolated).
        let db = self.store.ready().await.map_err(profile_failure)?;
        let row = query_one(
            db.as_ref(),
            "SELECT * FROM user_profile_changelogs WHERE id = ?",
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(store_failure)?;
        Ok(row.as_ref().map(|row| ChangelogDto {
            id: row.text("id").unwrap_or_default(),
            profile_id: row.text("profile_id").unwrap_or_default(),
            version: row.i64("version").unwrap_or(0),
            change_type: row.text("change_type").unwrap_or_default(),
            change_summary: row.text("change_summary").unwrap_or_default(),
            profile_data_snapshot: row.text("profile_data_snapshot").unwrap_or_default(),
            created_at: row.i64("created_at").unwrap_or(0),
        }))
    }

    async fn update_profile(
        &self,
        profile_id: &str,
        profile_data: &JsonValue,
        change_summary: &str,
    ) -> Result<bool, PortError> {
        let data = Self::parse_data(profile_data)?;
        // `additional_prompts_analyzed = 0`: the REST update path does not
        // analyse prompts (upstream's `handleUpdateProfileItem`).
        self.store
            .update_profile(profile_id, &data, 0, change_summary)
            .await
            .map_err(profile_failure)
    }

    async fn decay(&self, profile_data: &JsonValue) -> Result<(JsonValue, bool), PortError> {
        let data = Self::parse_data(profile_data)?;
        let outcome = self.store.decay_in_memory(&data, now_millis());
        let decayed = serde_json::to_value(&outcome.data)
            .map_err(|error| PortError::Store(error.to_string()))?;
        Ok((decayed, outcome.has_changes))
    }

    async fn ai_cleanup(
        &self,
        _user_id: &str,
        _profile_version: i64,
        profile_data: &JsonValue,
        include_ids: Option<Vec<String>>,
    ) -> Result<CleanupPreview, PortError> {
        let data = Self::parse_data(profile_data)?;
        // The scoped path mirrors upstream's `filterProfileForCleanup`; the
        // unscoped one indexes the whole profile.
        let indexed = match include_ids.as_deref() {
            Some(ids) if !ids.is_empty() => filter_profile_for_cleanup(&data, ids),
            _ => index_profile(&data),
        };
        // No provider in this build: the gateway answers
        // `NO_PROVIDER_MESSAGE`, surfaced as the upstream business message.
        let gateway = ProfileCleanupGateway::new(None, None);
        let result = ai_cleanup_profile_from_indexed(&indexed, &gateway)
            .await
            .map_err(profile_failure)?;
        let cleaned = serde_json::to_value(&result.cleaned)
            .map_err(|error| PortError::Store(error.to_string()))?;
        Ok(CleanupPreview {
            old: profile_data.clone(),
            cleaned,
            changes: cleanup_changes(&result.diff),
        })
    }

    async fn apply_cleanup(
        &self,
        _user_id: &str,
        _body: &JsonValue,
    ) -> Result<CleanupApplied, PortError> {
        // The pending-cleanup session is in-memory state that only a
        // provider-backed `ai_cleanup` populates; with no provider it can
        // never exist, so upstream's documented business message is the
        // truthful answer.
        Err(PortError::Invalid(
            "No pending cleanup found. Run AI cleanup first.".to_string(),
        ))
    }
}

/// The production embedder seam over [`EmbeddingService`].
pub struct ProductionEmbed {
    service: Arc<EmbeddingService>,
}

impl ProductionEmbed {
    pub fn new(service: Arc<EmbeddingService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl EmbedPort for ProductionEmbed {
    async fn warmup(&self) -> Result<(), PortError> {
        self.service
            .warmup()
            .await
            .map_err(|error| PortError::Store(error.to_string()))
    }

    async fn embed(&self, text: &str, task: EmbedTask) -> Result<Vec<f32>, PortError> {
        self.service
            .embed(text, Some(task))
            .await
            .map_err(|error| PortError::Store(error.to_string()))
    }
}

// ---------------------------------------------------------------------------
// SessionPort — EpisodicStore (W6.3)
// ---------------------------------------------------------------------------

/// Maps a `mem-episodic` failure onto the port surface.
///
/// A `Validation` error carries the field name and a fixed reason sentence but
/// never the offending value, so it is safe to surface verbatim as a `400`
/// (nothing is echoed — D-9/D-10). `NotFound` is the port's "no such visible
/// row" answer. Everything else is an internal fault: the detail is logged by
/// the handler and the client sees the fixed `500`.
fn episodic_failure(error: EpisodicError) -> PortError {
    match error {
        EpisodicError::Validation { field, message } => {
            PortError::Invalid(format!("invalid {field}: {message}"))
        }
        EpisodicError::NotFound { .. } => PortError::NotFound,
        other => PortError::Store(other.to_string()),
    }
}

/// The production session-history seam over the W4.2 [`EpisodicStore`] query
/// layer (D-13).
///
/// The store owns every rule that matters here: the timeline is project-scoped
/// and newest-first, the soft delete is project-scoped (wrong-project, unknown
/// and already-deleted all return `false` — no existence leak) and it marks
/// children orphaned in the same `BatchMode::Write` batch (D-12). The adapter
/// only translates: `SessionSummary` → JSON and `false` → [`PortError::NotFound`].
pub struct EpisodicSessionPort {
    store: Arc<EpisodicStore>,
}

impl EpisodicSessionPort {
    /// Wraps one episodic store.
    pub fn new(store: Arc<EpisodicStore>) -> Self {
        Self { store }
    }

    /// Rejects an empty path parameter before the store sees it.
    ///
    /// `matchit` 0.8.4 matches an **empty** segment as a `{project_id}` /
    /// `{session_id}` value, so `/api/projects//sessions` reaches the handler
    /// with `""` instead of falling through to the router's 404. The blueprint
    /// pins that shape to `404` ("the request addresses no project"), not to a
    /// `400` validation answer, so an empty id is mapped to
    /// [`PortError::NotFound`]. Every *non-empty* malformed id (outer
    /// whitespace, NUL, oversize) still goes through the store's
    /// reject-never-coerce `check_id` and answers the `400`.
    fn empty_id(param: &str) -> Result<(), PortError> {
        if param.is_empty() {
            Err(PortError::NotFound)
        } else {
            Ok(())
        }
    }
}

/// Builds the production session port over `store` (the W5.x composition
/// root's `.with_sessions(...)` argument).
pub fn episodic_session_port(store: Arc<EpisodicStore>) -> Arc<dyn SessionPort> {
    Arc::new(EpisodicSessionPort::new(store))
}

/// One `SessionSummary` as the REST `data` element.
///
/// `SessionSummary` is already `camelCase` on the wire (`agentName`,
/// `parentSessionId`, `createdAt`, `orphaned`, …), so the mapping is the
/// serde projection — no hand-built object can drift from the tool result's
/// shape.
fn session_summary_json(summary: &SessionSummary) -> Result<JsonValue, PortError> {
    serde_json::to_value(summary).map_err(|error| PortError::Store(error.to_string()))
}

#[async_trait]
impl SessionPort for EpisodicSessionPort {
    async fn list_sessions(&self, project_id: &str) -> Result<Vec<JsonValue>, PortError> {
        Self::empty_id(project_id)?;
        let summaries = self
            .store
            .timeline(project_id, &TimelineFilter::default())
            .await
            .map_err(episodic_failure)?;
        summaries.iter().map(session_summary_json).collect()
    }

    async fn delete_session(&self, project_id: &str, session_id: &str) -> Result<(), PortError> {
        Self::empty_id(project_id)?;
        Self::empty_id(session_id)?;
        match self
            .store
            .soft_delete_session(project_id, session_id)
            .await
            .map_err(episodic_failure)?
        {
            true => Ok(()),
            // Unknown, wrong-project and already-deleted are one answer: the
            // handler's fixed `404 "Session not found"` (no existence leak).
            false => Err(PortError::NotFound),
        }
    }
}

// ---------------------------------------------------------------------------
// SkillPort — SkillsManager (W6.4)
// ---------------------------------------------------------------------------

/// Maps a `mem-skills` failure onto the port surface.
///
/// The manager's business rejections (an invalid id, a missing name or
/// content, a duplicate) are upstream messages and are surfaced verbatim
/// as a `400`; a missing row is the port's "no such visible row" answer;
/// everything else is an internal fault whose detail is logged, never
/// returned (D-9).
fn skills_failure(error: mem_skills::schema::SkillsError) -> PortError {
    use mem_skills::schema::SkillsError;
    match error {
        SkillsError::InvalidId
        | SkillsError::NameRequired
        | SkillsError::ContentRequired
        | SkillsError::AlreadyExists(_) => PortError::Invalid(error.to_string()),
        SkillsError::NotFound(_) => PortError::NotFound,
        other => PortError::Store(other.to_string()),
    }
}

/// The production skills seam over the W6.4 [`SkillsManager`].
///
/// The adapter only translates: the manager owns the validation, the
/// `SKILL.md` content file and the change notification.
pub struct ProductionSkills {
    manager: Arc<SkillsManager>,
}

impl ProductionSkills {
    /// Wraps one skills manager.
    pub fn new(manager: Arc<SkillsManager>) -> Self {
        Self { manager }
    }
}

/// Builds the production skills port over `manager` (the W5.x composition
/// root's `.with_skills(...)` argument).
pub fn production_skill_port(manager: Arc<SkillsManager>) -> Arc<dyn super::ports::SkillPort> {
    Arc::new(ProductionSkills::new(manager))
}

/// One `mem-skills` record as the port's DTO.
fn skill_row(skill: MemSkill) -> Skill {
    Skill {
        id: skill.id,
        name: skill.name,
        description: skill.description,
        content: skill.content,
        autoinvoke: skill.autoinvoke,
        enabled: skill.enabled,
        container_tag: skill.container_tag,
        created_at: skill.created_at,
        updated_at: skill.updated_at,
    }
}

#[async_trait]
impl super::ports::SkillPort for ProductionSkills {
    async fn list(
        &self,
        container_tag: Option<&str>,
        enabled_only: bool,
    ) -> Result<Vec<Skill>, PortError> {
        let skills = self
            .manager
            .list(container_tag, enabled_only)
            .await
            .map_err(skills_failure)?;
        Ok(skills.into_iter().map(skill_row).collect())
    }

    async fn get(&self, id: &str) -> Result<Option<Skill>, PortError> {
        let skill = self.manager.get(id).await.map_err(skills_failure)?;
        Ok(skill.map(skill_row))
    }

    async fn create(&self, skill: NewSkill) -> Result<Skill, PortError> {
        let created = self
            .manager
            .create(MemSkillInput {
                id: skill.id,
                name: skill.name,
                description: skill.description,
                content: skill.content,
                autoinvoke: skill.autoinvoke,
                enabled: skill.enabled,
                container_tag: skill.container_tag,
            })
            .await
            .map_err(skills_failure)?;
        Ok(skill_row(created))
    }

    async fn update(&self, id: &str, patch: SkillPatch) -> Result<Skill, PortError> {
        let updated = self
            .manager
            .update(
                id,
                MemSkillPatch {
                    name: patch.name,
                    description: patch.description,
                    content: patch.content,
                    autoinvoke: patch.autoinvoke,
                    enabled: patch.enabled,
                    container_tag: patch.container_tag,
                },
            )
            .await
            .map_err(skills_failure)?;
        Ok(skill_row(updated))
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        self.manager.delete(id).await.map_err(skills_failure)
    }

    async fn catalog_index(&self) -> Result<Vec<CatalogEntry>, PortError> {
        let entries = self.manager.catalog_index().await.map_err(skills_failure)?;
        Ok(entries
            .into_iter()
            .map(|entry| CatalogEntry {
                name: entry.name,
                version: entry.version,
                files: entry.files,
            })
            .collect())
    }

    async fn catalog_markdown(&self, id: &str) -> Result<Option<String>, PortError> {
        self.manager
            .catalog_markdown(id)
            .await
            .map_err(skills_failure)
    }
}

// ---------------------------------------------------------------------------
// FactStore — mem_portability's durable-fact seam (W6.4)
// ---------------------------------------------------------------------------

/// Maps a store fault onto the portability surface.
fn portability_store_failure(error: StoreError) -> PortabilityError {
    PortabilityError::Store(error.message().to_string())
}

/// The production durable-fact adapter over the shard manager.
///
/// It is the `mem_portability::facts::FactStore` implementation behind
/// `PortabilityService::new`'s `facts` argument. The scope of a container
/// tag is derived from the tag itself (the tag's own scope segment and
/// hash), so the adapter never re-hashes a path.
pub struct ProductionFacts {
    manager: Arc<ShardManager>,
}

impl ProductionFacts {
    /// Wraps one shard manager.
    pub fn new(manager: Arc<ShardManager>) -> Self {
        Self { manager }
    }

    /// The shards of one container tag's scope.
    async fn shards_for(&self, container_tag: &str) -> Result<Vec<ShardRef>, PortabilityError> {
        let scope_ref = mem_core::scope::extract_scope_from_container_tag(container_tag)
            .map_err(|error| PortabilityError::InvalidScope(error.to_string()))?;
        let scope = scope_ref.scope;
        let hash = scope_ref.hash.map(|hash| hash.as_str().to_string());
        let shards = self
            .manager
            .get_all_shards(scope, hash.as_deref())
            .await
            .map_err(portability_store_failure)?;
        Ok(shards
            .into_iter()
            .map(|shard| ShardRef {
                scope: shard.scope,
                scope_hash: shard.scope_hash,
                shard_index: shard.shard_index,
                db_path: shard.db_path,
            })
            .collect())
    }

    /// Opens one shard's database.
    async fn open(&self, shard: &ShardRef) -> Result<DbHandle, PortabilityError> {
        self.manager
            .open_shard(&shard.db_path)
            .await
            .map_err(portability_store_failure)
    }
}

/// The comma-separated `tags` column as a list.
fn split_tags(tags: Option<&str>) -> Option<Vec<String>> {
    let tags = tags?;
    let list: Vec<String> = tags
        .split(',')
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    if list.is_empty() { None } else { Some(list) }
}

/// The `metadata` JSON column as a map.
fn parse_metadata(metadata: Option<&str>) -> Option<std::collections::BTreeMap<String, JsonValue>> {
    let text = metadata?;
    serde_json::from_str(text).ok()
}

/// One `memories` row as the bundle's memory shape.
///
/// **Privacy by design (Mandate 6).** The `<private>` regions are STRIPPED
/// (upstream's `stripPrivateContent`) and a fully private row is DROPPED
/// (upstream's `isFullyPrivate`): a portable bundle is a file on disk, so
/// it must never carry a private region — and a row that is private
/// through and through is dropped instead of being exported as a
/// redacted husk.
fn exported_memory(row: &Row) -> Option<ExportedMemory> {
    let content = strip_private_content(&row.text("content").unwrap_or_default());
    if is_fully_private(&content) {
        return None;
    }
    Some(ExportedMemory {
        id: row.text("id").unwrap_or_default(),
        content,
        memory_type: row.text("type"),
        tags: split_tags(row.text("tags").as_deref()),
        created_at: row.i64("created_at").unwrap_or(0),
        updated_at: row.i64("updated_at"),
        is_pinned: Some(row.i64("is_pinned").unwrap_or(0) != 0),
        metadata: parse_metadata(row.text("metadata").as_deref()),
        display_name: row.text("display_name"),
        user_name: row.text("user_name"),
        user_email: row.text("user_email"),
        project_path: row.text("project_path"),
        project_name: row.text("project_name"),
        git_repo_url: row.text("git_repo_url"),
    })
}

#[async_trait]
impl FactStore for ProductionFacts {
    async fn list_memories(
        &self,
        container_tag: &str,
    ) -> Result<Vec<ExportedMemory>, PortabilityError> {
        let mut memories = Vec::new();
        for shard in self.shards_for(container_tag).await? {
            let db = self.open(&shard).await?;
            let rows = vector::list_memories_by_container_tag(db.as_ref(), container_tag, 100_000)
                .await
                .map_err(portability_store_failure)?;
            if rows.len() >= 100_000 {
                tracing::warn!(
                    container_tag,
                    limit = 100_000,
                    "portability export truncated the facts tier at the listing bound"
                );
            }
            memories.extend(rows.iter().filter_map(exported_memory));
        }
        Ok(memories)
    }

    async fn memory_exists(&self, id: &str, container_tag: &str) -> Result<bool, PortabilityError> {
        for shard in self.shards_for(container_tag).await? {
            let db = self.open(&shard).await?;
            let row = vector::get_memory_by_id(db.as_ref(), id)
                .await
                .map_err(portability_store_failure)?;
            if let Some(row) = row
                && row.text("container_tag").as_deref() == Some(container_tag)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn insert_memory(
        &self,
        target_tag: &str,
        memory: ExportedMemory,
        embedding: Vec<f32>,
        tags_embedding: Option<Vec<f32>>,
        metadata_json: String,
    ) -> Result<(), PortabilityError> {
        let scope_ref = mem_core::scope::extract_scope_from_container_tag(target_tag)
            .map_err(|error| PortabilityError::InvalidScope(error.to_string()))?;
        let scope = scope_ref.scope;
        let hash = scope_ref
            .hash
            .map(|hash| hash.as_str().to_string())
            .ok_or_else(|| {
                PortabilityError::InvalidScope(format!(
                    "Invalid containerTag hash: expected 16 lowercase hex characters in \"{target_tag}\""
                ))
            })?;
        let manager = self.manager.clone();
        let lock_manager = manager.clone();
        // Captured BEFORE the move into the insert: the frozen INSERT has no
        // pin column, so the flag is applied with a follow-up statement.
        let pinned = memory.is_pinned.unwrap_or(false);
        let memory_id = memory.id.clone();

        let insert = MemoryInsert {
            id: memory.id,
            content: memory.content,
            vector: embedding,
            tags_vector: tags_embedding,
            container_tag: target_tag.to_string(),
            tags: memory
                .tags
                .as_ref()
                .filter(|tags| !tags.is_empty())
                .map(|tags| tags.join(",")),
            memory_type: memory.memory_type,
            created_at: memory.created_at,
            updated_at: memory.updated_at.unwrap_or(memory.created_at),
            metadata: Some(metadata_json),
            session_id: None,
            display_name: memory.display_name,
            user_name: memory.user_name,
            user_email: memory.user_email,
            project_path: memory.project_path,
            project_name: memory.project_name,
            git_repo_url: memory.git_repo_url,
        };
        let lock_hash = hash.clone();
        let arg_hash = hash.clone();
        let outcome: Result<(), StoreError> = manager
            .with_scope_write_lock(scope, arg_hash.as_str(), move || {
                let manager = lock_manager;
                let hash = lock_hash.clone();
                Box::pin(async move {
                    let shard = manager.get_write_shard(scope, hash.as_str()).await?;
                    let db = manager.open_shard(&shard.db_path).await?;
                    vector::insert_vector(db.as_ref(), &insert).await?;
                    // The pin flag is NOT part of the INSERT (the frozen
                    // statement has no such column): upstream sets it with a
                    // follow-up UPDATE, and the port does the same through
                    // the store's own pin primitive.
                    if pinned {
                        vector::pin_memory(db.as_ref(), &memory_id).await?;
                    }
                    manager.increment_vector_count(shard.id).await
                })
            })
            .await;
        outcome.map_err(portability_store_failure)
    }

    async fn delete_memory(&self, id: &str, container_tag: &str) -> Result<(), PortabilityError> {
        for shard in self.shards_for(container_tag).await? {
            let db = self.open(&shard).await?;
            let row = vector::get_memory_by_id(db.as_ref(), id)
                .await
                .map_err(portability_store_failure)?;
            if let Some(row) = row
                && row.text("container_tag").as_deref() == Some(container_tag)
            {
                vector::delete_memory_by_id(db.as_ref(), id)
                    .await
                    .map_err(portability_store_failure)?;
                if let Some(info) = self
                    .manager
                    .get_shard_by_path(&shard.db_path)
                    .await
                    .map_err(portability_store_failure)?
                {
                    self.manager
                        .decrement_vector_count(info.id)
                        .await
                        .map_err(portability_store_failure)?;
                }
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mem_store::db::{Row, Value};

    use super::exported_memory;

    /// One `memories` row carrying the given content.
    fn row_with_content(content: &str) -> Row {
        let columns = [
            "id",
            "content",
            "type",
            "tags",
            "created_at",
            "updated_at",
            "is_pinned",
            "metadata",
            "display_name",
            "user_name",
            "user_email",
            "project_path",
            "project_name",
            "git_repo_url",
        ]
        .map(str::to_string);
        let values = vec![
            Value::Text("m-1".to_string()),
            Value::Text(content.to_string()),
            Value::Text("note".to_string()),
            Value::Null,
            Value::Integer(1),
            Value::Null,
            Value::Integer(0),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ];
        Row::new(Arc::new(columns.to_vec()), values)
    }

    /// Mandate 6 — the export path strips the `<private>` regions.
    #[test]
    fn the_export_strips_the_private_regions() {
        let memory = exported_memory(&row_with_content("visible <private>secret</private> text"))
            .expect("the row is exported");
        assert_eq!(memory.content, "visible [REDACTED] text");
        assert!(!memory.content.contains("secret"));
    }

    /// Mandate 6 — a fully private row is dropped, not exported.
    #[test]
    fn the_export_drops_a_fully_private_row() {
        assert!(
            exported_memory(&row_with_content("<private>secret</private>")).is_none(),
            "a fully private row must not be exported"
        );
        assert!(
            exported_memory(&row_with_content("   ")).is_none(),
            "an empty row must not be exported"
        );
    }

    /// A row without a private region is exported verbatim.
    #[test]
    fn the_export_keeps_an_ordinary_row() {
        let memory =
            exported_memory(&row_with_content("the plain content")).expect("the row is exported");
        assert_eq!(memory.content, "the plain content");
        assert_eq!(memory.id, "m-1");
        assert_eq!(memory.memory_type.as_deref(), Some("note"));
    }
}
