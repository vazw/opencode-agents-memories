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
//! [`DedupStore`] is the only boundary between the algorithm and libsql, so
//! the greedy scan and the run guard are provable with a fake while the real
//! glue stays in [`store`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use mem_store::shard_manager::ShardManager;

use super::ports::{
    DedupDuplicate, DedupMemoryRef, DedupReport, MaintenancePort, MigrationReport,
    MigrationRunReport, NearDuplicateGroup, PortError, TagMigrationProgressView,
    TagMigrationReport,
};

pub mod engine;
pub mod store;

/// Upstream's disabled-service business message.
pub const DISABLED_MESSAGE: &str = "Deduplication is disabled in config";
/// Upstream's concurrent-run business message.
pub const RUNNING_MESSAGE: &str = "Deduplication already running";

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

    /// Every row of one shard, ordered `created_at DESC, id ASC`.
    async fn memories(&self, shard: &DedupShard) -> Result<Vec<DedupMemory>, PortError>;

    /// Delete one row, then bring the shard's vector accounting in line with
    /// the present rows.
    ///
    /// Today: `delete_memory_by_id` + `decrement_vector_count`. After
    /// hardening 43.2 / ADR-0018: the delete plus an ABSOLUTE recount
    /// (`sync_count_from_shard`). The trait, the service and every behavioural
    /// assertion are identical across that migration; only the production
    /// adapter's second call changes.
    async fn delete(&self, shard: &DedupShard, memory_id: &str) -> Result<(), PortError>;
}

/// One deduplication pass over a [`DedupStore`].
pub struct DedupService {
    store: Arc<dyn DedupStore>,
    enabled: bool,
    threshold: f64,
    running: Arc<AtomicBool>,
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
            running: Arc::new(AtomicBool::new(false)),
        }
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

        for shard in &shards {
            let rows = self.store.memories(shard).await?;
            let plan = engine::exact_plan(&rows);

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

            for group in engine::near_groups(&rows, &plan.survivors, self.threshold) {
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
        })
    }
}

/// Held for one run; `Drop` releases the flag on every exit path, including
/// `?` and a panic.
struct RunGuard {
    flag: Arc<AtomicBool>,
}

impl RunGuard {
    fn acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self {
                flag: Arc::clone(flag),
            })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// `MaintenancePort` with a real `deduplicate()`; every other method keeps the
/// documented `Unavailable` semantics (migrations are 37/39's to wire).
pub struct ProductionDeduplication {
    service: DedupService,
}

impl ProductionDeduplication {
    /// Wraps a store-backed service.
    pub fn new(store: Arc<dyn DedupStore>, enabled: bool, threshold: f64) -> Self {
        Self {
            service: DedupService::new(store, enabled, threshold),
        }
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
}

/// The composition-root injection point: `src/main.rs` calls it with the
/// resolved `deduplication_enabled` / `deduplication_similarity_threshold`.
///
/// One `MaintenancePort` whose `deduplicate()` delegates to [`DedupService`];
/// 37/39 fuse their own ports around it rather than adding a second
/// `.with_maintenance(..)` call (the field holds one port).
pub fn production_deduplication_port(
    shards: ShardManager,
    enabled: bool,
    threshold: f64,
) -> Arc<dyn MaintenancePort> {
    let store = Arc::new(store::ShardDedupStore::new(shards));
    Arc::new(ProductionDeduplication::new(store, enabled, threshold))
}
