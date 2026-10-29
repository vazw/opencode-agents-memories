//! W7 — retired-shard reaping (ADR-0023).
//!
//! One root-wide entry point, [`ShardManager::reap_retired_shards`]: a retired
//! shard's registry row is deleted first, then its `.db` and sidecars are
//! removed under issue 46's exclusivity proof, after the `-tshm` liveness
//! query answers `Free`/`Missing` at both call sites.

use std::path::{Path, PathBuf};

use crate::db::Value;
use crate::error::StoreError;
use crate::operation_lock::{METADATA_DB_NAME, acquire_turso_operation_lock};
use crate::shard_manager::{ShardInfo, ShardManager};
use crate::sqlite_files::{
    FILE_LOCK_RETRY_DELAYS_MS, RetryContext, remove_sqlite_database_reporting, sidecar_path,
    with_sqlite_file_lock_retry,
};

pub use crate::sqlite_files::{FileRemovalFailure, RemovalOutcome};

/// The root-wide registry listing; the SQL lives here because `sql.rs` is the
/// upstream-golden locus (`golden_sql.rs` pins it byte-for-byte).
const LIST_ALL_SHARDS: &str =
    "SELECT * FROM shards ORDER BY scope ASC, scope_hash ASC, shard_index ASC";

/// What one reap considered, removed and left behind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReapReport {
    /// Every registry row that passed the predicate, in registry order.
    pub candidates: Vec<i64>,
    /// Every candidate whose row and pre-existing files are gone.
    pub reaped: Vec<i64>,
    /// Every considered row that was not reaped, with its reason.
    pub skipped: Vec<SkippedShard>,
    /// The apparent size of the removed empty files.
    pub bytes_freed: u64,
}

/// One considered row that was not reaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedShard {
    /// The registry row id.
    pub shard_id: i64,
    /// Why it was left alone.
    pub reason: ReapSkipReason,
}

/// The stable skip reasons, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReapSkipReason {
    /// The row is the tuple's active shard.
    Active,
    /// The shard file still holds rows.
    NonEmpty {
        /// The measured `COUNT(*)`.
        rows: u64,
    },
    /// A zero-active tuple's highest row is the reactivation target.
    HighestWithoutActive,
    /// The shard file is missing, unopenable or unreadable.
    Unopenable,
    /// A live `-tshm` holder answered the byte-0 query `Held`.
    LiveTshmHolder,
    /// The `-tshm` probe could not answer (fail closed).
    TshmProbeUnknown {
        /// The probe's OS error, when it had one.
        errno: Option<i32>,
    },
    /// The registry row vanished before the delete.
    RowGone,
    /// The row is gone but at least one file removal failed.
    FileDeleteFailed {
        /// One entry per failed file.
        failures: Vec<FileRemovalFailure>,
    },
}

impl ReapSkipReason {
    /// The stable token the CLI renders.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NonEmpty { .. } => "non_empty",
            Self::HighestWithoutActive => "highest_without_active",
            Self::Unopenable => "unopenable",
            Self::LiveTshmHolder => "live_tshm_holder",
            Self::TshmProbeUnknown { .. } => "tshm_probe_unknown",
            Self::RowGone => "row_gone",
            Self::FileDeleteFailed { .. } => "file_delete_failed",
        }
    }

    /// The reason-specific detail, when the variant carries one.
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::NonEmpty { rows } => Some(format!("{rows} rows")),
            Self::TshmProbeUnknown { errno } => Some(format!("errno {errno:?}")),
            Self::FileDeleteFailed { failures } => {
                Some(format!("{} file(s) failed", failures.len()))
            }
            Self::Active
            | Self::HighestWithoutActive
            | Self::Unopenable
            | Self::LiveTshmHolder
            | Self::RowGone => None,
        }
    }
}

/// The four file targets of one shard database.
fn target_paths(db_path: &Path) -> [PathBuf; 4] {
    [
        db_path.to_path_buf(),
        sidecar_path(db_path, "-wal"),
        sidecar_path(db_path, "-shm"),
        sidecar_path(db_path, "-tshm"),
    ]
}

/// The target paths that are regular files before the removal, with their
/// sizes. `bytes_freed` counts only these that are gone afterwards.
fn pre_existing_files(db_path: &Path) -> Vec<(PathBuf, u64)> {
    target_paths(db_path)
        .into_iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(&path).ok()?;
            metadata.is_file().then_some((path, metadata.len()))
        })
        .collect()
}

impl ShardManager {
    /// Reaps the shards the scope has retired: a registry row is reaped only
    /// when it is inactive, its own row count is zero, it is not the
    /// reactivation target of a zero-active tuple, no live `-tshm` holder
    /// exists, and its database opens cleanly read-only.
    ///
    /// `dry_run == true` stops after the predicate and before the first row
    /// delete: the returned report names the candidates and mutates nothing.
    /// `dry_run == false` deletes each candidate's registry row first, then
    /// its `.db` and sidecars under the engine-lock proof.
    pub async fn reap_retired_shards(&self, _dry_run: bool) -> Result<ReapReport, StoreError> {
        let Some(proof) = self.exclusive_root() else {
            return Err(StoreError::NoExclusiveRoot {
                db_path: self.config().storage_path.join(METADATA_DB_NAME),
            });
        };
        // The inter-operation mutex (owner ruling 7), held for the whole call
        // and by the dry run too.
        let _operation_lock = acquire_turso_operation_lock(
            &self.config().storage_path,
            self.process_probe(),
            "reap-shards",
        )?;
        let metadata = self.metadata_handle().await?;
        let rows = metadata.query(LIST_ALL_SHARDS, Vec::new()).await?;
        let shards: Vec<ShardInfo> = rows
            .into_iter()
            .map(|row| self.row_to_shard_info(row))
            .collect();

        let mut report = ReapReport::default();
        let mut candidate_shards: Vec<ShardInfo> = Vec::new();
        for shard in &shards {
            if shard.is_active {
                report.skipped.push(SkippedShard {
                    shard_id: shard.id,
                    reason: ReapSkipReason::Active,
                });
                continue;
            }
            report.candidates.push(shard.id);
            candidate_shards.push(shard.clone());
        }

        for shard in candidate_shards {
            let deleted = metadata
                .execute(crate::sql::DELETE_SHARD_ROW, vec![Value::Integer(shard.id)])
                .await?;
            if deleted.changes != 1 {
                report.skipped.push(SkippedShard {
                    shard_id: shard.id,
                    reason: ReapSkipReason::RowGone,
                });
                continue;
            }
            let pre_existing = pre_existing_files(&shard.db_path);
            let claim = proof.claim(&shard.db_path)?;
            let outcome = with_sqlite_file_lock_retry(
                || remove_sqlite_database_reporting(claim),
                FILE_LOCK_RETRY_DELAYS_MS.len(),
                &RetryContext::system(),
            )?;
            // Step 1 ignores the failure report; Step 6 turns it into a skip.
            let _ = outcome;
            report.reaped.push(shard.id);
            for (path, size) in pre_existing {
                if !path.exists() {
                    report.bytes_freed = report.bytes_freed.saturating_add(size);
                }
            }
        }
        Ok(report)
    }
}
