//! W7 — the `reap-shards` CLI (ADR-0023 D-23-8).
//!
//! `opencode-agents-memory --mode=reap-shards [--apply] [--storage-root <path>]`
//! is dry-run by default; `--apply` alone is the confirmation (owner ruling
//! 14). The safety conditions live in the store primitive
//! ([`mem_store::reaping`]); this module only composes the store, runs the
//! full ready gate, and renders the report.
//!
//! # Exit codes (this plan's decision, aligned to the house)
//!
//! `0` success including nothing to do · `1` a completed report with at least
//! one file-operation failure · `64` usage (validated in `src/main.rs` before
//! `Config::load_default`) · `74` store/operation failure including a lost
//! engine lock · `75` busy (the operation lock or the ready gate's legacy
//! step is held).
//!
//! # The dry run is not side-effect-free end-to-end
//!
//! The ready gate's legacy step reconciles the shard registry and can adopt an
//! orphan file into `metadata.db` **before** the report. The store-level dry
//! run mutates nothing; a CLI dry run may therefore add registry rows (never
//! remove files). The candidate set is the registry at call time.

use std::path::Path;
use std::sync::Arc;

use mem_core::config::Config;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
use mem_store::error::StoreError;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::ready::build_ready_gate_with_encryption;
use mem_store::reaping::{ReapReport, ReapSkipReason, SkippedShard};
use mem_store::shard_manager::{ShardManager, ShardRegistry};

use crate::engine::structured_stderr;

/// The one field the CLI surface needs: whether the operator confirmed the act.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapRequest {
    /// `false` (the default) is the dry run; `--apply` sets `true`.
    pub apply: bool,
}

/// Why a reap did not complete.
enum ReapRunError {
    /// Another live engine owns the storage root (exit 74, `reap.refused`).
    Refused { holder_pid: u32 },
    /// The operation lock or the legacy step is held (exit 75, `reap.busy`).
    Busy(StoreError),
    /// Any other store/ready failure (exit 74, `reap.failed`).
    Failed(StoreError),
}

/// Maps the two busy shapes onto `Busy`; every other error is a store failure.
fn classify(error: StoreError) -> ReapRunError {
    let busy = matches!(
        &error,
        StoreError::MigrationLocked { .. }
            | StoreError::Io {
                kind: Some(std::io::ErrorKind::AlreadyExists),
                ..
            }
    );
    if busy {
        ReapRunError::Busy(error)
    } else {
        ReapRunError::Failed(error)
    }
}

/// Runs one root-wide reap and returns the process exit code.
///
/// Owns a current-thread runtime so `src/main.rs` stays tokio-free. The
/// storage root is the CLI/env root (`HostConfig.storage_root`), never the
/// JSONC `storagePath` key; the encryption settings come from the resolved
/// config.
pub fn run(config: &Config, storage_root: &Path, request: &ReapRequest) -> i32 {
    let mut store_config: StoreConfig = mem_store::config::from_core(config);
    store_config.storage_path = storage_root.to_path_buf();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            structured_stderr(
                "reap.failed",
                "error",
                "reap",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error, "reap runtime failed");
            return 74;
        }
    };

    let outcome = runtime.block_on(async {
        // The engine lock FIRST: the reaper unlinks files, so a root owned by
        // another live engine is refused before anything is opened for
        // deletion (exit 74, never a mutation).
        let _engine_lock = match acquire_engine_lock(storage_root, Arc::new(SystemProcessProbe)) {
            Ok(EngineLockOutcome::Acquired(lock)) => lock,
            Ok(EngineLockOutcome::Lost { holder_pid }) => {
                tracing::warn!(
                    holder_pid,
                    "reap: the storage root is owned by another process"
                );
                return Err(ReapRunError::Refused { holder_pid });
            }
            Err(error) => return Err(ReapRunError::Failed(error)),
        };
        let connections = Arc::new(ConnectionManager::new(
            storage_root.to_path_buf(),
            store_config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory),
        ));
        // Identity `None`: this run never writes a shard.
        let shards = Arc::new(ShardManager::new(
            storage_root.to_path_buf(),
            store_config.clone(),
            connections.clone(),
            Arc::new(SystemProcessProbe),
            None,
        ));
        let registry: Arc<dyn ShardRegistry> = shards.clone();
        let ready = Arc::new(build_ready_gate_with_encryption(
            storage_root,
            connections.clone(),
            shards.clone(),
            registry,
            &store_config,
            Vec::new(),
        ));
        // The ready gate runs OUTSIDE the reap's operation lock: its legacy
        // step takes the same single lock file, so running it under the reap
        // lock would self-refuse.
        ready.ensure_ready().await.map_err(classify)?;
        // The ready gate's legacy step opened every shard read-write through
        // the connection manager and cached the handles. A cached writable
        // handle IS a live `-tshm` holder to the reaper's probe, so close
        // them before the predicate; the primitive reopens each candidate
        // read-only for its own count. Closing mutates no file.
        connections.close_all().await.map_err(classify)?;
        shards
            .reap_retired_shards(!request.apply)
            .await
            .map_err(classify)
    });

    match outcome {
        Ok(report) => {
            print_completed(&report, !request.apply);
            print_file_failures(&report);
            if report
                .skipped
                .iter()
                .any(|skip| matches!(skip.reason, ReapSkipReason::FileDeleteFailed { .. }))
            {
                1
            } else {
                0
            }
        }
        Err(ReapRunError::Refused { holder_pid }) => {
            structured_stderr(
                "reap.refused",
                "warn",
                "reap",
                serde_json::json!({ "holder_pid": holder_pid, "detail_short": true }),
            );
            74
        }
        Err(ReapRunError::Busy(error)) => {
            structured_stderr(
                "reap.busy",
                "warn",
                "reap",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error.message(), "reap busy");
            75
        }
        Err(ReapRunError::Failed(error)) => {
            structured_stderr(
                "reap.failed",
                "error",
                "reap",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error.message(), "reap failed");
            74
        }
    }
}

/// Prints EXACTLY one stdout line: the completion report.
fn print_completed(report: &ReapReport, dry_run: bool) {
    let skipped: Vec<serde_json::Value> = report.skipped.iter().map(skipped_json).collect();
    let line = serde_json::json!({
        "event": "reap.completed",
        "dry_run": dry_run,
        "candidates": report.candidates,
        "reaped": report.reaped,
        "skipped": skipped,
        "bytes_freed": report.bytes_freed,
    });
    println!("{line}");
}

/// One `skipped` entry: the stable token plus the reason-specific detail.
fn skipped_json(skip: &SkippedShard) -> serde_json::Value {
    let mut value = serde_json::json!({
        "shard_id": skip.shard_id,
        "reason": skip.reason.as_str(),
    });
    if let serde_json::Value::Object(map) = &mut value {
        match &skip.reason {
            ReapSkipReason::NonEmpty { rows } => {
                map.insert("rows".to_string(), serde_json::json!(rows));
            }
            ReapSkipReason::TshmProbeUnknown { errno } => {
                map.insert("errno".to_string(), serde_json::json!(errno));
            }
            ReapSkipReason::FileDeleteFailed { failures } => {
                let failed: Vec<serde_json::Value> = failures
                    .iter()
                    .map(|failure| {
                        serde_json::json!({
                            "path": failure.path.display().to_string(),
                            "error": failure.message,
                        })
                    })
                    .collect();
                map.insert("failures".to_string(), serde_json::json!(failed));
            }
            ReapSkipReason::Active
            | ReapSkipReason::HighestWithoutActive
            | ReapSkipReason::Unopenable
            | ReapSkipReason::LiveTshmHolder
            | ReapSkipReason::RowGone => {}
        }
    }
    value
}

/// One `structured_stderr` line per failed FILE (owner ruling 6).
fn print_file_failures(report: &ReapReport) {
    for skip in &report.skipped {
        let ReapSkipReason::FileDeleteFailed { failures } = &skip.reason else {
            continue;
        };
        for failure in failures {
            structured_stderr(
                "reap.file_delete_failed",
                "error",
                "reap",
                serde_json::json!({
                    "shard_id": skip.shard_id,
                    "db_path": failure.path.display().to_string(),
                    "error": failure.message,
                }),
            );
        }
    }
}
