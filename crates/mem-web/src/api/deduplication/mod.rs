//! Memory-shard deduplication — upstream `deduplication-service.ts`
//! (`DeduplicationService.detectAndRemoveDuplicates`).
//!
//! **This is upstream's memory-shard deduplication, not `mem_core::dedup`.**
//! That other module is the profile-conflict grouping kernel (cosine band
//! `[0.65, 0.9)` over exact `(category, description)` pairs) and its own doc
//! forbids wiring it here. Nothing in this module imports it — in particular
//! not `mem_core::dedup::normalize_key`, whose trim/lowercase/collapse would
//! turn exact duplicate grouping into semantic merging.
//!
//! One run walks every shard (user shards, then project shards) and does two
//! things per shard:
//!
//! 1. **deletes** exact duplicates — rows sharing the byte-for-byte pair
//!    `(container_tag, content)`, newest wins, `id ASC` breaking a tie;
//! 2. **reports** near-duplicate groups — a greedy, non-transitive scan whose
//!    cosine similarity lies in `[threshold, 1.0)` within one container tag.
//!    This path deletes, merges and re-embeds nothing.
//!
//! The exact-delete half is never truncated **by the budget**. The
//! near-duplicate half runs on the blocking pool under one global
//! candidate-comparison budget ([`MAX_NEAR_SCAN_PAIRS`] by default), so it
//! cannot occupy the engine's single runtime thread, cannot outrun shutdown
//! (the scan checks a one-way cancel flag), and cannot hold a blocking thread
//! past the budget. A pass that exhausts the budget reports `truncated` in
//! [`NearDuplicateScanReport`] instead of refusing to run.
//!
//! A shard whose rows could not be read is counted in `unreadable_shards`,
//! forces `truncated`, and is never reported as an empty shard: both its near
//! scan and its exact-delete half go unexamined (the latter is a read that
//! never happened, not a budget truncation), and the pass continues with the
//! remaining shards because earlier shards' exact deletions are already
//! committed.
//!
//! [`DedupStore`] is the only boundary between the algorithm and libsql, so
//! the greedy scan and the run guard are provable with a fake while the real
//! glue stays in [`store`].

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mem_episodic::inbox::CaptureInbox;
use mem_episodic::store::EpisodicStore;
use mem_profile::decay::sweep_is_due;
use mem_profile::prompt::UserPromptManager;
use mem_store::shard_manager::ShardManager;

use super::cleanup_sweep::{load_stamp_fail_safe, save_stamp};
use super::ports::{
    CleanupReport, CleanupStatus, CleanupTick, DedupDuplicate, DedupMemoryRef, DedupReport,
    MaintenancePort, MigrationReport, MigrationRunReport, NearDuplicateGroup,
    NearDuplicateScanReport, PortError, TagMigrationProgressView, TagMigrationReport,
};

pub mod engine;
pub mod store;

/// Upstream's disabled-service business message.
pub const DISABLED_MESSAGE: &str = "Deduplication is disabled in config";
/// Upstream's concurrent-run business message.
pub const RUNNING_MESSAGE: &str = "Deduplication already running";

/// Hard ceiling on one pass's near-duplicate scan, in candidate comparisons.
///
/// **Derivation (issue 56 ruling, 2026-10-28), release profile.**
/// `c_iter_release = 658.9 ns` per candidate comparison at 768 lanes, the
/// median of three release runs at `n = 10,000` (49,995,000 comparisons per
/// run, spread 5.2 %), measured by `crates/mem-web/tests/dedup_scan_measure.rs`
/// and committed with its raw runs in
/// `docs/evidence/dedup-release-profile-2026-10-26/`. With `T_SCAN_MAX = 10 s`
/// re-affirmed for the release profile:
///
/// ```text
/// 10 s / c_iter                  = 15,176,810 pairs
/// floor to 2 significant figures = 15,000,000
/// min(max(250_000, 15,000,000), 1,250,000,000) = 15,000,000
/// 15,000,000 × 658.9 ns          = 9.8835 s
/// ```
///
/// The floor guards a broken measurement; the ceiling is one shard's whole pair
/// grid at `max_vectors_per_shard = 50_000` (1,249,975,000) rounded up so a
/// single shard at capacity is never truncated *by the ceiling itself*. The
/// ceiling is ~83x the value, so it does not bind; the cap does: a 50,000-row
/// shard is truncated, and a whole pass covers about
/// `sqrt(2 × 15,000,000) ≈ 5,477` survivors in walk order (11.0 % of a 50k
/// shard) against the previous 740,000's `sqrt(2 × 740,000) ≈ 1,217` (2.4 %).
/// The 50,000-row rung was not measured: `t50_release ≈ 823.6 s` is INFERRED
/// from `c_iter_release`.
///
/// **The debug figure is retained as the conservative input (issue 54b).**
/// `c_iter = 13,487.9 ns` per candidate comparison at 768 lanes, measured in
/// the debug profile (`cargo test` without `--release`) at `n = 10,000`
/// (49,995,000 comparisons in 674.326 s). 54b's artifact is gone; its numbers
/// are transcribed in `.scratch/rust-port/issues/54b-dedup-scan-bounding.md`.
/// That figure derives 740,000, the cap that shipped before this ruling, and
/// spends 0.49 s at the release figure. The measured debug/release factor is
/// k = 13,487.9 / 658.9 = 20.470.
///
/// **Tail, recorded not smoothed.** At the slowest measured release run
/// (677.1 ns) the cap spends 10.16 s, 1.6 % above the pre-registered budget;
/// the ruling is stated against the recorded median, as the rule specifies.
///
/// It is a GLOBAL per-pass budget, so a multi-shard pass at capacity can reach
/// it and truncate — that is the case it exists for, and shard count is
/// unbounded (no `max_shards` key). If the ceiling ever binds, the value is
/// non-binding per shard, not absent: only a validity-gate measurement (a
/// harness that is not measuring cosines) gets there, and this one is ~83x
/// clear of it.
///
/// The rule is pinned by the derivation gate in
/// `crates/mem-web/tests/dedup_scan_measure.rs`, which re-implements it rather
/// than reading it back: `derive_cap(658.9e-9)` must equal this constant.
pub const MAX_NEAR_SCAN_PAIRS: u64 = 15_000_000;

/// One shard the pass walks; opaque to the engine (it is handed back
/// verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupShard {
    pub shard_id: i64,
    pub db_path: PathBuf,
}

/// One `memories` row as the pass sees it.
///
/// Deliberately carries no `is_pinned`: upstream does not exempt pinned rows,
/// and this type makes such an exemption impossible without a deliberate type
/// change.
#[derive(Debug, Clone, PartialEq)]
pub struct DedupMemory {
    pub id: String,
    pub content: String,
    pub container_tag: String,
    pub created_at: i64,
    /// `None` ⇔ upstream `parseExtractedVector(...) === null`.
    pub vector: Option<Vec<f32>>,
}

/// The store seam of one deduplication pass.
#[async_trait]
pub trait DedupStore: Send + Sync {
    /// User-scope shards, then project-scope shards (upstream's order), each
    /// scope in `shard_index ASC`.
    async fn shards(&self) -> Result<Vec<DedupShard>, PortError>;

    /// Every row of one shard, ordered `created_at DESC, id ASC`; an open
    /// failure or a shard fault is returned as an `Err`, counted as unreadable
    /// by the pass.
    async fn memories(&self, shard: &DedupShard) -> Result<Vec<DedupMemory>, PortError>;

    /// Delete one row, then bring the shard's vector accounting in line with
    /// the present rows.
    ///
    /// W3 / ADR-0018: the delete and a statement-measured ABSOLUTE
    /// `COUNT(*)` land in one transaction (`delete_memories`), and the
    /// registry adopts that value. No `±1` survives.
    async fn delete(&self, shard: &DedupShard, memory_id: &str) -> Result<(), PortError>;
}

/// One deduplication pass over a [`DedupStore`].
pub struct DedupService {
    store: Arc<dyn DedupStore>,
    enabled: bool,
    threshold: f64,
    running: Arc<AtomicUsize>,
    /// Candidate comparisons the whole pass may spend on the near scan.
    near_pair_budget: u64,
    /// One-way shutdown signal, shared with the blocking scan.
    cancel: Arc<AtomicBool>,
}

impl DedupService {
    /// Builds the service. `enabled` and `threshold` are the resolved
    /// `deduplicationEnabled` / `deduplicationSimilarityThreshold` config
    /// values; this module never loads config itself.
    pub fn new(store: Arc<dyn DedupStore>, enabled: bool, threshold: f64) -> Self {
        Self {
            store,
            enabled,
            threshold,
            running: Arc::new(AtomicUsize::new(0)),
            near_pair_budget: MAX_NEAR_SCAN_PAIRS,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Caps one pass's near-duplicate scan at `pairs` candidate comparisons.
    /// The budget is global across shards, in walk order, and never
    /// re-granted; the exact-delete half is unaffected.
    pub fn with_near_pair_budget(mut self, pairs: u64) -> Self {
        self.near_pair_budget = pairs;
        self
    }

    /// Shares a shutdown flag with the near scan. When it is set the scan
    /// returns at its next check and the pass answers an error rather than a
    /// partial report.
    pub fn with_cancellation(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = cancel;
        self
    }

    /// Upstream `detectAndRemoveDuplicates`.
    ///
    /// The disabled check comes first, so a disabled service never takes the
    /// run flag and the two business errors can never both be observable on
    /// one request.
    pub async fn run(&self) -> Result<DedupReport, PortError> {
        if !self.enabled {
            return Err(PortError::Invalid(DISABLED_MESSAGE.to_string()));
        }
        let _guard = RunGuard::acquire(&self.running)
            .ok_or_else(|| PortError::Invalid(RUNNING_MESSAGE.to_string()))?;

        let shards = self.store.shards().await?;
        let mut exact_duplicates_deleted = 0_u64;
        let mut near_duplicate_groups: Vec<NearDuplicateGroup> = Vec::new();
        // One budget for the whole pass, consumed in walk order (user shards
        // then project shards, `shard_index ASC`) and never re-granted: a
        // per-shard budget with an unbounded shard count is not a bound.
        let mut pairs_remaining = self.near_pair_budget;
        let mut pairs_scanned = 0_u64;
        let mut unreadable_shards = 0_u64;
        let mut truncated = false;

        for shard in &shards {
            let rows = match self.store.memories(shard).await {
                Ok(rows) => Arc::new(rows),
                Err(error) => {
                    // The arm is the loop's first statement: a shard whose
                    // rows could not be read contributes nothing to the
                    // deletions, the groups or the pair count, and its
                    // coverage claim is withdrawn rather than answered as
                    // "empty". The pass continues because earlier shards'
                    // exact deletions are already committed — aborting would
                    // answer an error over a committed write.
                    tracing::error!(
                        shard_id = shard.shard_id,
                        error = %error,
                        "deduplication: shard read failed"
                    );
                    unreadable_shards = unreadable_shards.saturating_add(1);
                    truncated = true;
                    continue;
                }
            };
            let plan = engine::exact_plan(&rows);

            // The exact-delete half is never truncated: it runs for every
            // shard even when the near budget is already spent.
            for index in &plan.deletions {
                let Some(row) = rows.get(*index) else {
                    continue;
                };
                match self.store.delete(shard, &row.id).await {
                    Ok(()) => exact_duplicates_deleted += 1,
                    Err(error) => tracing::error!(
                        shard_id = shard.shard_id,
                        memory_id = %row.id,
                        error = %error,
                        "deduplication: delete failed"
                    ),
                }
            }

            if plan.survivors.len() <= 1 {
                // No scan work exists for this shard: nothing was left undone.
                continue;
            }
            if pairs_remaining == 0 {
                // The shard's exact duplicates were still deleted above, and
                // there is scan work left undone.
                truncated = true;
                continue;
            }

            // The near scan is pure CPU over the rows just read, so it runs on
            // the blocking pool: one cosine per ordered survivor pair would
            // otherwise occupy the engine's single runtime thread — and the
            // HTTP timeout task with it. Only this pass moves off the runtime;
            // the store legs above and below stay awaited here.
            let lease = ScanLease::acquire(&self.running);
            let scan_rows = Arc::clone(&rows);
            let survivors = plan.survivors;
            let threshold = self.threshold;
            let limits = engine::NearScanLimits {
                max_pairs: pairs_remaining,
                cancel: Arc::clone(&self.cancel),
            };
            let scan = tokio::task::spawn_blocking(move || {
                // The lease travels with the work: the run guard stays taken
                // for as long as the detached scan runs, even if the request
                // future that queued it is dropped.
                let _lease = lease;
                engine::near_groups(&scan_rows, &survivors, threshold, &limits)
            })
            .await
            .map_err(|error| {
                PortError::Store(format!("deduplication scan task failed: {error}"))
            })?;

            if scan.cancelled {
                // D-9 keeps the detail out of the response; the process is
                // shutting down, so a report nobody reads is worse than an
                // honest error — and the guard and lease still unwind.
                return Err(PortError::Store("deduplication scan cancelled".to_string()));
            }
            pairs_remaining = pairs_remaining.saturating_sub(scan.pairs_scanned);
            pairs_scanned = pairs_scanned.saturating_add(scan.pairs_scanned);
            truncated |= scan.truncated;

            for group in scan.groups {
                let Some(representative) = rows.get(group.representative) else {
                    continue;
                };
                let duplicates: Vec<DedupDuplicate> = group
                    .duplicates
                    .iter()
                    .filter_map(|(index, similarity)| {
                        rows.get(*index).map(|row| DedupDuplicate {
                            id: row.id.clone(),
                            content: row.content.clone(),
                            similarity: *similarity,
                        })
                    })
                    .collect();
                near_duplicate_groups.push(NearDuplicateGroup {
                    representative: DedupMemoryRef {
                        id: representative.id.clone(),
                        content: representative.content.clone(),
                        container_tag: representative.container_tag.clone(),
                        created_at: representative.created_at,
                    },
                    duplicates,
                });
            }
        }

        Ok(DedupReport {
            exact_duplicates_deleted,
            near_duplicate_groups,
            near_duplicate_scan: NearDuplicateScanReport {
                pairs_scanned,
                pair_budget: self.near_pair_budget,
                truncated,
                unreadable_shards,
            },
        })
    }
}

/// Held for one async run; `Drop` releases the count on every exit path,
/// including `?` and a panic.
///
/// The counter is a **count**, not a boolean, because a detached scan holds a
/// second count of its own ([`ScanLease`]): a pass is "running" iff the count
/// is non-zero, and the last holder to release frees admission. A dropped
/// request future therefore cannot open a second pass while the scan it queued
/// still burns a blocking thread.
struct RunGuard {
    running: Arc<AtomicUsize>,
}

impl RunGuard {
    fn acquire(running: &Arc<AtomicUsize>) -> Option<Self> {
        // The RMW is what provides mutual exclusion (nothing is *published*
        // through this counter, so `Relaxed` would be sound; `AcqRel`/`Acquire`
        // is the conservative house choice, not a data-publication need).
        running
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count == 0).then_some(1)
            })
            .ok()
            .map(|_| Self {
                running: Arc::clone(running),
            })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The detached near scan's claim on the run counter.
///
/// Acquired in `run()` **before** `spawn_blocking` and moved into the closure,
/// with no await between the increment and the spawn. Constructing it inside
/// the closure instead is forbidden because of the **admission window**: a
/// request future dropped after `spawn_blocking` has returned but before the
/// closure's first instruction runs would release the async guard, leaving the
/// count at zero while a scan is already queued — and an overlapping pass would
/// be admitted against a scan that is about to start.
struct ScanLease {
    running: Arc<AtomicUsize>,
}

impl ScanLease {
    fn acquire(running: &Arc<AtomicUsize>) -> Self {
        running.fetch_add(1, Ordering::AcqRel);
        Self {
            running: Arc::clone(running),
        }
    }
}

impl Drop for ScanLease {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::AcqRel);
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// `MaintenancePort` with a real `deduplicate()` and `cleanup()`; every other
/// method keeps the documented `Unavailable` semantics (migrations are 37's to wire).
pub struct ProductionDeduplication {
    service: DedupService,
    shards: Option<ShardManager>,
    prompts: Option<Arc<UserPromptManager>>,
    retention_days: u32,
    cleanup_running: Arc<AtomicBool>,
    /// `None` keeps issue 39's shape: the manual endpoint works, the automatic
    /// tick answers `Disabled` (issue 41).
    cleanup: Option<CleanupSweepConfig>,
    /// The in-memory gate, seeded from the sidecar at construction: memory is
    /// the gate, the sidecar the durability channel (D-4).
    cleanup_last_run: Mutex<Option<i64>>,
    /// Keeps the future-stamp warning to one line per process (C-2).
    stamp_future_warned: AtomicBool,
    /// Issue 32: the episodic store whose dead staging rows are COUNTED (never
    /// purged — ADR-0033). `None` = not wired, so the report answers `null`.
    episodic: Option<Arc<EpisodicStore>>,
}

/// The automatic cleanup surface the composition root hands the port (issue 41).
///
/// `enabled` is checked by `cleanup_tick` alone — `cleanup()` never reads it, so
/// `POST /api/cleanup` keeps working on a host that opted out of the tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupSweepConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub sweep_path: PathBuf,
}

/// Why a sweep did not produce a report (issue 41).
///
/// `Busy` is typed so the tick never string-matches the manual endpoint's
/// message (D-5); `Failed` carries the store error unchanged, so `cleanup()`'s
/// wire answers stay byte-identical to issue 39's.
enum CleanupRunError {
    Busy,
    Unavailable,
    Failed(PortError),
}

struct CleanupRunGuard<'a> {
    running: &'a AtomicBool,
}

impl<'a> Drop for CleanupRunGuard<'a> {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

impl ProductionDeduplication {
    /// Wraps a store-backed service and retention sweep dependencies.
    pub fn new(
        store: Arc<dyn DedupStore>,
        shards: ShardManager,
        prompts: Arc<UserPromptManager>,
        retention_days: u32,
        enabled: bool,
        threshold: f64,
    ) -> Self {
        Self {
            service: DedupService::new(store, enabled, threshold),
            shards: Some(shards),
            prompts: Some(prompts),
            retention_days,
            cleanup_running: Arc::new(AtomicBool::new(false)),
            cleanup: None,
            cleanup_last_run: Mutex::new(None),
            stamp_future_warned: AtomicBool::new(false),
            episodic: None,
        }
    }

    /// Helper for test fixtures that only exercise deduplication.
    pub fn for_dedup_only(store: Arc<dyn DedupStore>, enabled: bool, threshold: f64) -> Self {
        Self {
            service: DedupService::new(store, enabled, threshold),
            shards: None,
            prompts: None,
            retention_days: 30,
            cleanup_running: Arc::new(AtomicBool::new(false)),
            cleanup: None,
            cleanup_last_run: Mutex::new(None),
            stamp_future_warned: AtomicBool::new(false),
            episodic: None,
        }
    }

    /// Shares the composition root's shutdown flag with the near scan.
    pub fn with_cancellation(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.service = self.service.with_cancellation(cancel);
        self
    }

    /// Arms the capture-triggered cleanup tick (issue 41), seeding the in-memory
    /// gate from the sidecar so a restart inside the interval is not due.
    pub fn with_cleanup(mut self, cleanup: CleanupSweepConfig) -> Self {
        self.cleanup_last_run = Mutex::new(load_stamp_fail_safe(&cleanup.sweep_path));
        self.cleanup = Some(cleanup);
        self
    }

    /// Issue 32: arms the dead-staging-row count. The store is only ever read
    /// (`CaptureInbox::count_dead`); no purge path exists on this schema.
    pub fn with_episodic(mut self, episodic: Option<Arc<EpisodicStore>>) -> Self {
        self.episodic = episodic;
        self
    }

    /// The one serializer for the manual endpoint and the tick (D-5).
    fn try_acquire_cleanup(&self) -> Result<CleanupRunGuard<'_>, CleanupRunError> {
        if self.shards.is_none() || self.prompts.is_none() {
            return Err(CleanupRunError::Unavailable);
        }
        if self
            .cleanup_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(CleanupRunError::Busy);
        }
        Ok(CleanupRunGuard {
            running: &self.cleanup_running,
        })
    }

    fn cleanup_gate(&self) -> Result<Option<i64>, PortError> {
        self.cleanup_last_run
            .lock()
            .map(|gate| *gate)
            .map_err(|_| PortError::Store("cleanup gate lock poisoned".to_string()))
    }

    /// One retention sweep: issue 39's `cleanup()` body, behind the typed guard
    /// so the manual endpoint and the tick share one serializer (D-5).
    async fn run_sweep(&self) -> Result<CleanupReport, CleanupRunError> {
        let _guard = self.try_acquire_cleanup()?;
        tokio::task::yield_now().await;
        let (Some(shards), Some(prompts)) = (&self.shards, &self.prompts) else {
            return Err(CleanupRunError::Unavailable);
        };

        let now = now_millis();
        let retention_ms = i64::from(self.retention_days).saturating_mul(86_400_000);
        let cutoff = now.saturating_sub(retention_ms);
        let two_retention_ms = retention_ms.saturating_mul(2);

        // Phase 1: Read expired prompt facts
        let facts = prompts
            .expired_prompt_facts(cutoff)
            .await
            .map_err(|e| CleanupRunError::Failed(PortError::Store(e.to_string())))?;

        let mut grace_ids = HashSet::new();
        for fact in facts {
            if fact.created_at.saturating_add(two_retention_ms) > cutoff
                && let Some(linked_id) = fact.linked_memory_id
            {
                grace_ids.insert(linked_id);
            }
        }

        // Phase 2: Sweep per shard
        let sweep_outcome = shards
            .run_retention_sweep(cutoff, retention_ms, two_retention_ms, &grace_ids)
            .await
            .map_err(|e| CleanupRunError::Failed(PortError::Store(e.message())))?;

        // Phase 3: Erase + delete prompts + clear dangling links
        let prompts_deleted = prompts
            .delete_expired_prompts(cutoff)
            .await
            .map_err(|e| CleanupRunError::Failed(PortError::Store(e.to_string())))?;

        let _links_cleared = prompts
            .clear_links(&sweep_outcome.deleted_ids)
            .await
            .map_err(|e| CleanupRunError::Failed(PortError::Store(e.to_string())))?;

        // Phase 4: Best-effort VACUUM
        if let Err(e) = prompts.vacuum().await {
            tracing::warn!(error = %e, "cleanup: vacuum skipped (DB busy)");
        }

        // Issue 32: a read in the report build, not a phase. An absent store is
        // `None` (unknown); a count FAILURE is a run failure, never `None` —
        // folding it would report "no store" for a store that is merely ill.
        let inbox_dead_retained = match &self.episodic {
            Some(episodic) => Some(
                CaptureInbox::new(Arc::clone(episodic))
                    .count_dead()
                    .await
                    .map_err(|error| {
                        CleanupRunError::Failed(PortError::Store(error.to_string()))
                    })?,
            ),
            None => None,
        };

        Ok(CleanupReport {
            deleted_count: sweep_outcome.deleted_count,
            user_count: sweep_outcome.user_count,
            project_count: sweep_outcome.project_count,
            prompts_deleted,
            protected_memories_skipped: sweep_outcome.protected_memories_skipped,
            inbox_dead_retained,
        })
    }
}

#[async_trait]
impl MaintenancePort for ProductionDeduplication {
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
        _tags: std::collections::HashMap<String, Result<Vec<String>, String>>,
    ) -> Result<mem_core::tags::migration::BatchOutcome, PortError> {
        Err(PortError::Unavailable)
    }

    async fn deduplicate(&self) -> Result<DedupReport, PortError> {
        self.service.run().await
    }

    async fn cleanup(&self) -> Result<CleanupReport, PortError> {
        self.run_sweep().await.map_err(|error| match error {
            CleanupRunError::Busy => PortError::Invalid("cleanup is already running".to_string()),
            CleanupRunError::Unavailable => PortError::Unavailable,
            CleanupRunError::Failed(error) => error,
        })
    }

    async fn cleanup_status(&self) -> Result<CleanupStatus, PortError> {
        let Some(config) = &self.cleanup else {
            return Err(PortError::Unavailable);
        };
        Ok(CleanupStatus {
            enabled: config.enabled,
            interval_seconds: config.interval_seconds,
            last_run_ms: self.cleanup_gate()?,
            running: self.cleanup_running.load(Ordering::Acquire),
        })
    }

    async fn cleanup_tick(&self, now: i64) -> Result<CleanupTick, PortError> {
        let Some(config) = &self.cleanup else {
            return Ok(CleanupTick::Disabled);
        };
        // The switch is read HERE and nowhere else: `cleanup()` never consults
        // it, so `POST /api/cleanup` stays live on a host that opted out.
        if !config.enabled {
            return Ok(CleanupTick::Disabled);
        }

        let interval_millis = i64::try_from(config.interval_seconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        let due = match self.cleanup_gate()? {
            // A stamp in the future is corrupt evidence, and corrupt evidence is
            // due (C-2); `sweep_is_due` alone would disable cleanup forever.
            Some(stamp) if stamp > now => {
                if !self.stamp_future_warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        target: "cleanup",
                        "cleanup.stamp_future: the sweep stamp is in the future; treating it as due"
                    );
                }
                true
            }
            last => sweep_is_due(last, now, interval_millis),
        };
        if !due {
            return Ok(CleanupTick::NotDue);
        }

        match self.run_sweep().await {
            Ok(report) => {
                // The stamp is written only after a COMPLETE successful sweep,
                // so a failed run stays retryable.
                {
                    let mut gate = self
                        .cleanup_last_run
                        .lock()
                        .map_err(|_| PortError::Store("cleanup gate lock poisoned".to_string()))?;
                    *gate = Some(now);
                }
                if let Err(error) = save_stamp(&config.sweep_path, now) {
                    tracing::warn!(
                        target: "cleanup",
                        path = %config.sweep_path.display(),
                        error = %error,
                        "cleanup.stamp_write_failed: the sweep ran but its stamp could not be persisted"
                    );
                }
                Ok(CleanupTick::Ran(report))
            }
            Err(CleanupRunError::Busy) => Ok(CleanupTick::Busy),
            Err(CleanupRunError::Unavailable) => Err(PortError::Unavailable),
            Err(CleanupRunError::Failed(error)) => Err(error),
        }
    }
}

/// The composition-root injection point: `src/main.rs` calls it with the
/// resolved `deduplication_enabled` / `deduplication_similarity_threshold`.
///
/// One `MaintenancePort` whose `deduplicate()` delegates to [`DedupService`];
/// 37/39 fuse their own ports around it rather than adding a second
/// `.with_maintenance(..)` call (the field holds one port).
///
/// This form keeps a fresh never-set cancel flag, so a caller that has
/// no shutdown signal to share (tests, embedded uses) keeps the old shape.
pub fn production_deduplication_port(
    shards: ShardManager,
    prompts: Arc<UserPromptManager>,
    retention_days: u32,
    enabled: bool,
    threshold: f64,
) -> Arc<dyn MaintenancePort> {
    production_deduplication_port_with_cancel(
        shards,
        prompts,
        retention_days,
        enabled,
        threshold,
        Arc::new(AtomicBool::new(false)),
    )
}

/// [`production_deduplication_port`], sharing a shutdown flag with the near
/// scan.
///
/// `src/main.rs` sets the flag before it stops the web server, so a scan that
/// is running or queued on the blocking pool returns at its next check, the
/// pass answers an error, its guard and lease unwind, and the engine's
/// shutdown budget is not held hostage by detached CPU work.
pub fn production_deduplication_port_with_cancel(
    shards: ShardManager,
    prompts: Arc<UserPromptManager>,
    retention_days: u32,
    enabled: bool,
    threshold: f64,
    cancel: Arc<AtomicBool>,
) -> Arc<dyn MaintenancePort> {
    let store = Arc::new(store::ShardDedupStore::new(shards.clone()));
    Arc::new(
        ProductionDeduplication::new(store, shards, prompts, retention_days, enabled, threshold)
            .with_cancellation(cancel),
    )
}

/// [`production_deduplication_port_with_cancel`] with the automatic cleanup
/// surface armed (issue 41): the composition root passes the resolved
/// `autoCleanupEnabled` / `autoCleanupInterval` and the sidecar path.
#[allow(clippy::too_many_arguments)]
pub fn production_deduplication_port_with_cleanup(
    shards: ShardManager,
    prompts: Arc<UserPromptManager>,
    retention_days: u32,
    enabled: bool,
    threshold: f64,
    cleanup: CleanupSweepConfig,
    cancel: Arc<AtomicBool>,
    episodic: Option<Arc<EpisodicStore>>,
) -> Arc<dyn MaintenancePort> {
    let store = Arc::new(store::ShardDedupStore::new(shards.clone()));
    Arc::new(
        ProductionDeduplication::new(store, shards, prompts, retention_days, enabled, threshold)
            .with_cancellation(cancel)
            .with_cleanup(cleanup)
            .with_episodic(episodic),
    )
}

#[cfg(test)]
mod tests {
    use super::RunGuard;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn run_guard_rejects_when_already_running() {
        let running = Arc::new(AtomicUsize::new(0));
        let guard = RunGuard::acquire(&running).expect("first acquire should succeed");
        assert!(
            RunGuard::acquire(&running).is_none(),
            "second acquire while guard held must be rejected"
        );
        drop(guard);
        assert_eq!(
            running.load(Ordering::Acquire),
            0,
            "counter must return to 0 after drop"
        );
    }
}
