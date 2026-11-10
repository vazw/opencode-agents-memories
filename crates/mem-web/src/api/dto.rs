//! Response DTOs, field order pinned to upstream `api-handlers.ts`.
//!
//! `serde` serializes struct fields in declaration order, so the declaration
//! order below *is* the upstream key order. Optional keys are omitted when
//! absent (`undefined` in upstream `JSON.stringify`), except `containerTag`
//! on a skill and `projectPath` on a prompt, where upstream emits `null`.

use mem_episodic::milestone::Milestone;
use serde::Serialize;
use serde_json::Value;

/// Upstream `safeToISOString(timestamp)`.
///
/// `null`/`undefined`, a non-number, a negative value or an out-of-range value
/// all fall back to "now"; otherwise the value is formatted
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` (the JS `Date#toISOString` shape).
pub fn safe_to_iso_string(timestamp: Option<i64>) -> String {
    let millis = timestamp.filter(|value| *value >= 0);
    millis
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|instant| instant.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(now_iso_string)
}

/// The current instant in the upstream ISO shape.
pub fn now_iso_string() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Upstream `toISOStringOrNull(value)`: unlike [`safe_to_iso_string`] there is
/// no "now" fallback — a finite (including negative) timestamp formats, an
/// out-of-range one maps to `null`.
pub fn iso_string_or_null(timestamp: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(timestamp)
        .map(|instant| instant.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Upstream `r.tags ? r.tags.split(",").map(trim) : []`.
pub fn split_tags(tags: Option<&str>) -> Vec<String> {
    tags.map(|raw| raw.split(',').map(|tag| tag.trim().to_string()).collect())
        .unwrap_or_default()
}

/// Upstream `TagInfo` (the `getDistinctTags` projection).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagInfoDto {
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_repo_url: Option<String>,
}

/// `GET /api/tags` data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagsDto {
    pub project: Vec<TagInfoDto>,
    pub user: Vec<TagInfoDto>,
    /// Memory tags ranked by use (count descending, tag ascending).
    pub memory_tags: Vec<MemoryTagDto>,
    /// Distinct memory tags matching the query, before `tagLimit`/`tagOffset`.
    pub memory_tag_total: usize,
}

/// One ranked memory tag.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MemoryTagDto {
    pub tag: String,
    pub count: u64,
}

/// One memory timeline item of `GET /api/memories`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItemDto {
    pub r#type: &'static str,
    pub id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_repo_url: Option<String>,
    pub is_pinned: bool,
}

/// One prompt timeline item of `GET /api/memories`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptItemDto {
    pub r#type: &'static str,
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub created_at: String,
    pub project_path: Option<String>,
    pub linked_memory_id: Option<String>,
}

/// A memory hit of `GET /api/search`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchMemoryDto {
    pub r#type: &'static str,
    pub id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub similarity: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_repo_url: Option<String>,
    pub is_pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_context: Option<bool>,
}

/// A prompt hit of `GET /api/search`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchPromptDto {
    pub r#type: &'static str,
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub created_at: String,
    pub project_path: Option<String>,
    pub linked_memory_id: Option<String>,
    pub similarity: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_context: Option<bool>,
}

/// One item of `GET /api/search` — a memory hit or a prompt hit. Untagged so
/// each variant keeps its own pinned field order.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SearchItemDto {
    Memory(SearchMemoryDto),
    Prompt(SearchPromptDto),
}

/// `PaginatedResponse<T>`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedDto<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub total_pages: usize,
}

/// `GET /api/stats` data — the upstream v2.33 shape, field order pinned to
/// `api-handlers.ts` `handleStats` (`total, byScope, byType, generatedAt,
/// shards, profile, daily, dailyError?`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub total: u64,
    pub by_scope: ByScopeDto,
    pub by_type: std::collections::BTreeMap<String, u64>,
    pub generated_at: String,
    pub shards: Vec<ShardStatsDto>,
    pub profile: ProfileStatsDto,
    pub daily: Vec<DailyStatsDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_error: Option<String>,
}

/// One shard's health row (upstream `ShardStats`, field order pinned).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShardStatsDto {
    pub shard_id: Option<i64>,
    pub scope: String,
    pub scope_hash: String,
    pub shard_index: u32,
    pub file_name: String,
    pub file_exists: bool,
    pub is_active: bool,
    pub registered_vector_count: u64,
    pub memory_count: u64,
    pub oldest_created_at: Option<String>,
    pub newest_created_at: Option<String>,
    pub duplicate_tag_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The profile summary of `GET /api/stats` (upstream `ProfileStats`) — counts
/// only, never the profile contents.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatsDto {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<ProfileCountsDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_analyzed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_prompts_analyzed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProfileStatsDto {
    /// The `{exists:false}` answer (missing profile or a store fault).
    pub fn missing() -> Self {
        Self {
            exists: false,
            version: None,
            counts: None,
            last_analyzed_at: None,
            total_prompts_analyzed: None,
            error: None,
        }
    }
}

/// The profile array counts (upstream `ProfileStats.counts`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProfileCountsDto {
    pub preferences: u64,
    pub patterns: u64,
    pub workflows: u64,
}

/// One day of `GET /api/stats`'s 14-day series (upstream `DailyStats`).
///
/// `errors` is a `BTreeMap`, so the JSON key order is stable (determinism).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailyStatsDto {
    pub date: String,
    pub captures: u64,
    pub capture_skipped: u64,
    pub profile_injections: u64,
    pub compaction_injections: u64,
    pub errors: std::collections::BTreeMap<String, u64>,
}

/// The `byScope` object.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ByScopeDto {
    pub user: u64,
    pub project: u64,
}

/// `{"id": "…"}` of `POST /api/memories`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AddedMemoryDto {
    pub id: String,
}

/// The `deletedPrompt` data of `DELETE /api/memories/{id}`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedMemoryDto {
    pub deleted_prompt: bool,
    /// Issue 43 W6c: the erasure state is ALWAYS present (D-19-3), so a delete
    /// answer is never readable as a complete erasure.
    pub erasure: ErasureStateDto,
}

/// The `deletedMemory` data of `DELETE /api/prompts/{id}`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedPromptDto {
    pub deleted_memory: bool,
    /// Issue 43 W6c: always present, additive (the field above is untouched).
    pub erasure: ErasureStateDto,
}

/// What survives a delete that the DTO can count.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SurvivingDerivedDto {
    /// Memories learned from the erased prompt; `None` when the count is not
    /// derivable (the UI renders the unconditional sentence).
    pub memories: Option<u64>,
    /// Milestones whose contributors were scrubbed. Zero by construction on a
    /// delete answer: the sweep has not run yet.
    pub milestones_scrubbed: u64,
}

/// One tier a delete does NOT erase, with the vault's tri-state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotErasedEntryDto {
    pub tier: &'static str,
    /// `"yes" | "no" | "unknown"` — a failed vault open is `unknown`, never `no`
    /// (ADR-0019 D-19-9, both directions pinned).
    pub erased: &'static str,
    pub reason: &'static str,
}

/// The erasure state of one deleted tier (ADR-0019 D-19-3, amended vocabulary).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErasureStateDto {
    /// `"pending"` on both HTTP paths in W6: `Erased` is the sweep report's
    /// answer after a completed pass, never the delete DTO's (ADR-0019 :165-167).
    pub state: &'static str,
    /// `"user-prompts" | "memories"` — never `"milestones"` on a delete answer.
    pub tier: &'static str,
    /// `"swept" | "notYetSwept" | "sweepFailed" | "legacyPeerLive" |
    /// "noSweepForTier"` (ADR-0019 D-19-3 as amended by W6c): `swept` only from
    /// the status read's stamp, `sweepFailed` when the read fails (never
    /// `notYetSwept`), `legacyPeerLive` unreachable from HTTP in W6c (the sweep's
    /// block record is report-only).
    pub sweep_state: &'static str,
    pub surviving_derived: SurvivingDerivedDto,
    /// The tiers a delete leaves, vault tri-state included.
    pub not_erased: Vec<NotErasedEntryDto>,
}

/// The `NOT_ERASED` map: the purge's surviving tiers plus the vault's
/// tri-state. `vault_erased == None` (a failed/unavailable vault open) renders
/// `"unknown"`, never `"no"` — the honest answer when the tool cannot look.
#[must_use]
pub fn not_erased_map(vault_erased: Option<bool>) -> Vec<NotErasedEntryDto> {
    let vault = match vault_erased {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    vec![
        NotErasedEntryDto {
            tier: "vault",
            erased: vault,
            reason: "encrypted at rest; the sweep excludes it (ADR-0012 D-14, ADR-0019 D-19-6)",
        },
        NotErasedEntryDto {
            tier: "memories-shard",
            erased: "no",
            reason: "no per-session provenance; Phase 4",
        },
        NotErasedEntryDto {
            tier: "milestones-recap",
            erased: "no",
            reason: "the merged milestone summary and its embedding survive a contributor scrub",
        },
    ]
}

/// The `deleted` count of the bulk-delete endpoints.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeletedCountDto {
    pub deleted: usize,
}

/// The missing-profile answer of `GET /api/user-profile`.
///
/// `message` is owned (issue 115, D-4): the zero-profile state still answers
/// upstream's literal byte-for-byte, but a root that holds OTHER identities
/// says so and names the requested one, instead of claiming no profile exists
/// while the picker offers two.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMissingDto {
    pub exists: bool,
    pub user_id: String,
    pub message: String,
}

/// One row of `GET /api/user-profiles` (issue 115 — the identity roster).
///
/// Identity-level fields only: no `profile_data`, so a roster read can never
/// become a second, unguarded profile payload path.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRosterItemDto {
    pub user_id: String,
    pub display_name: String,
    pub version: i64,
    pub total_prompts_analyzed: i64,
    /// `safe_to_iso_string` of the stored stamp.
    pub last_analyzed_at: String,
    /// Whether this row is the identity the composition root resolved.
    pub is_default: bool,
}

/// The populated profile answer of `GET /api/user-profile`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub exists: bool,
    pub id: String,
    pub user_id: String,
    pub display_name: String,
    pub user_name: String,
    pub user_email: String,
    pub version: i64,
    pub created_at: String,
    pub last_analyzed_at: String,
    pub total_prompts_analyzed: i64,
    pub profile_data: Value,
}

/// One formatted changelog entry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogDto {
    pub id: String,
    pub profile_id: String,
    pub version: i64,
    pub change_type: String,
    pub change_summary: String,
    pub created_at: String,
}

/// `GET /api/user-profile/snapshot` data.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSnapshotDto {
    pub version: i64,
    pub created_at: String,
    pub profile_data: Value,
}

/// `POST /api/user-profile/refresh` data.
///
/// D-4 (issue 48.13): `unanalyzedPrompts` was **removed**, not relabelled.
/// Upstream's response carried it; nothing consumes it in this tree (the
/// shipped Leptos UI never references it, and this release ships no automatic
/// profile learning), so the readout no longer advertises a backlog nothing
/// consumes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefreshProfileDto {
    pub message: &'static str,
    pub profile_exists: bool,
    pub decay_applied: bool,
    pub note: &'static str,
}

/// `POST /api/user-profile/ai-cleanup` data.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPreviewDto {
    pub old: Value,
    pub new: Value,
    pub changes: Value,
}

/// `POST /api/user-profile/ai-cleanup/apply` and `PATCH …/item` data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMutationDto {
    pub message: String,
    pub version: i64,
}

/// `DELETE /api/user-profile` data (issue 48.14 — the port-only erasure route).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileErasureDto {
    pub message: &'static str,
    pub profile_id: String,
    pub profiles_deleted: u64,
    pub changelogs_deleted: u64,
}

/// One skill of `skillToApi`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub autoinvoke: bool,
    pub enabled: bool,
    pub container_tag: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /api/skills` data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillsListDto {
    pub items: Vec<SkillDto>,
}

/// One catalog index entry (`{name, version, files}`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogEntryDto {
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
}

/// The bare catalog index body — NOT the envelope.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogIndexDto {
    pub skills: Vec<CatalogEntryDto>,
}

/// The `{deleted:true}` data of `DELETE /api/skills/{id}`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillDeletedDto {
    pub deleted: bool,
}

/// One shard mismatch of the migration detect report.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MigrationDetectDto {
    pub needs_migration: bool,
    pub config_dimensions: u32,
    pub config_model: String,
    pub shard_mismatches: Vec<Value>,
}

/// `POST /api/migration/run` data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRunDto {
    pub success: bool,
    pub strategy: String,
    pub deleted_shards: u32,
    pub re_embedded_memories: u32,
    pub duration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `GET /api/migration/tags/detect` data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagMigrationDetectDto {
    pub needs_migration: bool,
    pub count: u64,
}

/// `GET /api/migration/tags/progress` data (upstream `MigrationProgress`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagMigrationProgressDto {
    pub processed: usize,
    pub cursor: usize,
    pub total: usize,
    pub current_batch: u32,
    pub total_batches: u32,
    pub is_complete: bool,
    pub errors: Vec<String>,
}

/// `POST /api/migration/tags/run-batch` data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagMigrationBatchDto {
    pub processed: usize,
    pub total: usize,
    pub has_more: bool,
    pub errors: usize,
}

/// `POST /api/cleanup` data (issue 39).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupDto {
    pub deleted_count: u64,
    pub user_count: u64,
    pub project_count: u64,
    pub prompts_deleted: u64,
    pub protected_memories_skipped: u64,
}

impl From<super::ports::CleanupReport> for CleanupDto {
    fn from(report: super::ports::CleanupReport) -> Self {
        Self {
            deleted_count: report.deleted_count,
            user_count: report.user_count,
            project_count: report.project_count,
            prompts_deleted: report.prompts_deleted,
            protected_memories_skipped: report.protected_memories_skipped,
        }
    }
}

/// `POST /api/deduplicate` data.
///
/// Served whenever a `MaintenancePort` with a real `deduplicate()` is
/// injected (`api::deduplication::production_deduplication_port`); the
/// contract suite's `fake_services()` injects `FakeBackend`, whose
/// `deduplicate()` answers `PortError::Unavailable`; the D-38 no-composed-stores
/// degrade path installs `ApiServices::unavailable()`'s `UnavailablePort`.
/// Those two are what pin the documented 501 now that production injects the
/// real port.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DedupDto {
    pub exact_duplicates_deleted: u64,
    pub near_duplicate_groups: Vec<NearDuplicateGroupDto>,
    pub near_duplicate_scan: NearDuplicateScanDto,
}

/// One pass's near-duplicate scan accounting (`nearDuplicateScan`).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NearDuplicateScanDto {
    pub pairs_scanned: u64,
    pub pair_budget: u64,
    pub truncated: bool,
    pub unreadable_shards: u64,
}

/// One reported near-duplicate group (upstream `DuplicateGroup`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NearDuplicateGroupDto {
    pub representative: DedupMemoryRefDto,
    pub duplicates: Vec<DedupDuplicateDto>,
}

/// The survivor of an exact group (upstream `representative`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DedupMemoryRefDto {
    pub id: String,
    pub content: String,
    pub container_tag: String,
    pub created_at: i64,
}

/// One reported near-duplicate (upstream `duplicates[]`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DedupDuplicateDto {
    pub id: String,
    pub content: String,
    pub similarity: f64,
}

impl From<super::ports::DedupReport> for DedupDto {
    fn from(report: super::ports::DedupReport) -> Self {
        Self {
            exact_duplicates_deleted: report.exact_duplicates_deleted,
            near_duplicate_groups: report
                .near_duplicate_groups
                .into_iter()
                .map(|group| NearDuplicateGroupDto {
                    representative: DedupMemoryRefDto {
                        id: group.representative.id,
                        content: group.representative.content,
                        container_tag: group.representative.container_tag,
                        created_at: group.representative.created_at,
                    },
                    duplicates: group
                        .duplicates
                        .into_iter()
                        .map(|duplicate| DedupDuplicateDto {
                            id: duplicate.id,
                            content: duplicate.content,
                            similarity: duplicate.similarity,
                        })
                        .collect(),
                })
                .collect(),
            near_duplicate_scan: NearDuplicateScanDto {
                pairs_scanned: report.near_duplicate_scan.pairs_scanned,
                pair_budget: report.near_duplicate_scan.pair_budget,
                truncated: report.near_duplicate_scan.truncated,
                unreadable_shards: report.near_duplicate_scan.unreadable_shards,
            },
        }
    }
}

/// The session list payload (W6.3 owns the shape; the stub never produces it).
pub type SessionListDto = Vec<Value>;

/// One `GET /api/projects` row (issue 116, D-6): a project of the roster with
/// its live-session count, its milestone count and its newest activity stamp.
///
/// `project_id` is path-shaped data, not a path: it is returned verbatim.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRowDto {
    pub project_id: String,
    pub session_count: i64,
    pub milestone_count: i64,
    pub last_activity_at: i64,
}

/// One `GET /api/projects/{project_id}/milestones` row (W6.6): the milestone's
/// own camelCase projection — `Milestone` is already `camelCase` on the wire
/// and never serializes its write-only `embedding` — plus the durable facts it
/// promoted.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneItemDto {
    #[serde(flatten)]
    pub milestone: Milestone,
    pub promoted_facts: Vec<PromotedFactDto>,
}

/// One durable fact promoted by a milestone.
///
/// No `memoryType`: durable facts carry a `tier`, and the upstream memory-type
/// vocabulary is a different concept. Promoted facts are agent-anonymous by
/// construction — their attribution is the milestone that carries them.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromotedFactDto {
    pub id: String,
    pub content: String,
    pub created_at: i64,
}
