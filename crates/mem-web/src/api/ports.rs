//! The eight port seams behind the REST surface.
//!
//! Upstream `api-handlers.ts` reaches the process-wide singletons
//! (`tursoShardManager`, `tursoConnectionManager`, `tursoVectorSearch`,
//! `embeddingService`, `userPromptManager`, `userProfileManager`,
//! `skillsManager`, the cleanup/deduplication/migration services). The port
//! keeps the same boundaries but makes them injectable: every handler talks to
//! one of the eight object-safe traits below, never to a store type. The
//! production adapters live in [`super::store_adapter`]; the tests inject
//! in-memory fakes.
//!
//! `ApiServices::unavailable()` is the default wiring: every port answers
//! [`PortError::Unavailable`], so the router is testable (and the W6.4 stubs
//! answer their documented 501) before the composition root supplies the
//! production adapters.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use mem_core::scope::Scope;
use mem_embed::embedder::EmbedTask;
use mem_store::error::ShardModelMismatch;
use serde_json::Value;

use super::dto::{MilestoneItemDto, ProjectRowDto};

/// A resolved shard (upstream `ShardInfo`, minus the registry bookkeeping the
/// handlers do not read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardRef {
    pub scope: Scope,
    pub scope_hash: String,
    pub shard_index: u32,
    pub db_path: PathBuf,
    /// The registry row id (upstream `ShardInfo.id`); `None` preserves
    /// upstream's `shardId: number | null` wire shape for adapters without
    /// registry ids.
    pub shard_id: Option<i64>,
    /// The registered vector count (upstream `ShardInfo.vectorCount`).
    pub vector_count: u64,
    /// Whether the shard is the scope's active write target (upstream
    /// `ShardInfo.isActive`).
    pub is_active: bool,
}

/// One `memories` row in the shape the handlers consume.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRow {
    pub id: String,
    pub content: String,
    pub container_tag: String,
    pub tags: Option<String>,
    pub memory_type: Option<String>,
    pub created_at: i64,
    pub updated_at: Option<i64>,
    pub metadata: Option<String>,
    pub session_id: Option<String>,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
    pub is_pinned: bool,
    pub has_tags_vector: bool,
}

/// One `getDistinctTags` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    pub container_tag: String,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
}

/// One `user_prompts` row (upstream `UserPrompt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRow {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub created_at: i64,
    pub project_path: Option<String>,
    pub linked_memory_id: Option<String>,
}

/// The write shape of `POST /api/memories` (upstream `handleAddMemory`'s
/// record).
#[derive(Debug, Clone, PartialEq)]
pub struct NewMemory {
    pub id: String,
    pub content: String,
    pub vector: Vec<f32>,
    pub tags_vector: Option<Vec<f32>>,
    pub container_tag: String,
    pub tags: Option<String>,
    pub memory_type: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub metadata: Option<String>,
    pub session_id: Option<String>,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
}

/// The replacement shape of `PUT /api/memories/{id}` (upstream's
/// delete+insert transaction keeps the original row's identity columns).
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryUpdate {
    pub id: String,
    pub content: String,
    pub vector: Vec<f32>,
    pub tags_vector: Option<Vec<f32>>,
    pub container_tag: String,
    pub tags: Option<String>,
    pub memory_type: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub metadata: Option<String>,
    pub session_id: Option<String>,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
}

impl From<MemoryUpdate> for NewMemory {
    fn from(value: MemoryUpdate) -> Self {
        Self {
            id: value.id,
            content: value.content,
            vector: value.vector,
            tags_vector: value.tags_vector,
            container_tag: value.container_tag,
            tags: value.tags,
            memory_type: value.memory_type,
            created_at: value.created_at,
            updated_at: value.updated_at,
            metadata: value.metadata,
            session_id: value.session_id,
            display_name: value.display_name,
            user_name: value.user_name,
            user_email: value.user_email,
            project_path: value.project_path,
            project_name: value.project_name,
            git_repo_url: value.git_repo_url,
        }
    }
}

/// One `searchInShard` hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub content: String,
    pub similarity: f64,
    pub created_at: i64,
    pub tags: Vec<String>,
    pub metadata: Option<Value>,
    pub container_tag: String,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
    pub is_pinned: bool,
}

/// One `user_profiles` row (the `profile_data` column stays raw JSON text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRecord {
    pub id: String,
    pub user_id: String,
    pub display_name: String,
    pub user_name: String,
    pub user_email: String,
    pub profile_data: String,
    pub version: i64,
    pub created_at: i64,
    pub last_analyzed_at: i64,
    pub total_prompts_analyzed: i64,
}

/// One `user_profile_changelogs` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChangelog {
    pub id: String,
    pub profile_id: String,
    pub version: i64,
    pub change_type: String,
    pub change_summary: String,
    pub profile_data_snapshot: String,
    pub created_at: i64,
}

/// What one profile erase actually removed (issue 48 — `ExecResult.changes`
/// sums from the two-table delete, never estimates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProfileErasure {
    pub profiles_deleted: u64,
    pub changelogs_deleted: u64,
}

/// The preview returned by `POST /api/user-profile/ai-cleanup`.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupPreview {
    pub old: Value,
    pub cleaned: Value,
    pub changes: Value,
}

/// The answer of `POST /api/user-profile/ai-cleanup/apply`.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupApplied {
    pub message: String,
    pub version: i64,
}

/// One skill (upstream `skillToApi`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub autoinvoke: bool,
    pub enabled: bool,
    pub container_tag: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `POST /api/skills` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSkill {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub content: String,
    pub autoinvoke: Option<bool>,
    pub enabled: Option<bool>,
    pub container_tag: Option<String>,
}

/// `PUT /api/skills/{id}` patch (`None` = the field was not sent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    pub autoinvoke: Option<bool>,
    pub enabled: Option<bool>,
    pub container_tag: Option<String>,
}

/// One entry of the OpenCode catalog index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
}

/// `GET /api/migration/detect` answer.
#[derive(Debug, Clone, PartialEq)]
pub struct MigrationReport {
    pub needs_migration: bool,
    pub config_dimensions: u32,
    pub config_model: String,
    pub shard_mismatches: Vec<Value>,
}

/// `POST /api/migration/run` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRunReport {
    pub success: bool,
    pub strategy: String,
    pub deleted_shards: u32,
    pub re_embedded_memories: u32,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// `GET /api/migration/tags/detect` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagMigrationReport {
    pub needs_migration: bool,
    pub count: u64,
}

/// `GET /api/migration/tags/progress` answer.
pub type TagMigrationProgressView = mem_core::tags::migration::MigrationProgress;

/// `POST /api/migration/tags/run-batch` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagMigrationBatchReport {
    pub processed: usize,
    pub total: usize,
    pub has_more: bool,
    pub errors: usize,
}

/// `POST /api/cleanup` answer (the seam is pre-wired; no service exists yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub deleted_count: u64,
    pub user_count: u64,
    pub project_count: u64,
}

/// `POST /api/deduplicate` answer (upstream `IDeduplicationResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct DedupReport {
    pub exact_duplicates_deleted: u64,
    pub near_duplicate_groups: Vec<NearDuplicateGroup>,
    /// What the bounded near-duplicate scan did — always present, so a
    /// consumer that ignores it still sees truthful legacy fields and one that
    /// reads it can tell "no near duplicates" from "did not look".
    pub near_duplicate_scan: NearDuplicateScanReport,
}

/// One pass's near-duplicate scan accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NearDuplicateScanReport {
    /// Candidate comparisons actually performed across the whole pass.
    pub pairs_scanned: u64,
    /// The pass's global candidate-comparison budget.
    pub pair_budget: u64,
    /// Whether any scan work was left undone — the budget was exhausted, or a
    /// shard's rows could not be read (`unreadable_shards > 0`).
    pub truncated: bool,
    /// Shards walked whose rows could not be read; a count, never a detail
    /// (D-9). Non-zero forces `truncated`. Both halves of such a shard are
    /// unexamined: its near scan contributes nothing, and its exact-delete
    /// half never runs, so `exact_duplicates_deleted` under-counts there too —
    /// a read that was never made, not a budget truncation.
    pub unreadable_shards: u64,
}

/// One reported near-duplicate group (upstream `DuplicateGroup`).
#[derive(Debug, Clone, PartialEq)]
pub struct NearDuplicateGroup {
    pub representative: DedupMemoryRef,
    pub duplicates: Vec<DedupDuplicate>,
}

/// The survivor of an exact group (upstream `representative`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupMemoryRef {
    pub id: String,
    pub content: String,
    pub container_tag: String,
    pub created_at: i64,
}

/// One reported near-duplicate (upstream `duplicates[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct DedupDuplicate {
    pub id: String,
    pub content: String,
    pub similarity: f64,
}

/// The single port error. `Invalid` carries an upstream business message
/// (answered at HTTP 200 with `success:false` by the handlers); `Store` is an
/// internal fault and is never echoed (D-9).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PortError {
    /// The endpoint's service is not wired in this build.
    #[error("the endpoint is not available in this build")]
    Unavailable,
    /// The addressed row does not exist.
    #[error("not found")]
    NotFound,
    /// An upstream business-rule rejection with its verbatim message.
    #[error("{0}")]
    Invalid(String),
    /// A store/port failure; the detail is logged, never returned.
    #[error("{0}")]
    Store(String),
    /// A shard's stored embedding identity disagrees with the configured one
    /// (W3): answered 409 with the typed Display (ids, models, the remedy
    /// command — never content and never a path).
    #[error("{0}")]
    ModelMismatch(Box<ShardModelMismatch>),
    /// Another consolidation or purge holds the episodic write gate for this
    /// storage root (D10): answered `409`. Carries no path and no content.
    #[error("another consolidation or purge is running on this storage root")]
    Busy,
    /// Issue 130 — the read lane refused a REGISTERED shard whose DB file is
    /// missing (stat-before-open). Answered **503** with a fixed body: the
    /// shard file is not recreated by the read and the fault is neither a
    /// "not wired" (501) nor a "broken here" (500). The payload rides the log
    /// channel only — the file name is never echoed to the wire (the same
    /// discipline the `Store` token rule keeps).
    ///
    /// TOCTOU is ACCEPTED and DOCUMENTED, not closed: nothing removes the
    /// tiny stat→open window; a file deleted inside it is still recreated by
    /// turso create-on-open (the pre-fix behavior, never data loss — the
    /// create is create-if-missing over an absent file). `open_shard` stays
    /// the last word; the refusal is advisory narrowing at the read lane.
    /// Inverted world: a file observed recreated after a guarded read
    /// answered 503 reopens the decision as a defect (issue 130 / the
    /// 2026-11-08 shard-resurrection decision record).
    #[error("shard storage is temporarily unavailable — the shard file is missing")]
    ShardFileMissing {
        /// The shard file's own name component, for diagnostics. Log-only.
        file_name: String,
    },
}

/// The memory store seam.
#[async_trait]
pub trait MemoryPort: Send + Sync {
    /// The shards to walk. `scope: None` means project shards then user
    /// shards (upstream `getAllMemoryShards`); `scope_hash: None` is the
    /// upstream `""` wildcard.
    async fn shards(
        &self,
        scope: Option<Scope>,
        scope_hash: Option<&str>,
    ) -> Result<Vec<ShardRef>, PortError>;

    /// Verifies every shard's stored embedding identity against the
    /// configured one (W3). REQUIRED, no default: a fail-open seam on a
    /// security-relevant read is not acceptable.
    ///
    /// Collects every mismatch of the list before refusing (all shards of a
    /// list share one scope by construction); a shard whose facts cannot be
    /// read is logged and skipped (today's per-shard tolerance — it is not a
    /// model mismatch).
    async fn verify_shards(&self, shards: &[ShardRef]) -> Result<(), PortError>;

    async fn all_memories(&self, shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError>;

    async fn list_memories_by_container_tag(
        &self,
        shard: &ShardRef,
        container_tag: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRow>, PortError>;

    /// One row lookup in one shard. Production adapters stat before open on
    /// the READ lanes — issue 130: `all_memories`,
    /// `list_memories_by_container_tag`, `distinct_tags`, `search_in_shard`
    /// refuse through the dedicated `ShardFileMissing` arm (a missing shard
    /// file is a 503, never a silent recreation), and this method stats
    /// directly so `find_memory`'s anchor scan (delete, PUT, pin/unpin, the
    /// prompt cascade) fails closed too: a missing shard file is a `Store`
    /// failure here, never a silent recreation (`open_shard` creates a
    /// missing database). Write lanes keep create-on-open through the
    /// explicit registration path.
    async fn memory_by_id(
        &self,
        shard: &ShardRef,
        id: &str,
    ) -> Result<Option<MemoryRow>, PortError>;

    async fn distinct_tags(&self, shard: &ShardRef) -> Result<Vec<TagRow>, PortError>;

    /// Insert under the scope write lock (production); increments the shard
    /// vector count.
    async fn insert_memory(
        &self,
        scope: Scope,
        scope_hash: &str,
        memory: NewMemory,
    ) -> Result<(), PortError>;

    /// Delete+insert in one transaction.
    async fn update_memory(&self, shard: &ShardRef, update: MemoryUpdate) -> Result<(), PortError>;

    /// Deletes every copy of `id` in the shard's `(scope, scope_hash)` across
    /// all registered shards (active and retired): one `delete_memories` batch
    /// plus absolute count adoption per touched shard, with
    /// `ensure_shard_schema` (migrate-on-touch) before the counted batch on a
    /// pre-v3 shard. An unreadable shard is a `Store` failure; deletes are
    /// never model-refused.
    async fn delete_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError>;

    async fn pin_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError>;

    async fn unpin_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError>;

    async fn search_in_shard(
        &self,
        shard: &ShardRef,
        query_vector: &[f32],
        container_tag: &str,
        limit: usize,
        query_text: Option<&str>,
    ) -> Result<Vec<SearchHit>, PortError>;
}

/// The user-prompt store seam.
#[async_trait]
pub trait PromptPort: Send + Sync {
    async fn captured_prompts(
        &self,
        project_path: Option<&str>,
    ) -> Result<Vec<PromptRow>, PortError>;

    async fn search_prompts(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PromptRow>, PortError>;

    async fn prompts_by_ids(&self, ids: &[String]) -> Result<Vec<PromptRow>, PortError>;

    async fn prompt_by_id(&self, id: &str) -> Result<Option<PromptRow>, PortError>;

    /// Deletes one prompt (and its derived profiles) and returns what the
    /// erasure actually did — the adapter no longer discards `PromptDeletion`
    /// (issue 43 W6c).
    async fn delete_prompt(&self, id: &str) -> Result<PromptErasure, PortError>;

    /// The tier's sweep stamp (`sweep_state.last_swept_unix_ms`), or `None`
    /// when the tier was never swept. A stamp answers `swept`, its absence
    /// `notYetSwept`, a failed read `sweepFailed` (never `notYetSwept`: a failed
    /// read is not evidence that no sweep ran). The delete answer never reads
    /// this — a delete's residue postdates any stamp (ADR-0019 :165-167).
    ///
    /// Default `Ok(None)`: a port that has no sweep table (a fake, or a build
    /// without the migration) answers honestly rather than failing the route.
    async fn sweep_state(&self, _tier: &str) -> Result<Option<i64>, PortError> {
        Ok(None)
    }
}

/// What one prompt delete erased, for the disclosure DTO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PromptErasure {
    /// Whether the prompt row existed (`false` ⇒ the 404/soft-miss path).
    pub existed: bool,
    /// Derived profile rows erased.
    pub profiles_deleted: u64,
    /// Their changelog snapshots erased.
    pub changelogs_deleted: u64,
}

/// The user-profile store seam (including the AI-cleanup pending session).
#[async_trait]
pub trait ProfilePort: Send + Sync {
    async fn active_profile(&self, user_id: &str) -> Result<Option<ProfileRecord>, PortError>;

    /// Every stored active profile, in the store's own order (the handler owns
    /// the documented ordering and the bound).
    ///
    /// The roster route (issue 115) needs the whole identity set to offer a
    /// picker: answering it from `active_profile` would require guessing
    /// identities. Only identity-level fields are read; `profile_data` never
    /// leaves the server through this seam.
    async fn active_profiles(&self) -> Result<Vec<ProfileRecord>, PortError>;

    /// Erase one profile and its changelog snapshots (issue 48, B2′). The
    /// handler resolves the profile first; a client never names a
    /// `profile_id` directly.
    async fn delete_profile(&self, profile_id: &str) -> Result<ProfileErasure, PortError>;

    async fn changelogs(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<ProfileChangelog>, PortError>;

    async fn changelog_by_id(&self, id: &str) -> Result<Option<ProfileChangelog>, PortError>;

    async fn update_profile(
        &self,
        profile_id: &str,
        profile_data: &Value,
        change_summary: &str,
    ) -> Result<bool, PortError>;

    /// Upstream `decayInMemory`; returns the decayed data and whether it
    /// changed.
    async fn decay(&self, profile_data: &Value) -> Result<(Value, bool), PortError>;

    async fn ai_cleanup(
        &self,
        user_id: &str,
        profile_version: i64,
        profile_data: &Value,
        include_ids: Option<Vec<String>>,
    ) -> Result<CleanupPreview, PortError>;

    async fn apply_cleanup(&self, user_id: &str, body: &Value)
    -> Result<CleanupApplied, PortError>;
}

/// The skills seam (production is `unavailable()` until W6.4).
#[async_trait]
pub trait SkillPort: Send + Sync {
    async fn list(
        &self,
        container_tag: Option<&str>,
        enabled_only: bool,
    ) -> Result<Vec<Skill>, PortError>;

    async fn get(&self, id: &str) -> Result<Option<Skill>, PortError>;

    async fn create(&self, skill: NewSkill) -> Result<Skill, PortError>;

    async fn update(&self, id: &str, patch: SkillPatch) -> Result<Skill, PortError>;

    async fn delete(&self, id: &str) -> Result<bool, PortError>;

    async fn catalog_index(&self) -> Result<Vec<CatalogEntry>, PortError>;

    async fn catalog_markdown(&self, id: &str) -> Result<Option<String>, PortError>;
}

/// The embedding seam (the test injects a fixed-vector fake, exactly like
/// upstream's monkey-patched `embedWithTimeout`).
#[async_trait]
pub trait EmbedPort: Send + Sync {
    async fn warmup(&self) -> Result<(), PortError>;

    async fn embed(&self, text: &str, task: EmbedTask) -> Result<Vec<f32>, PortError>;
}

/// The maintenance seam (migrations + the cleanup/dedup stubs).
#[async_trait]
pub trait MaintenancePort: Send + Sync {
    async fn detect_migration(&self) -> Result<MigrationReport, PortError>;

    async fn run_migration(&self, strategy: &str) -> Result<MigrationRunReport, PortError>;

    async fn detect_tag_migration(&self) -> Result<TagMigrationReport, PortError>;

    async fn tag_migration_progress(&self) -> Result<TagMigrationProgressView, PortError>;

    /// All project-shard memory records (upstream's `SELECT * FROM memories`).
    async fn tag_migration_records(
        &self,
    ) -> Result<Vec<mem_core::tags::migration::MemoryRecord>, PortError>;

    /// Upstream `provider.executeToolCall` seam: structured tag generation.
    async fn generate_tags(&self, memory_id: &str, content: &str) -> Result<Vec<String>, String>;

    /// Run one batch window through the sync `TagMigrator` inside
    /// `spawn_blocking`, with the tag-generation calls pre-computed by the
    /// handler (D-22/D-23).
    async fn run_tag_migration_batch(
        &self,
        batch_size: usize,
        records: Vec<mem_core::tags::migration::MemoryRecord>,
        tags: HashMap<String, Result<Vec<String>, String>>,
    ) -> Result<mem_core::tags::migration::BatchOutcome, PortError>;

    async fn cleanup(&self) -> Result<CleanupReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn deduplicate(&self) -> Result<DedupReport, PortError> {
        Err(PortError::Unavailable)
    }
}

/// The W6.3 session-history seam.
#[async_trait]
pub trait SessionPort: Send + Sync {
    async fn list_sessions(&self, project_id: &str) -> Result<Vec<Value>, PortError>;

    async fn delete_session(&self, project_id: &str, session_id: &str) -> Result<(), PortError>;

    /// The project roster (issue 116, D-6): every project with live sessions or
    /// milestones, newest activity first, bounded by `limit`.
    ///
    /// It lives on this port because the session side of the union is the
    /// episodic store's, and splitting it into a ninth port would duplicate the
    /// same handle.
    async fn projects(&self, limit: usize) -> Result<Vec<ProjectRowDto>, PortError>;
}

/// The W6.6 milestone-projection seam.
///
/// The production adapter is `EpisodicMilestonePort`: `EpisodicStore`'s
/// milestone rows plus the `facts.db` promoted-facts join. The handler sees
/// only the DTO, never a store type.
#[async_trait]
pub trait MilestonePort: Send + Sync {
    /// The project's milestones, newest first, bounded by `limit`; each
    /// carries the durable facts it promoted.
    async fn list_milestones(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<MilestoneItemDto>, PortError>;
}

/// The eight ports bundled into the shared state.
#[derive(Clone)]
pub struct ApiServices {
    pub memory: Arc<dyn MemoryPort>,
    pub prompts: Arc<dyn PromptPort>,
    pub profile: Arc<dyn ProfilePort>,
    pub skills: Arc<dyn SkillPort>,
    pub embed: Arc<dyn EmbedPort>,
    pub maintenance: Arc<dyn MaintenancePort>,
    pub sessions: Arc<dyn SessionPort>,
    pub milestones: Arc<dyn MilestonePort>,
}

impl ApiServices {
    /// Every port answers [`PortError::Unavailable`].
    pub fn unavailable() -> Self {
        let unavailable = Arc::new(UnavailablePort);
        Self {
            memory: unavailable.clone(),
            prompts: unavailable.clone(),
            profile: unavailable.clone(),
            skills: unavailable.clone(),
            embed: unavailable.clone(),
            maintenance: unavailable.clone(),
            sessions: unavailable.clone(),
            milestones: unavailable,
        }
    }

    /// The production wiring over the real store (the W5.x composition root's
    /// entry point).
    ///
    /// `MemoryPort` / `PromptPort` / `ProfilePort` are the adapters in
    /// [`super::store_adapter`]; the embedder is injected (production wraps
    /// `EmbeddingService`, the tests inject a fixed-vector fake). Skills stay
    /// unavailable until W6.4. The root composition root attaches the session
    /// port through `.with_sessions(episodic_session_port(store))` (issue 37);
    /// `production()` itself keeps the unavailable default for every port it
    /// does not take in `ProductionDeps`. The composition root injects the
    /// deduplication port through `.with_maintenance(..)`; every migration
    /// method keeps answering `Unavailable` until the W5.x readiness gate
    /// supplies a `MigrationService`. The `with_skills` / `with_sessions` /
    /// `with_maintenance` builders are the injection points.
    pub fn production(deps: super::store_adapter::ProductionDeps) -> Self {
        Self {
            memory: Arc::new(super::store_adapter::ProductionMemory::new(deps.shards)),
            prompts: Arc::new(super::store_adapter::ProductionPrompts::new(deps.prompts)),
            profile: Arc::new(super::store_adapter::ProductionProfile::new(deps.profile)),
            skills: Arc::new(UnavailablePort),
            embed: deps.embed,
            maintenance: Arc::new(UnavailablePort),
            sessions: Arc::new(UnavailablePort),
            milestones: Arc::new(UnavailablePort),
        }
    }

    /// W6.4 injects the skills manager.
    pub fn with_skills(mut self, skills: Arc<dyn SkillPort>) -> Self {
        self.skills = skills;
        self
    }

    /// W6.3 injects the episodic session store.
    pub fn with_sessions(mut self, sessions: Arc<dyn SessionPort>) -> Self {
        self.sessions = sessions;
        self
    }

    /// W6.6 injects the episodic milestone projection.
    pub fn with_milestones(mut self, milestones: Arc<dyn MilestonePort>) -> Self {
        self.milestones = milestones;
        self
    }

    /// The cleanup/deduplication services (or their future fill).
    pub fn with_maintenance(mut self, maintenance: Arc<dyn MaintenancePort>) -> Self {
        self.maintenance = maintenance;
        self
    }

    pub fn with_memory(mut self, memory: Arc<dyn MemoryPort>) -> Self {
        self.memory = memory;
        self
    }

    pub fn with_prompts(mut self, prompts: Arc<dyn PromptPort>) -> Self {
        self.prompts = prompts;
        self
    }

    pub fn with_profile(mut self, profile: Arc<dyn ProfilePort>) -> Self {
        self.profile = profile;
        self
    }

    pub fn with_embed(mut self, embed: Arc<dyn EmbedPort>) -> Self {
        self.embed = embed;
        self
    }
}

impl Default for ApiServices {
    fn default() -> Self {
        Self::unavailable()
    }
}

/// The all-unavailable port implementation behind [`ApiServices::unavailable`].
struct UnavailablePort;

#[async_trait]
impl MemoryPort for UnavailablePort {
    async fn shards(
        &self,
        _scope: Option<Scope>,
        _scope_hash: Option<&str>,
    ) -> Result<Vec<ShardRef>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn verify_shards(&self, _shards: &[ShardRef]) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn all_memories(&self, _shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn list_memories_by_container_tag(
        &self,
        _shard: &ShardRef,
        _container_tag: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn memory_by_id(
        &self,
        _shard: &ShardRef,
        _id: &str,
    ) -> Result<Option<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn distinct_tags(&self, _shard: &ShardRef) -> Result<Vec<TagRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn insert_memory(
        &self,
        _scope: Scope,
        _scope_hash: &str,
        _memory: NewMemory,
    ) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn update_memory(
        &self,
        _shard: &ShardRef,
        _update: MemoryUpdate,
    ) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn pin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn unpin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn search_in_shard(
        &self,
        _shard: &ShardRef,
        _query_vector: &[f32],
        _container_tag: &str,
        _limit: usize,
        _query_text: Option<&str>,
    ) -> Result<Vec<SearchHit>, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl PromptPort for UnavailablePort {
    async fn captured_prompts(
        &self,
        _project_path: Option<&str>,
    ) -> Result<Vec<PromptRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn search_prompts(
        &self,
        _query: &str,
        _project_path: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<PromptRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn prompts_by_ids(&self, _ids: &[String]) -> Result<Vec<PromptRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn prompt_by_id(&self, _id: &str) -> Result<Option<PromptRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_prompt(&self, _id: &str) -> Result<PromptErasure, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl ProfilePort for UnavailablePort {
    async fn active_profile(&self, _user_id: &str) -> Result<Option<ProfileRecord>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn active_profiles(&self) -> Result<Vec<ProfileRecord>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_profile(&self, _profile_id: &str) -> Result<ProfileErasure, PortError> {
        Err(PortError::Unavailable)
    }

    async fn changelogs(
        &self,
        _profile_id: &str,
        _limit: usize,
    ) -> Result<Vec<ProfileChangelog>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn changelog_by_id(&self, _id: &str) -> Result<Option<ProfileChangelog>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn update_profile(
        &self,
        _profile_id: &str,
        _profile_data: &Value,
        _change_summary: &str,
    ) -> Result<bool, PortError> {
        Err(PortError::Unavailable)
    }

    async fn decay(&self, _profile_data: &Value) -> Result<(Value, bool), PortError> {
        Err(PortError::Unavailable)
    }

    async fn ai_cleanup(
        &self,
        _user_id: &str,
        _profile_version: i64,
        _profile_data: &Value,
        _include_ids: Option<Vec<String>>,
    ) -> Result<CleanupPreview, PortError> {
        Err(PortError::Unavailable)
    }

    async fn apply_cleanup(
        &self,
        _user_id: &str,
        _body: &Value,
    ) -> Result<CleanupApplied, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl SkillPort for UnavailablePort {
    async fn list(
        &self,
        _container_tag: Option<&str>,
        _enabled_only: bool,
    ) -> Result<Vec<Skill>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn get(&self, _id: &str) -> Result<Option<Skill>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn create(&self, _skill: NewSkill) -> Result<Skill, PortError> {
        Err(PortError::Unavailable)
    }

    async fn update(&self, _id: &str, _patch: SkillPatch) -> Result<Skill, PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete(&self, _id: &str) -> Result<bool, PortError> {
        Err(PortError::Unavailable)
    }

    async fn catalog_index(&self) -> Result<Vec<CatalogEntry>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn catalog_markdown(&self, _id: &str) -> Result<Option<String>, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl EmbedPort for UnavailablePort {
    async fn warmup(&self) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn embed(&self, _text: &str, _task: EmbedTask) -> Result<Vec<f32>, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl MaintenancePort for UnavailablePort {
    async fn detect_migration(&self) -> Result<MigrationReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn run_migration(&self, _strategy: &str) -> Result<MigrationRunReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn detect_tag_migration(&self) -> Result<TagMigrationReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn tag_migration_progress(&self) -> Result<TagMigrationProgressView, PortError> {
        Err(PortError::Unavailable)
    }

    async fn tag_migration_records(
        &self,
    ) -> Result<Vec<mem_core::tags::migration::MemoryRecord>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn generate_tags(&self, _memory_id: &str, _content: &str) -> Result<Vec<String>, String> {
        Err("the maintenance provider is unavailable".to_string())
    }

    async fn run_tag_migration_batch(
        &self,
        _batch_size: usize,
        _records: Vec<mem_core::tags::migration::MemoryRecord>,
        _tags: HashMap<String, Result<Vec<String>, String>>,
    ) -> Result<mem_core::tags::migration::BatchOutcome, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl SessionPort for UnavailablePort {
    async fn list_sessions(&self, _project_id: &str) -> Result<Vec<Value>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_session(&self, _project_id: &str, _session_id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn projects(&self, _limit: usize) -> Result<Vec<ProjectRowDto>, PortError> {
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl MilestonePort for UnavailablePort {
    async fn list_milestones(
        &self,
        _project_id: &str,
        _limit: usize,
    ) -> Result<Vec<MilestoneItemDto>, PortError> {
        Err(PortError::Unavailable)
    }
}
