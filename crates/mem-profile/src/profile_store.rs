//! `ProfileStore` — the minimal `UserProfileManager` port (W3.1 step 5).
//!
//! Storage is upstream's actual `user-profiles.db` (D-V1): `user_profiles` +
//! `user_profile_changelogs`, DDL verbatim in `mem_store::sql`.
//!
//! Scope of this wave (blueprint Step 5 + Step 7): DB ops (create / read /
//! optimistic-version update / changelogs) and the merge paths the W3.1
//! acceptance criteria exercise — exact `(category, description)` dedup, the
//! cold-start buffer push/drain, and appending a new observation. The
//! AI-ownership areas (conflict detection, three-way merges, drift fuse,
//! Thompson weak-hit upgrades, AI cleanup) stay inert/deferred to W3.3, as
//! the blueprint directs.
//!
//! DIVERGENCE (reviewer note): `CONFIG.storagePath` becomes an explicit
//! constructor argument (the established W1.x/W2.x pattern); `ProfileConfig`
//! carries the behavioural knobs and is built from `mem_core`'s resolved
//! config, so the store never re-resolves user config.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mem_core::sync::lock_or_recover;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, DbHandle, Value, query_one, turso_connect_factory};
use mem_store::platform::Platform;
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, user_profiles_migrations,
};
use serde::{Deserialize, Serialize};

use crate::erasure::{EraseReport, could_have_contributed};
use crate::error::ProfileError;
use crate::ids::{next_changelog_id, next_profile_id, now_millis};

/// Upstream `USER_PROFILES_DB_NAME`.
pub const USER_PROFILES_DB_NAME: &str = "user-profiles.db";

/// Upstream `COLD_BUFFER_DEFAULT_KEY` — the bucket for merges that ran
/// without a profile id.
pub const COLD_BUFFER_DEFAULT_KEY: &str = "__unattributed__";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// The behavioural knobs the profile store reads (the `mem_core` resolved
/// config slice for this crate; upstream reads the `CONFIG` singleton).
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileConfig {
    pub auto_capture_enabled: bool,
    pub auto_capture_max_retries: u32,
    pub analysis_interval: u32,
    pub embedding_min_description_length: u32,
    pub embedding_threshold_same_cat: f64,
    pub embedding_threshold_same_cat_weak: f64,
    pub embedding_threshold_cross_cat: f64,
    pub embedding_threshold_cross_cat_weak: f64,
    pub centroid_drift_threshold: f64,
    pub confidence_decay_days: u32,
    pub stale_days: u32,
    pub min_evidence_for_retention: u32,
    pub changelog_retention_count: u32,
    pub validation_enabled: bool,
    pub auto_cleanup_enabled: bool,
    pub auto_cleanup_interval: f64,
}

impl Default for ProfileConfig {
    /// Upstream `config.ts` defaults (the pinned values in
    /// `mem_core::config::schema`).
    fn default() -> Self {
        Self {
            auto_capture_enabled: true,
            auto_capture_max_retries: 3,
            analysis_interval: 10,
            embedding_min_description_length: 5,
            embedding_threshold_same_cat: 0.8,
            embedding_threshold_same_cat_weak: 0.5,
            embedding_threshold_cross_cat: 0.9,
            embedding_threshold_cross_cat_weak: 0.8,
            centroid_drift_threshold: 0.65,
            confidence_decay_days: 30,
            stale_days: 2,
            min_evidence_for_retention: 3,
            changelog_retention_count: 5,
            validation_enabled: false,
            auto_cleanup_enabled: true,
            auto_cleanup_interval: 100.0,
        }
    }
}

impl ProfileConfig {
    /// Copies the profile slice out of a resolved `mem_core` config.
    pub fn from_core(config: &mem_core::config::Config) -> Self {
        Self {
            auto_capture_enabled: config.auto_capture_enabled,
            auto_capture_max_retries: config.auto_capture_max_retries,
            analysis_interval: config.user_profile_analysis_interval,
            embedding_min_description_length: config.user_profile_embedding_min_description_length,
            embedding_threshold_same_cat: config.user_profile_embedding_threshold_same_cat,
            embedding_threshold_same_cat_weak: config
                .user_profile_embedding_threshold_same_cat_weak,
            embedding_threshold_cross_cat: config.user_profile_embedding_threshold_cross_cat,
            embedding_threshold_cross_cat_weak: config
                .user_profile_embedding_threshold_cross_cat_weak,
            centroid_drift_threshold: config.user_profile_centroid_drift_threshold,
            confidence_decay_days: config.user_profile_confidence_decay_days,
            stale_days: config.user_profile_stale_days,
            min_evidence_for_retention: config.user_profile_min_evidence_for_retention,
            changelog_retention_count: config.user_profile_changelog_retention_count,
            validation_enabled: config.user_profile_validation_enabled,
            auto_cleanup_enabled: config.user_profile_auto_cleanup_enabled,
            auto_cleanup_interval: f64::from(config.user_profile_auto_cleanup_interval),
        }
    }
}

// ---------------------------------------------------------------------------
// Data shapes (upstream `user-profile/types.ts`)
// ---------------------------------------------------------------------------

/// One profile entry (preference / pattern / workflow share this shape
/// upstream; `steps` is the workflow-only extra).
///
/// `rename_all = "camelCase"` matches the stored JSON; unknown fields are
/// preserved through `extra` so a round trip never drops data written by a
/// peer (or a future version).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileItem {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_match_time: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centroid: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weak_alpha: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weak_beta: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weak_hit_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_weak_hit_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_below_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_validation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<String>>,
    /// Fields this wave does not model (and any future ones) survive a
    /// round trip verbatim.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ProfileItem {
    /// Upstream `item.frequency || 1` (JS falsy-zero semantics).
    pub fn frequency_or_one(&self) -> i64 {
        match self.frequency {
            Some(value) if value != 0 => value,
            _ => 1,
        }
    }

    /// Upstream `this.ensureArray(item.evidence)`.
    pub fn evidence_or_empty(&self) -> Vec<String> {
        self.evidence.clone().unwrap_or_default()
    }
}

/// Upstream `UserProfileData`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProfileData {
    #[serde(default)]
    pub preferences: Vec<ProfileItem>,
    #[serde(default)]
    pub patterns: Vec<ProfileItem>,
    #[serde(default)]
    pub workflows: Vec<ProfileItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_paths: Option<Vec<serde_json::Value>>,
}

/// A category array is written back to disk only when present in the update
/// (upstream `if (updates.preferences)` — an empty `Some(vec![])` still runs
/// the merge, which is what makes a warm drain fire).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfileUpdates {
    pub preferences: Option<Vec<ProfileItem>>,
    pub patterns: Option<Vec<ProfileItem>>,
    pub workflows: Option<Vec<ProfileItem>>,
}

/// Upstream `UserProfile` row (profile_data is the raw stored JSON text).
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
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
    pub is_active: bool,
}

/// Upstream `UserProfileChangelog` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileChangelog {
    pub id: String,
    pub profile_id: String,
    pub version: i64,
    pub change_type: String,
    pub change_summary: String,
    pub profile_data_snapshot: String,
    pub created_at: i64,
}

/// The result of one in-memory decay pass (upstream `decayInMemory`'s
/// `{ data, hasChanges }`).
#[derive(Debug, Clone, PartialEq)]
pub struct DecayOutcome {
    pub data: ProfileData,
    /// `true` when any item was dropped or had its confidence re-derived —
    /// i.e. the caller must persist `data`.
    pub has_changes: bool,
}

/// What one persisted sweep touched (R5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecaySweepReport {
    pub profiles_scanned: usize,
    pub profiles_updated: usize,
}

/// One category array's decay result.
struct DecayedItems {
    items: Vec<ProfileItem>,
    has_changes: bool,
}

/// Upstream `decayItems`: migrate, re-derive, then drop the stale-and-thin.
fn decay_items(
    items: &[ProfileItem],
    now: i64,
    stale_days: f64,
    min_evidence: usize,
) -> DecayedItems {
    let mut kept: Vec<ProfileItem> = Vec::with_capacity(items.len());
    let mut has_changes = false;

    for item in items {
        let mut item = item.clone();
        lazy_migrate_alpha(&mut item, now);
        if item.last_seen.is_none() {
            item.last_seen = Some(now);
        }
        if item.evidence.is_none() {
            item.evidence = Some(Vec::new());
        }

        let previous = item.confidence;
        sync_confidence(&mut item, now);
        if item.confidence != previous {
            has_changes = true;
        }

        let last_seen = item.last_seen.unwrap_or(now);
        let age = (now - last_seen) as f64;
        let evidence_count = item.evidence_or_empty().len();
        if crate::decay::is_stale(age, stale_days, evidence_count, min_evidence) {
            // Never log profile payloads: prefer the category and fall back to
            // a character-truncated (never byte-sliced, R8) description snippet,
            // matching upstream's `category || description.substring(0, 30)`.
            let snippet: String = item.description.chars().take(30).collect();
            tracing::debug!(
                category = item.category.as_deref().unwrap_or(&snippet),
                evidence_count,
                min_evidence,
                "profile decay: removed stale"
            );
            has_changes = true;
            continue;
        }
        kept.push(item);
    }

    DecayedItems {
        items: kept,
        has_changes,
    }
}

/// The embedding boundary (upstream's injectable `EmbeddingService`).
///
/// W3.1 injects mocks; W1.4's real service is adapted by a later wave.
pub trait EmbedBoundary: Send + Sync {
    /// Upstream `embed.isWarmedUp` — when cold, non-explicit observations are
    /// buffered instead of merged.
    fn is_warmed_up(&self) -> bool;
    /// Upstream `embed.embed(text, { task: "document" })`.
    fn embed(&self, text: &str) -> Result<Vec<f32>, ProfileError>;
}

/// Which category array a merge is running over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemType {
    Preference,
    Pattern,
    Workflow,
}

impl ItemType {
    /// Upstream `coldBuffer[itemType + "s"]`.
    fn slice_mut(self, bucket: &mut crate::cold_buffer::Bucket) -> &mut Vec<ProfileItem> {
        match self {
            Self::Preference => &mut bucket.preferences,
            Self::Pattern => &mut bucket.patterns,
            Self::Workflow => &mut bucket.workflows,
        }
    }
}

/// The in-memory cold-buffer cache plus the failure recorded by the
/// fail-safe constructor path (upstream `coldBufferLoadError`).
#[derive(Debug, Default)]
struct ColdState {
    buffers: crate::cold_buffer::Snapshot,
    load_error: Option<String>,
}

/// The profile store over one `user-profiles.db`.
pub struct ProfileStore {
    storage_path: PathBuf,
    db_path: PathBuf,
    cold_buffer_path: PathBuf,
    connections: ConnectionManager,
    handle: tokio::sync::OnceCell<DbHandle>,
    config: ProfileConfig,
    /// Cold-start buffers keyed by profile id, so items observed for one user
    /// never drain into another user's merge (cross-user contamination wall).
    cold_state: Mutex<ColdState>,
    /// The instant of the last persisted decay sweep (R5); `None` until the
    /// first sweep. A raw `Option<i64>` (not a `SystemTime`) keeps the clock
    /// injected — the sweep never reads a wall clock itself.
    last_decay_sweep: Mutex<Option<i64>>,
}

impl ProfileStore {
    /// Production constructor over the real turso factory.
    pub fn new(storage_path: PathBuf, store_config: StoreConfig, config: ProfileConfig) -> Self {
        Self::with_factory(
            storage_path,
            store_config,
            Arc::new(turso_connect_factory()),
            config,
        )
    }

    /// Constructor with an injected [`ConnectFactory`] (test seam).
    pub fn with_factory(
        storage_path: PathBuf,
        store_config: StoreConfig,
        factory: Arc<dyn ConnectFactory>,
        config: ProfileConfig,
    ) -> Self {
        let db_path = storage_path.join(USER_PROFILES_DB_NAME);
        let cold_buffer_path = crate::cold_buffer::cold_buffer_path(&storage_path);
        let platform = Platform::current();
        let connections = ConnectionManager::new(
            storage_path.clone(),
            store_config.clone(),
            platform,
            factory,
        );
        // Upstream evaluates `loadColdBuffersFailSafe()` in the constructor: a
        // non-critical cache read error must never crash the plugin import
        // path, so it is only recorded and the next merge round fails closed.
        let cold_state = Mutex::new(load_cold_state_fail_safe(&cold_buffer_path));
        Self {
            storage_path,
            db_path,
            cold_buffer_path,
            connections,
            handle: tokio::sync::OnceCell::new(),
            config,
            cold_state,
            last_decay_sweep: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &ProfileConfig {
        &self.config
    }

    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Upstream `reset()` — forget the initialized handle AND reload the
    /// cold-buffer snapshot: keeping the old in-memory map would resurrect the
    /// previous storage's buckets on the next save.
    pub fn reset(&mut self) {
        self.handle = tokio::sync::OnceCell::new();
        if let Ok(mut state) = self.cold_state.lock() {
            *state = load_cold_state_fail_safe(&self.cold_buffer_path);
        }
        if let Ok(mut last) = self.last_decay_sweep.lock() {
            *last = None;
        }
    }

    /// Upstream `CONFIG.storagePath = <new>; reset()`.
    pub fn rebind(&mut self, storage_path: PathBuf, store_config: StoreConfig) {
        self.storage_path = storage_path.clone();
        self.db_path = storage_path.join(USER_PROFILES_DB_NAME);
        self.cold_buffer_path = crate::cold_buffer::cold_buffer_path(&storage_path);
        self.connections = ConnectionManager::new(
            storage_path,
            store_config,
            Platform::current(),
            Arc::new(turso_connect_factory()),
        );
        self.handle = tokio::sync::OnceCell::new();
        if let Ok(mut state) = self.cold_state.lock() {
            *state = load_cold_state_fail_safe(&self.cold_buffer_path);
        }
        if let Ok(mut last) = self.last_decay_sweep.lock() {
            *last = None;
        }
    }

    /// The on-disk cold-buffer location.
    pub fn cold_buffer_path(&self) -> &Path {
        &self.cold_buffer_path
    }

    /// The last cold-buffer load failure recorded by the fail-safe paths.
    pub fn cold_buffer_load_error(&self) -> Option<String> {
        self.cold_state
            .lock()
            .ok()
            .and_then(|state| state.load_error.clone())
    }

    /// Upstream `beginColdBufferRound()` — a strict reload; on failure the
    /// error is recorded and propagated so no mutation proceeds against
    /// unknown on-disk state (fail-closed).
    pub fn begin_cold_buffer_round(&self) -> Result<(), ProfileError> {
        match crate::cold_buffer::load_snapshot(&self.cold_buffer_path) {
            Ok(snapshot) => {
                // cold_state: reload writes buffers then load_error = None, adjacent, neither panics (ADR-0013 D-2 D2-R)
                let mut state = lock_or_recover(&self.cold_state);
                state.buffers = snapshot;
                state.load_error = None;
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                // cold_state: failure path stores one diagnostic String; the file is the durable record (ADR-0013 D-2 D2-R)
                let mut state = lock_or_recover(&self.cold_state);
                state.load_error = Some(message);
                Err(error)
            }
        }
    }

    /// Upstream `getColdBuffer(profileId?)` — the unattributed bucket is used
    /// when a merge ran without a profile id.
    fn drain_cold_bucket(&self, profile_id: Option<&str>, item_type: ItemType) -> Vec<ProfileItem> {
        // cold_state: mem::take of one item-type bucket (ADR-0013 D-2 D2-R)
        let mut state = lock_or_recover(&self.cold_state);
        let key = profile_id.unwrap_or(COLD_BUFFER_DEFAULT_KEY).to_string();
        let bucket = state.buffers.entry(key).or_default();
        std::mem::take(item_type.slice_mut(bucket))
    }

    /// Upstream's cold-start push: buffer an observation, cap the buffer at 50
    /// (oldest dropped), then persist the whole snapshot.
    fn buffer_cold_item(
        &self,
        profile_id: Option<&str>,
        item_type: ItemType,
        item: &ProfileItem,
    ) -> Result<(), ProfileError> {
        {
            // cold_state: push + cap on one bucket; a lost push loses one buffered observation (ADR-0013 D-2 D2-R)
            let mut state = lock_or_recover(&self.cold_state);
            let key = profile_id.unwrap_or(COLD_BUFFER_DEFAULT_KEY).to_string();
            let bucket = state.buffers.entry(key).or_default();
            let slot = item_type.slice_mut(bucket);
            slot.push(item.clone());
            if slot.len() > 50 {
                slot.remove(0);
            }
        }
        self.save_cold_buffers()
    }

    /// Upstream `saveColdBuffers()` — persists the current in-memory snapshot.
    pub fn save_cold_buffers(&self) -> Result<(), ProfileError> {
        let snapshot = {
            // cold_state: clone of buffers for persistence; no mutation (ADR-0013 D-2 D2-R)
            let state = lock_or_recover(&self.cold_state);
            state.buffers.clone()
        };
        crate::cold_buffer::save_snapshot(&self.cold_buffer_path, &snapshot)
    }

    /// Current in-memory snapshot (test/inspection helper — upstream has no
    /// equivalent; peers are observed through the file itself).
    pub fn cold_snapshot(&self) -> crate::cold_buffer::Snapshot {
        // cold_state: clone for inspection; no mutation (ADR-0013 D-2 D2-R)
        lock_or_recover(&self.cold_state).buffers.clone()
    }

    /// Upstream `deleteProfile` — the B2′ two-table erase, returning what it
    /// actually removed (`ExecResult.changes`, never estimates).
    ///
    /// Statement order is load-bearing:
    /// 1. `user_profile_changelogs` first — every row carries a full profile
    ///    snapshot in `profile_data_snapshot`, so deleting the profile alone
    ///    would erase nothing;
    /// 2. `user_profiles`;
    /// 3. the cold-buffer round + bucket removal — a bucket left behind
    ///    resurrects the deleted profile's buffered observations.
    ///
    /// The declared `ON DELETE CASCADE` on `user_profile_changelogs` is **not**
    /// relied on: the table is created with `CREATE TABLE IF NOT EXISTS`,
    /// which cannot add an FK to a table a migrated root already has, and no
    /// test exercises the cascade. The explicit delete is the contract.
    ///
    /// Two call-site consequences, both deliberate:
    /// - D-3: `mem-portability`'s `compensate()` rollback now also removes the
    ///   just-created profile's changelog rows (they would otherwise be
    ///   orphans);
    /// - D-2 corollary: `EraseReport` is **not** `#[must_use]`, so that
    ///   rollback's `let _ = …` line compiles unchanged.
    ///
    /// A concurrent peer deleting the same profile first makes both statements
    /// change 0 rows — idempotent success, not an error.
    pub async fn delete_profile(&self, profile_id: &str) -> Result<EraseReport, ProfileError> {
        let db = self.ready().await?;
        let changelogs = db
            .execute(
                "DELETE FROM user_profile_changelogs WHERE profile_id = ?",
                vec![Value::Text(profile_id.to_string())],
            )
            .await?;
        let profiles = db
            .execute(
                "DELETE FROM user_profiles WHERE id = ?",
                vec![Value::Text(profile_id.to_string())],
            )
            .await?;
        self.begin_cold_buffer_round()?;
        let removed = {
            // cold_state: single bucket remove after the on-disk re-read (ADR-0013 D-2 D2-R)
            let mut state = lock_or_recover(&self.cold_state);
            state.buffers.remove(profile_id).is_some()
        };
        if removed {
            self.save_cold_buffers()?;
        }
        Ok(EraseReport {
            profiles_considered: 1,
            profiles_deleted: profiles.changes,
            changelogs_deleted: changelogs.changes,
        })
    }

    /// The B2′ scan (47.1-B2′): erase every profile whose watermark says the
    /// prompt created at `prompt_created_at` could have contributed to it.
    ///
    /// **All** `user_profiles` rows are considered, not only `is_active = 1`:
    /// an inactive row still holds derived text (`get_all_active_profiles` is
    /// deliberately not reused for exactly that reason). An unreadable
    /// watermark reads as `0`, which contributes — the direction is
    /// conservative.
    ///
    /// Scan-then-delete is not atomic: a profile whose watermark advances
    /// between the scan and the delete is over-erased, never under-erased.
    /// Accepted — the direction is conservative, and under A2 no learner can
    /// move the watermark (D-5 records that the write paths *would*).
    pub async fn erase_profiles_derived_from(
        &self,
        prompt_created_at: i64,
    ) -> Result<EraseReport, ProfileError> {
        let db = self.ready().await?;
        let rows = db
            .query("SELECT id, last_analyzed_at FROM user_profiles", Vec::new())
            .await?;
        let mut report = EraseReport::default();
        for row in &rows {
            let Some(profile_id) = row.text("id") else {
                // `id` is the NOT NULL primary key; without it there is no
                // target a DELETE could name.
                continue;
            };
            // NOT NULL in the DDL (`sql.rs`), so an unreadable value is a
            // migrated-row case; 0 contributes under every prompt timestamp
            // (fail toward erasure, never retention).
            let watermark = row.i64("last_analyzed_at").unwrap_or(0);
            report.profiles_considered += 1;
            if !could_have_contributed(watermark, prompt_created_at) {
                continue;
            }
            let erased = self.delete_profile(&profile_id).await?;
            report.profiles_deleted += erased.profiles_deleted;
            report.changelogs_deleted += erased.changelogs_deleted;
        }
        Ok(report)
    }

    /// Upstream `ready()`.
    pub async fn ready(&self) -> Result<DbHandle, ProfileError> {
        let handle = self
            .handle
            .get_or_try_init(|| async { self.open_and_init().await })
            .await?;
        Ok(handle.clone())
    }

    /// Upstream `initDatabase()` — `USER_PROFILES_MIGRATIONS`.
    async fn open_and_init(&self) -> Result<DbHandle, ProfileError> {
        let db = self.connections.get_connection(&self.db_path).await?;
        let options = MigrationOptions {
            db_path: Some(self.db_path.clone()),
            label: USER_PROFILES_DB_NAME,
        };
        apply_schema_migrations(db.as_ref(), &user_profiles_migrations(), &options).await?;
        Ok(db)
    }

    // -----------------------------------------------------------------------
    // DB ops
    // -----------------------------------------------------------------------

    /// Upstream `createProfile` (always version 1, active, with a `create`
    /// changelog row).
    pub async fn create_profile(
        &self,
        user_id: &str,
        display_name: &str,
        user_name: &str,
        user_email: &str,
        profile_data: &ProfileData,
        prompts_analyzed: i64,
    ) -> Result<String, ProfileError> {
        let db = self.ready().await?;
        let id = next_profile_id();
        let now = now_millis();
        let cleaned = Self::clean_data(profile_data);
        let serialized = serde_json::to_string(&cleaned)?;

        db.execute(
            "
      INSERT INTO user_profiles (
        id, user_id, display_name, user_name, user_email,
        profile_data, version, created_at, last_analyzed_at,
        total_prompts_analyzed, is_active
      )
      VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, 1)
    ",
            vec![
                Value::Text(id.clone()),
                Value::Text(user_id.to_string()),
                Value::Text(display_name.to_string()),
                Value::Text(user_name.to_string()),
                Value::Text(user_email.to_string()),
                Value::Text(serialized),
                Value::Integer(now),
                Value::Integer(now),
                Value::Integer(prompts_analyzed),
            ],
        )
        .await?;

        self.add_changelog(&id, 1, "create", "Initial profile creation", &cleaned)
            .await?;

        Ok(id)
    }

    /// Upstream `updateProfile` — optimistic version check; `false` means the
    /// caller lost the race (no changelog written).
    pub async fn update_profile(
        &self,
        profile_id: &str,
        profile_data: &ProfileData,
        additional_prompts_analyzed: i64,
        change_summary: &str,
    ) -> Result<bool, ProfileError> {
        let db = self.ready().await?;
        let now = now_millis();
        let cleaned = Self::clean_data(profile_data);
        let serialized = serde_json::to_string(&cleaned)?;

        let version_row = query_one(
            db.as_ref(),
            "SELECT version FROM user_profiles WHERE id = ?",
            vec![Value::Text(profile_id.to_string())],
        )
        .await?;
        let current_version = version_row
            .as_ref()
            .and_then(|row| row.i64("version"))
            .unwrap_or(0);
        let new_version = current_version + 1;

        let result = db
            .execute(
                "
      UPDATE user_profiles
      SET profile_data = ?,
          version = ?,
          last_analyzed_at = ?,
          total_prompts_analyzed = total_prompts_analyzed + ?
      WHERE id = ? AND version = ?
    ",
                vec![
                    Value::Text(serialized),
                    Value::Integer(new_version),
                    Value::Integer(now),
                    Value::Integer(additional_prompts_analyzed),
                    Value::Text(profile_id.to_string()),
                    Value::Integer(current_version),
                ],
            )
            .await?;

        if result.changes == 0 {
            return Ok(false);
        }

        self.add_changelog(profile_id, new_version, "update", change_summary, &cleaned)
            .await?;
        self.cleanup_old_changelogs(profile_id).await?;

        Ok(true)
    }

    /// W6.4 — the portability import seam: write a profile **verbatim**.
    ///
    /// The id, the version and every timestamp are the SOURCE's, so a
    /// profile survives an export/import round trip unchanged; the write is
    /// an upsert, so importing into a store that already holds the profile
    /// replaces it instead of failing on the primary key. A changelog row
    /// records the import.
    ///
    /// `true` means the row was written, `false` means the store refused
    /// the write (the caller decides what to report).
    pub async fn import_profile(&self, profile: Profile) -> Result<bool, ProfileError> {
        let db = self.ready().await?;
        let result = db
            .execute(
                "
      INSERT INTO user_profiles (
        id, user_id, display_name, user_name, user_email,
        profile_data, version, created_at, last_analyzed_at,
        total_prompts_analyzed, is_active
      )
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET
        user_id = excluded.user_id,
        display_name = excluded.display_name,
        user_name = excluded.user_name,
        user_email = excluded.user_email,
        profile_data = excluded.profile_data,
        version = excluded.version,
        created_at = excluded.created_at,
        last_analyzed_at = excluded.last_analyzed_at,
        total_prompts_analyzed = excluded.total_prompts_analyzed,
        is_active = excluded.is_active
    ",
                vec![
                    Value::Text(profile.id.clone()),
                    Value::Text(profile.user_id.clone()),
                    Value::Text(profile.display_name.clone()),
                    Value::Text(profile.user_name.clone()),
                    Value::Text(profile.user_email.clone()),
                    Value::Text(profile.profile_data.clone()),
                    Value::Integer(profile.version),
                    Value::Integer(profile.created_at),
                    Value::Integer(profile.last_analyzed_at),
                    Value::Integer(profile.total_prompts_analyzed),
                    Value::Integer(i64::from(profile.is_active)),
                ],
            )
            .await?;
        if result.changes == 0 {
            return Ok(false);
        }
        // The changelog records the import; the snapshot is the imported
        // payload itself (the import preserves the source's data).
        let snapshot = profile.profile_data.clone();
        let id = next_changelog_id();
        let now = now_millis();
        db.execute(
            "
      INSERT INTO user_profile_changelogs (
        id, profile_id, version, change_type, change_summary,
        profile_data_snapshot, created_at
      )
      VALUES (?, ?, ?, ?, ?, ?, ?)
    ",
            vec![
                Value::Text(id),
                Value::Text(profile.id.clone()),
                Value::Integer(profile.version),
                Value::Text("import".to_string()),
                Value::Text("Imported from a portability bundle".to_string()),
                Value::Text(snapshot),
                Value::Integer(now),
            ],
        )
        .await?;
        Ok(true)
    }

    /// The alias of [`Self::import_profile`] (the blueprint names the seam
    /// `import_profile` / `upsert_profile`).
    pub async fn upsert_profile(&self, profile: Profile) -> Result<bool, ProfileError> {
        self.import_profile(profile).await
    }

    /// Upstream `addChangelog`.
    async fn add_changelog(
        &self,
        profile_id: &str,
        version: i64,
        change_type: &str,
        change_summary: &str,
        profile_data: &ProfileData,
    ) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        let id = next_changelog_id();
        let now = now_millis();
        let serialized = serde_json::to_string(profile_data)?;

        db.execute(
            "
      INSERT INTO user_profile_changelogs (
        id, profile_id, version, change_type, change_summary,
        profile_data_snapshot, created_at
      )
      VALUES (?, ?, ?, ?, ?, ?, ?)
    ",
            vec![
                Value::Text(id),
                Value::Text(profile_id.to_string()),
                Value::Integer(version),
                Value::Text(change_type.to_string()),
                Value::Text(change_summary.to_string()),
                Value::Text(serialized),
                Value::Integer(now),
            ],
        )
        .await?;
        Ok(())
    }

    /// Upstream `cleanupOldChangelogs` (retention by version DESC).
    async fn cleanup_old_changelogs(&self, profile_id: &str) -> Result<(), ProfileError> {
        let db = self.ready().await?;
        db.execute(
            "
      DELETE FROM user_profile_changelogs
      WHERE profile_id = ?
      AND id NOT IN (
        SELECT id FROM user_profile_changelogs
        WHERE profile_id = ?
        ORDER BY version DESC
        LIMIT ?
      )
    ",
            vec![
                Value::Text(profile_id.to_string()),
                Value::Text(profile_id.to_string()),
                Value::Integer(i64::from(self.config.changelog_retention_count)),
            ],
        )
        .await?;
        Ok(())
    }

    /// Upstream `getProfileChangelogs(profileId, limit = 10)`.
    pub async fn get_profile_changelogs(
        &self,
        profile_id: &str,
        limit: i64,
    ) -> Result<Vec<ProfileChangelog>, ProfileError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                "
      SELECT * FROM user_profile_changelogs
      WHERE profile_id = ?
      ORDER BY version DESC
      LIMIT ?
    ",
                vec![Value::Text(profile_id.to_string()), Value::Integer(limit)],
            )
            .await?;
        Ok(rows.iter().map(row_to_changelog).collect())
    }

    /// Upstream `getActiveProfile` (`is_active = 1`; unknown user ⇒ `None`,
    /// never an auto-create).
    pub async fn get_active_profile(&self, user_id: &str) -> Result<Option<Profile>, ProfileError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            "
      SELECT * FROM user_profiles
      WHERE user_id = ? AND is_active = 1
      LIMIT 1
    ",
            vec![Value::Text(user_id.to_string())],
        )
        .await?;
        Ok(row.as_ref().map(row_to_profile))
    }

    /// Upstream `getProfileById`.
    pub async fn get_profile_by_id(
        &self,
        profile_id: &str,
    ) -> Result<Option<Profile>, ProfileError> {
        let db = self.ready().await?;
        let row = query_one(
            db.as_ref(),
            "SELECT * FROM user_profiles WHERE id = ?",
            vec![Value::Text(profile_id.to_string())],
        )
        .await?;
        Ok(row.as_ref().map(row_to_profile))
    }

    /// Upstream `getAllActiveProfiles`.
    pub async fn get_all_active_profiles(&self) -> Result<Vec<Profile>, ProfileError> {
        let db = self.ready().await?;
        let rows = db
            .query(
                "SELECT * FROM user_profiles WHERE is_active = 1",
                Vec::new(),
            )
            .await?;
        Ok(rows.iter().map(row_to_profile).collect())
    }

    /// Upstream `safeArray` on the three category arrays: a missing category
    /// is stored as an empty array (never `null`).
    fn clean_data(profile_data: &ProfileData) -> ProfileData {
        ProfileData {
            preferences: profile_data.preferences.clone(),
            patterns: profile_data.patterns.clone(),
            workflows: profile_data.workflows.clone(),
            learning_paths: profile_data.learning_paths.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Decay (upstream `decayInMemory` + the W3.3 daily sweep, R5)
    // -----------------------------------------------------------------------

    /// Port of upstream `decayInMemory` L559-635: re-derive every item's
    /// confidence and drop the stale-and-thin ones.
    ///
    /// The clock is the `now` argument — this method never reads a wall clock,
    /// so a sweep is fully reproducible from its inputs.
    ///
    /// DIVERGENCE (disclosed, accepted): upstream re-derives each item's
    /// confidence twice per pass — `lazyMigrateAlpha` ends with
    /// `syncConfidence`, and `decayItems` then calls it again to detect
    /// `hasChanges`. Upstream's `syncConfidence` was idempotent, so the second
    /// call was a no-op; the PRD's EMA lock (R1/R3) makes it a real second
    /// blend, and the decay factor is applied again. The structure is kept
    /// byte-faithful to upstream because R4 pins the decay semantics, and the
    /// doubled derivation is what the pinned fixtures assert.
    pub fn decay_in_memory(&self, data: &ProfileData, now: i64) -> DecayOutcome {
        let stale_days = f64::from(self.config.stale_days);
        let min_evidence = self.config.min_evidence_for_retention as usize;

        let preferences = decay_items(&data.preferences, now, stale_days, min_evidence);
        let patterns = decay_items(&data.patterns, now, stale_days, min_evidence);
        let workflows = decay_items(&data.workflows, now, stale_days, min_evidence);

        DecayOutcome {
            data: ProfileData {
                preferences: preferences.items,
                patterns: patterns.items,
                workflows: workflows.items,
                learning_paths: data.learning_paths.clone(),
            },
            // Upstream's OR of the three categories.
            has_changes: preferences.has_changes || patterns.has_changes || workflows.has_changes,
        }
    }

    /// The persisted daily sweep (R5): `decay_in_memory` over every active
    /// profile, written back through the existing optimistic-version
    /// `update_profile` when anything changed.
    ///
    /// The sweep stamp is recorded **only after** the whole loop completes, so
    /// a mid-sweep fault re-attempts on the next tick instead of silently
    /// skipping a day. A lost optimistic-version race (`update_profile`
    /// returning `false`) is counted, logged and left for the next sweep —
    /// never retried inline and never forced.
    ///
    /// Nothing in this wave calls this on a schedule — see R13 (the host
    /// lifecycle in `mem-bridge`/the composition root owns the daily tick).
    pub async fn sweep_decay_daily(&self, now: i64) -> Result<DecaySweepReport, ProfileError> {
        let due = {
            // last_decay_sweep gates the persisted decay side effect: a poisoned gate must refuse (ADR-0013 D-2 D2-c)
            let last = self
                .last_decay_sweep
                .lock()
                .map_err(|_| ProfileError::LockPoisoned {
                    lock: "last_decay_sweep",
                    operation: "sweep_decay_daily",
                })?;
            crate::decay::sweep_is_due(*last, now, crate::decay::SWEEP_INTERVAL_MILLIS)
        };
        if !due {
            return Ok(DecaySweepReport {
                profiles_scanned: 0,
                profiles_updated: 0,
            });
        }

        let profiles = self.get_all_active_profiles().await?;
        let mut profiles_scanned = 0usize;
        let mut profiles_updated = 0usize;

        for profile in profiles {
            profiles_scanned += 1;
            // A parse fault propagates: a corrupt stored profile is never
            // silently skipped (it would hide a storage defect forever).
            let data: ProfileData = serde_json::from_str(&profile.profile_data)?;
            let outcome = self.decay_in_memory(&data, now);
            if !outcome.has_changes {
                continue;
            }
            let written = self
                .update_profile(
                    &profile.id,
                    &outcome.data,
                    0,
                    "Decay sweep: confidence decayed",
                )
                .await?;
            if written {
                profiles_updated += 1;
            } else {
                tracing::debug!(
                    profile_id = %profile.id,
                    "decay sweep: lost the optimistic-version race, retrying next tick"
                );
            }
        }

        // the same stamp, written after the loop: the write is the other half of the gate (ADR-0013 D-2 D2-c)
        *self
            .last_decay_sweep
            .lock()
            .map_err(|_| ProfileError::LockPoisoned {
                lock: "last_decay_sweep",
                operation: "sweep_decay_daily",
            })? = Some(now);

        Ok(DecaySweepReport {
            profiles_scanned,
            profiles_updated,
        })
    }
}

/// Upstream `loadColdBuffersFailSafe`: record the failure, return an empty map.
fn load_cold_state_fail_safe(cold_buffer_path: &Path) -> ColdState {
    match crate::cold_buffer::load_snapshot(cold_buffer_path) {
        Ok(buffers) => ColdState {
            buffers,
            load_error: None,
        },
        Err(error) => ColdState {
            buffers: crate::cold_buffer::Snapshot::new(),
            load_error: Some(error.to_string()),
        },
    }
}

fn row_to_profile(row: &mem_store::db::Row) -> Profile {
    Profile {
        id: row.text("id").unwrap_or_default(),
        user_id: row.text("user_id").unwrap_or_default(),
        display_name: row.text("display_name").unwrap_or_default(),
        user_name: row.text("user_name").unwrap_or_default(),
        user_email: row.text("user_email").unwrap_or_default(),
        profile_data: row.text("profile_data").unwrap_or_default(),
        version: row.i64("version").unwrap_or(0),
        created_at: row.i64("created_at").unwrap_or(0),
        last_analyzed_at: row.i64("last_analyzed_at").unwrap_or(0),
        total_prompts_analyzed: row.i64("total_prompts_analyzed").unwrap_or(0),
        is_active: row.i64("is_active") == Some(1),
    }
}

fn row_to_changelog(row: &mem_store::db::Row) -> ProfileChangelog {
    ProfileChangelog {
        id: row.text("id").unwrap_or_default(),
        profile_id: row.text("profile_id").unwrap_or_default(),
        version: row.i64("version").unwrap_or(0),
        change_type: row.text("change_type").unwrap_or_default(),
        change_summary: row.text("change_summary").unwrap_or_default(),
        profile_data_snapshot: row.text("profile_data_snapshot").unwrap_or_default(),
        created_at: row.i64("created_at").unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Merge (upstream `mergeProfileData` / `mergeItems`)
// ---------------------------------------------------------------------------

impl ProfileStore {
    /// Upstream `mergeProfileData(existing, updates, embedService?, profileId?)`.
    ///
    /// The explicit (non-embedding) path is what the pinned `profile-write`
    /// cases exercise: a duplicate `(category, description)` merges into the
    /// existing entry, anything else is appended. Non-explicit observations
    /// seen while the embedding boundary is cold are buffered instead, and a
    /// warm merge drains that profile's bucket first.
    ///
    /// Cross-process safety: every round starts from the freshest complete
    /// on-disk snapshot (mtime comparison cannot be trusted — same-mtime
    /// writes and clock rollback both hide peer updates), and the loaded map
    /// is fixed for the whole round, so pending unsaved increments are never
    /// dropped by a mid-round reference swap.
    pub async fn merge_profile_data(
        &self,
        existing: &ProfileData,
        updates: &ProfileUpdates,
        embed: Option<&dyn EmbedBoundary>,
        profile_id: Option<&str>,
    ) -> Result<ProfileData, ProfileError> {
        self.begin_cold_buffer_round()?;

        let mut merged = ProfileData {
            preferences: existing.preferences.clone(),
            patterns: existing.patterns.clone(),
            workflows: existing.workflows.clone(),
            learning_paths: existing.learning_paths.clone(),
        };

        if let Some(incoming) = &updates.preferences {
            merged.preferences = self
                .merge_items(
                    merged.preferences,
                    incoming.clone(),
                    ItemType::Preference,
                    embed,
                    profile_id,
                )
                .await?;
        }
        if let Some(incoming) = &updates.patterns {
            merged.patterns = self
                .merge_items(
                    merged.patterns,
                    incoming.clone(),
                    ItemType::Pattern,
                    embed,
                    profile_id,
                )
                .await?;
        }
        if let Some(incoming) = &updates.workflows {
            merged.workflows = self
                .merge_items(
                    merged.workflows,
                    incoming.clone(),
                    ItemType::Workflow,
                    embed,
                    profile_id,
                )
                .await?;
        }

        // Upstream runs `detectConflicts` / `deduplicateItems` here for every
        // category with ≥ 2 items. Both are LLM-backed, so with no provider
        // configured they early-return — the W3.1 tests never configure one,
        // and W3.3 owns the real wiring. Keeping the seam inert preserves the
        // no-provider behaviour exactly.

        Ok(merged)
    }

    /// Upstream `mergeItems(existing, incoming, itemType, embedService?, profileId?)`.
    async fn merge_items(
        &self,
        mut existing: Vec<ProfileItem>,
        mut incoming: Vec<ProfileItem>,
        item_type: ItemType,
        embed: Option<&dyn EmbedBoundary>,
        profile_id: Option<&str>,
    ) -> Result<Vec<ProfileItem>, ProfileError> {
        let warmed_up = embed.is_some_and(|boundary| boundary.is_warmed_up());
        if !warmed_up {
            tracing::debug!(
                profile_id = profile_id.unwrap_or(COLD_BUFFER_DEFAULT_KEY),
                "profile embedding skipped: model not warmed up"
            );
        }

        // Warm drain: the buffered observations for THIS profile (never a
        // peer's) are spliced in front of the incoming batch and the drain is
        // persisted immediately, exactly like upstream.
        if warmed_up {
            let buffered = self.drain_cold_bucket(profile_id, item_type);
            if !buffered.is_empty() {
                self.save_cold_buffers()?;
                tracing::info!(
                    r#type = ?item_type,
                    profile_id = profile_id.unwrap_or(COLD_BUFFER_DEFAULT_KEY),
                    buffer_size = buffered.len(),
                    "profile cold start: draining buffer"
                );
                let mut combined = buffered;
                combined.extend(incoming);
                incoming = combined;
            }
        }

        let now = now_millis();
        for item in existing.iter_mut() {
            lazy_migrate_alpha(item, now);
        }

        let min_desc_len = self.config.embedding_min_description_length as usize;
        let mut appended: Vec<ProfileItem> = Vec::new();

        // Upstream pre-seeds a centroid on existing items whose description is
        // long enough (the "profile centroid migrated" pass).
        if warmed_up && let Some(boundary) = embed {
            for item in existing.iter_mut() {
                if item.description.len() >= min_desc_len
                    && item.centroid.is_none()
                    && let Ok(vector) = boundary.embed(&normalize_description(&item.description))
                {
                    item.centroid = Some(vector.clone());
                    item.anchor = Some(vector);
                }
            }
        }

        for new_item in incoming.drain(..) {
            let exact = existing.iter().position(|candidate| {
                candidate.category == new_item.category
                    && candidate.description == new_item.description
            });

            if let Some(index) = exact {
                let merged = merge_confirmed_match(&existing[index], &new_item, true, now);
                existing[index] = merged;
                continue;
            }

            // Upstream's deterministic strong-band match: a long-enough
            // description is compared against every existing centroid.
            let mut matched = false;
            if warmed_up
                && new_item.description.len() >= min_desc_len
                && let Some(boundary) = embed
                && let Ok(vector) = boundary.embed(&normalize_description(&new_item.description))
            {
                {
                    {
                        let mut top1: Option<(usize, f64, bool)> = None;
                        for (index, candidate) in existing.iter().enumerate() {
                            let Some(centroid) = candidate.centroid.as_ref() else {
                                continue;
                            };
                            if candidate.description.len() < min_desc_len {
                                continue;
                            }
                            let score = mem_core::utils::math::cosine_similarity(&vector, centroid);
                            let same_cat = candidate.category == new_item.category;
                            let strong = if same_cat {
                                self.config.embedding_threshold_same_cat
                            } else {
                                self.config.embedding_threshold_cross_cat
                            };
                            if score >= strong
                                && top1.map(|(_, best, _)| score > best).unwrap_or(true)
                            {
                                top1 = Some((index, score, same_cat));
                            }
                        }
                        if let Some((index, _score, same_cat)) = top1 {
                            let existing_item = existing[index].clone();
                            let centroid = existing_item.centroid.clone().unwrap_or_default();
                            let anchor = existing_item.anchor.clone().unwrap_or_default();
                            let mut combined =
                                merge_confirmed_match(&existing_item, &new_item, same_cat, now);
                            let weight = 0.85_f32;
                            let complement = 0.15_f32;
                            combined.centroid = Some(
                                centroid
                                    .iter()
                                    .enumerate()
                                    .map(|(position, value)| {
                                        weight * value
                                            + complement
                                                * vector.get(position).copied().unwrap_or(0.0)
                                    })
                                    .collect(),
                            );
                            combined.anchor = Some(anchor);
                            combined.weak_hit_count = Some(0);
                            combined.last_weak_hit_at = None;
                            existing[index] = combined;
                            matched = true;
                        }
                    }
                }
            }
            if matched {
                continue;
            }

            // Cold boundary: a non-explicit observation is buffered for a
            // later warm merge rather than merged into a profile the model
            // cannot yet compare it against. An explicit write (the `explicit`
            // category or `manual-write` evidence) always lands immediately.
            let is_explicit = new_item.category.as_deref() == Some("explicit")
                || new_item
                    .evidence
                    .as_ref()
                    .is_some_and(|evidence| evidence.iter().any(|entry| entry == "manual-write"));
            if !warmed_up && new_item.description.len() >= min_desc_len && !is_explicit {
                self.buffer_cold_item(profile_id, item_type, &new_item)?;
                tracing::debug!(
                    r#type = ?item_type,
                    profile_id = profile_id.unwrap_or(COLD_BUFFER_DEFAULT_KEY),
                    "profile cold start: buffered"
                );
                continue;
            }

            let mut centroid = None;
            let mut anchor = None;
            if warmed_up
                && new_item.description.len() >= min_desc_len
                && let Some(boundary) = embed
                && let Ok(vector) = boundary.embed(&normalize_description(&new_item.description))
            {
                centroid = Some(vector.clone());
                anchor = Some(vector);
            }
            appended.push(init_item(&new_item, centroid, anchor, now));
        }

        existing.extend(appended);
        Ok(existing)
    }
}

/// Upstream `normalizeDescription` (punctuation/whitespace folding before an
/// embedding comparison).
pub(crate) fn normalize_description(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for character in text.trim().chars() {
        if matches!(
            character,
            '，' | ',' | '。' | '！' | '？' | '；' | ';' | '：' | ':' | '、' | '\n' | '\r' | '.'
        ) {
            folded.push(' ');
        } else {
            folded.push(character);
        }
    }
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Upstream `lazyMigrateAlpha`: seeds `alpha`/`beta` from a legacy
/// `confidence`, then re-derives `confidence`.
pub(crate) fn lazy_migrate_alpha(item: &mut ProfileItem, now: i64) {
    let needs_migration =
        item.alpha.is_none() || (item.alpha == Some(0.5) && item.beta == Some(1.5));
    if needs_migration {
        let confidence = item.confidence.unwrap_or(1.0).clamp(0.0, 1.0);
        item.alpha = Some(confidence / (1.0 - confidence + 0.01));
        item.beta = Some(1.0);
    }
    item.weak_alpha.get_or_insert(1.0);
    item.weak_beta.get_or_insert(1.0);
    item.last_match_time
        .get_or_insert(item.last_seen.unwrap_or(now));
    item.first_seen.get_or_insert(item.last_seen.unwrap_or(now));
    if item.pending_validation.is_none() {
        item.pending_validation = Some(false);
    }
    sync_confidence(item, now);
}

/// Upstream `syncConfidence`, with the PRD's EMA lock (R1/R3): the stored
/// confidence is the *prior* and the Beta estimator is the *observation*.
///
/// `alpha`/`beta` bookkeeping is untouched (W3.1 semantics); only the
/// derivation of `item.confidence` changes, so a repeated call without an
/// interval gate drifts toward the estimator — which is exactly why the read
/// path is paired with the gated daily sweep rather than run on every tick.
pub(crate) fn sync_confidence(item: &mut ProfileItem, now: i64) {
    let alpha = item.alpha.unwrap_or(1.0);
    let beta = item.beta.unwrap_or(1.0);
    let beta_mean = alpha / (alpha + beta);
    let observation = alpha / (alpha + beta);
    let prior = item.confidence.unwrap_or(beta_mean);
    let blended = crate::decay::ema_update(prior, observation, crate::decay::CONFIDENCE_EMA_ALPHA);
    item.confidence = Some(blended * confidence_decay_factor(item, now));
}

/// The upstream time-decay factor: the pure kernel's `timeFactor` capped by the
/// 30-day trend multiplier (the two had drifted apart; the kernel is now the
/// single source of truth for the half-life math).
pub(crate) fn confidence_decay_factor(item: &ProfileItem, now: i64) -> f64 {
    let frequency = item.frequency_or_one() as f64;
    let age = (now - item.last_seen.unwrap_or(now)) as f64;
    let match_time = item.last_match_time.or(item.last_seen).unwrap_or(now);
    let match_age_days = (now - match_time) as f64 / crate::decay::MILLIS_PER_DAY;
    let trend_multiplier = 0.8 + 0.2 * (-match_age_days / 30.0).exp();
    let decay = crate::decay::decay_factor(age, crate::decay::HALF_LIFE_DAYS, frequency);
    decay.min(trend_multiplier)
}

/// Upstream `mergeConfirmedMatch`.
fn merge_confirmed_match(
    existing: &ProfileItem,
    new_item: &ProfileItem,
    same_cat: bool,
    now: i64,
) -> ProfileItem {
    let mut merged = existing.clone();
    lazy_migrate_alpha(&mut merged, now);

    let mut evidence: Vec<String> = vec![new_item.description.clone()];
    evidence.extend(new_item.evidence_or_empty());
    evidence.extend(merged.evidence_or_empty());
    evidence.dedup();
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<String> = Vec::new();
    for entry in evidence {
        if seen.insert(entry.clone()) {
            deduped.push(entry);
        }
    }
    deduped.truncate(10);

    merged.alpha = Some(merged.alpha.unwrap_or(1.0) + if same_cat { 1.0 } else { 0.5 });
    merged.beta = Some(merged.beta.unwrap_or(1.0));
    if same_cat {
        merged.frequency = Some(merged.frequency_or_one() + 1);
    }
    merged.weak_alpha = Some(1.0);
    merged.weak_beta = Some(1.0);
    merged.evidence = Some(deduped);
    merged.last_seen = Some(now);
    merged.last_match_time = Some(now);
    if merged.steps.is_none() && new_item.steps.is_some() {
        merged.steps = new_item.steps.clone();
    }
    sync_confidence(&mut merged, now);
    merged
}

/// Upstream `initItem`: a brand-new observation. An evidenced item keeps its
/// supplied confidence and starts at `alpha = beta = 1`; an unevidenced one
/// starts weak (`alpha = 0.3`, `beta = 1.5`, `confidence = 0.2`).
fn init_item(
    new_item: &ProfileItem,
    centroid: Option<Vec<f32>>,
    anchor: Option<Vec<f32>>,
    now: i64,
) -> ProfileItem {
    let has_evidence = new_item
        .evidence
        .as_ref()
        .is_some_and(|evidence| !evidence.is_empty());
    let mut item = new_item.clone();
    item.alpha = Some(if has_evidence { 1.0 } else { 0.3 });
    item.beta = Some(if has_evidence { 1.0 } else { 1.5 });
    item.weak_alpha = Some(1.0);
    item.weak_beta = Some(1.0);
    item.confidence = Some(if has_evidence {
        new_item.confidence.unwrap_or(0.5)
    } else {
        0.2
    });
    item.frequency = Some(1);
    item.last_seen = Some(now);
    item.last_match_time = Some(now);
    item.first_seen = Some(now);
    item.evidence = Some(new_item.evidence_or_empty());
    item.weak_hit_count = Some(0);
    item.drift_below_count = Some(0);
    item.pending_validation = Some(true);
    item.centroid = centroid;
    item.anchor = anchor;
    sync_confidence(&mut item, now);
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[tokio::test]
    async fn test_sweep_decay_propagates_poisoned_stamp() {
        let storage_path = PathBuf::from("/nonexistent/opencode-mem-profile-test");
        let store_config = StoreConfig {
            storage_path: storage_path.clone(),
            embedding_dimensions: 2,
            embedding_model: "test-model".to_string(),
            max_vectors_per_shard: 1000,
            container_tag_prefix: "opencode".to_string(),
            database_encryption_enabled: false,
            database_encryption_cipher: "aes256gcm".to_string(),
            database_encryption_key: None,
            allow_unguarded_open: false,
        };
        let store = ProfileStore::new(storage_path, store_config, ProfileConfig::default());
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = store
                .last_decay_sweep
                .lock()
                .expect("fresh decay sweep lock");
            panic!("poison");
        }));
        assert!(
            store.last_decay_sweep.is_poisoned(),
            "the decay stamp must be poisoned"
        );

        let error = store
            .sweep_decay_daily(1_700_000_000_000)
            .await
            .expect_err("a poisoned decay stamp must refuse the sweep");
        assert!(matches!(
            error,
            ProfileError::LockPoisoned {
                lock: "last_decay_sweep",
                ..
            }
        ));
    }
}
