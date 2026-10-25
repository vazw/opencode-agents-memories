//! The maintenance endpoints (upstream `api-handlers.ts` L873–935 and
//! L1487–1671) and the D-B sync/async bridge.
//!
//! Three groups live here:
//! - the dimension-migration pair (`GET /api/migration/detect`,
//!   `POST /api/migration/run`) over the [`MaintenancePort`];
//! - the tag-migration group (`GET /api/migration/tags/detect`,
//!   `POST /api/migration/tags/run-batch`, `GET /api/migration/tags/progress`)
//!   over the already-ported sync `TagMigrator` (W1.1);
//! - `POST /api/cleanup` (the documented 501 stub, D-15) and
//!   `POST /api/deduplicate`, which is a **real handler** over
//!   `MaintenancePort::deduplicate()`: it answers 200 with the typed report
//!   when the injected port implements the pass, answers the upstream
//!   business message at 200 when deduplication is disabled, and still
//!   answers the documented 501 when the port is the default
//!   `UnavailablePort`.
//!
//! `mem_core::dedup` is the **profile-conflict** grouping kernel, not
//! upstream's memory-deduplication service; it must not be wired here. The
//! memory-shard pass lives in [`super::deduplication`].
//!
//! **D-B / D-22 / D-23 (the bridge):** the handler pre-computes the batch
//! window's tag-generation calls **asynchronously** (`generate_tags`), then
//! hands the map to the port, which runs the sync `TagMigrator` inside
//! `spawn_blocking`. The async store/embedder calls are awaited on the
//! blocking thread through `Handle::current().block_on` — never on a tokio
//! worker, and `block_in_place` is forbidden (it panics on the current-thread
//! runtime the test suite uses). The migrated set, the counts and the cursor
//! advance are identical to upstream's row-by-row interleaving; only the
//! provider-call ordering differs.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

use super::dto::{
    DedupDto, MigrationDetectDto, MigrationRunDto, TagMigrationBatchDto, TagMigrationDetectDto,
    TagMigrationProgressDto,
};
use super::envelope;
use super::ports::PortError;
use super::{json_body_or_default, port_failure};
use crate::server::AppState;

/// The default batch window (`body?.batchSize || 5`).
const DEFAULT_BATCH_SIZE: usize = 5;
/// The two accepted migration strategies (upstream's literal check).
const STRATEGIES: [&str; 2] = ["fresh-start", "re-embed"];
/// D-15: the cleanup stub's fixed message.
const CLEANUP_UNAVAILABLE: &str = "cleanup is not available in this build";
/// The documented 501 of a real handler whose port is unavailable in this
/// build (issue 37 injects a real maintenance port).
const DEDUPLICATE_UNAVAILABLE: &str = "deduplicate is not available in this build";

/// `POST /api/migration/run` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunMigrationRequest {
    strategy: Option<String>,
}

/// `POST /api/migration/tags/run-batch` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunBatchRequest {
    batch_size: Option<usize>,
}

/// Maps a maintenance port failure: an upstream business message is answered
/// at HTTP 200 with `success:false`, everything else is the fixed 500 (D-9).
fn maintenance_failure(endpoint: &'static str, error: PortError) -> Response {
    match error {
        PortError::Invalid(message) => envelope::business_error(&message),
        other => port_failure(endpoint, other),
    }
}

/// `GET /api/migration/detect` — upstream `handleDetectMigration`.
pub async fn detect_migration(State(state): State<Arc<AppState>>) -> Response {
    match state.api.maintenance.detect_migration().await {
        Ok(report) => envelope::ok(&MigrationDetectDto {
            needs_migration: report.needs_migration,
            config_dimensions: report.config_dimensions,
            config_model: report.config_model,
            shard_mismatches: report.shard_mismatches,
        }),
        Err(error) => maintenance_failure("GET /api/migration/detect", error),
    }
}

/// `POST /api/migration/run` — upstream `handleRunMigration`.
///
/// Upstream answers `success: result.success` **while still carrying `data`**,
/// so the envelope is built with [`envelope::success_with_flag`].
pub async fn run_migration(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: RunMigrationRequest = json_body_or_default(&body);
    let strategy = request
        .strategy
        .filter(|strategy| !strategy.is_empty())
        .unwrap_or_else(|| STRATEGIES[0].to_string());
    if !STRATEGIES.contains(&strategy.as_str()) {
        return envelope::business_error("Invalid strategy");
    }

    match state.api.maintenance.run_migration(&strategy).await {
        Ok(report) => flagged_response(
            report.success,
            &MigrationRunDto {
                success: report.success,
                strategy: report.strategy,
                deleted_shards: report.deleted_shards,
                re_embedded_memories: report.re_embedded_memories,
                duration: report.duration_ms,
                error: report.error,
            },
        ),
        Err(error) => maintenance_failure("POST /api/migration/run", error),
    }
}

/// `{"success":<flag>,"data":…}` at `200`, or the D-9 `500` when the DTO
/// cannot be serialized (F11: the flag form could not answer 500 before).
fn flagged_response(flag: bool, data: &impl serde::Serialize) -> Response {
    match envelope::success_with_flag(flag, data) {
        Some(body) => envelope::json(StatusCode::OK, body),
        None => envelope::internal_error(),
    }
}

/// `GET /api/migration/tags/detect` — upstream `handleDetectTagMigration`.
pub async fn detect_tag_migration(State(state): State<Arc<AppState>>) -> Response {
    match state.api.maintenance.detect_tag_migration().await {
        Ok(report) => envelope::ok(&TagMigrationDetectDto {
            needs_migration: report.needs_migration,
            count: report.count,
        }),
        Err(error) => maintenance_failure("GET /api/migration/tags/detect", error),
    }
}

/// `GET /api/migration/tags/progress` — upstream
/// `handleGetTagMigrationProgress`.
pub async fn tag_migration_progress(State(state): State<Arc<AppState>>) -> Response {
    match state.api.maintenance.tag_migration_progress().await {
        Ok(progress) => envelope::ok(&TagMigrationProgressDto {
            processed: progress.processed,
            cursor: progress.cursor,
            total: progress.total,
            current_batch: progress.current_batch,
            total_batches: progress.total_batches,
            is_complete: progress.is_complete,
            errors: progress.errors,
        }),
        Err(error) => maintenance_failure("GET /api/migration/tags/progress", error),
    }
}

/// Whether a record still needs tag generation (upstream's
/// `tags.split(",").map(trim).filter(Boolean).length === 0`).
fn needs_tags(tags: Option<&str>) -> bool {
    tags.map(|raw| raw.split(',').all(|tag| tag.trim().is_empty()))
        .unwrap_or(true)
}

/// `POST /api/migration/tags/run-batch` — upstream
/// `handleRunTagMigrationBatch`, through the D-B bridge.
pub async fn run_tag_migration_batch(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: RunBatchRequest = json_body_or_default(&body);
    let batch_size = request
        .batch_size
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_BATCH_SIZE);

    // 1. The project shards' rows and the migrator's own cursor.
    let records = match state.api.maintenance.tag_migration_records().await {
        Ok(records) => records,
        Err(error) => return maintenance_failure("POST /api/migration/tags/run-batch", error),
    };
    let progress = match state.api.maintenance.tag_migration_progress().await {
        Ok(progress) => progress,
        Err(error) => return maintenance_failure("POST /api/migration/tags/run-batch", error),
    };
    // Mirror `TagMigrator`'s reset rule: a fresh run (or a retry after a
    // completed pass) starts at 0, so the pre-computed window must too.
    let start = if progress.total == 0 || progress.is_complete {
        0
    } else {
        progress.cursor
    };
    let start = start.min(records.len());
    let end = start.saturating_add(batch_size).min(records.len());

    // 2. The window's tag-generation calls, awaited BEFORE the window is
    //    written (D-22): the sync `TagGeneration` seam is fed by this map.
    let mut precomputed: HashMap<String, Result<Vec<String>, String>> = HashMap::new();
    for record in &records[start..end] {
        if needs_tags(record.tags.as_deref()) {
            let tags = state
                .api
                .maintenance
                .generate_tags(&record.id, &record.content)
                .await;
            precomputed.insert(record.id.clone(), tags);
        }
    }

    // 3. The sync migrator runs on a blocking thread (never a worker).
    match state
        .api
        .maintenance
        .run_tag_migration_batch(batch_size, records, precomputed)
        .await
    {
        Ok(outcome) => envelope::ok(&TagMigrationBatchDto {
            processed: outcome.processed,
            total: outcome.total,
            has_more: outcome.has_more,
            errors: outcome.errors,
        }),
        Err(error) => maintenance_failure("POST /api/migration/tags/run-batch", error),
    }
}

/// `POST /api/cleanup` — the documented 501 stub (D-15).
///
/// The service does not exist in this build; the endpoint and its seam stay
/// registered so the contract is stable. This handler must be replaced when
/// `MaintenancePort::cleanup` gains an implementation.
pub async fn cleanup() -> Response {
    envelope::not_implemented(CLEANUP_UNAVAILABLE)
}

/// `POST /api/deduplicate` — upstream `handleRunDeduplication`.
///
/// Four arms inline: the typed report, the unavailable-port 501, the upstream
/// business message at 200 (`Deduplication is disabled in config`), and the
/// D-9 500 for everything else. The match is inline on purpose — the shared
/// `maintenance_failure` helper belongs to the migration handlers, whose
/// arity another wave is changing.
pub async fn deduplicate(State(state): State<Arc<AppState>>) -> Response {
    match state.api.maintenance.deduplicate().await {
        Ok(report) => envelope::ok(&DedupDto::from(report)),
        Err(PortError::Unavailable) => envelope::not_implemented(DEDUPLICATE_UNAVAILABLE),
        Err(PortError::Invalid(message)) => envelope::business_error(&message),
        Err(other) => port_failure("POST /api/deduplicate", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DTO whose `Serialize` always fails, so `success_with_flag`'s `None`
    /// arm is reachable without a live maintenance run.
    struct FailingSerialize;

    impl serde::Serialize for FailingSerialize {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "intentional serialization failure",
            ))
        }
    }

    #[test]
    fn test_success_with_flag_serialization_failure_yields_500() {
        assert!(envelope::success_with_flag(true, &FailingSerialize).is_none());
        let response = flagged_response(true, &FailingSerialize);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
