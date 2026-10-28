//! W3 — the `reembed-scope` CLI: one scope's supported way out of a model
//! change (ADR-0018 R-12).
//!
//! The command is the one the typed refusal names
//! (`opencode-agents-memory --mode=reembed-scope --scope <container tag>`).
//! It validates the tag BEFORE opening anything (exit 64), composes the store
//! exactly like the engine's root, wraps [`EmbeddingService`] — never the raw
//! `Embedder`, because the service is the only layer that applies the
//! configured document prefix, which is part of the identity — and runs
//! `MigrationService::reembed_scope` under its operation lock.
//!
//! Exit codes: `0` success, `64` invalid scope, `74` any `success:false`.
//! Exactly one JSON line is printed on stdout for a completed run.

use std::path::Path;
use std::sync::Arc;

use mem_core::config::Config;
use mem_core::embedding::EmbeddingIdentity;
use mem_core::scope::extract_scope_from_container_tag;
use mem_embed::EmbeddingServiceOptions;
use mem_embed::embedder::BoxFuture;
use mem_embed::embedder::EmbedTask;
use mem_embed::service::EmbeddingService;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
use mem_store::error::StoreError;
use mem_store::migration_service::{MigrationResult, MigrationService, ReEmbedder};
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::ready::build_ready_gate_with_encryption;
use mem_store::shard_manager::{ShardManager, ShardRegistry};

use crate::composition::embedder_from_config;
use crate::engine::structured_stderr;

/// The one field the CLI surface needs: the scope's container tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReembedRequest {
    /// `opencode_<scope>_<hash>` (the tag the error's remedy names).
    pub container_tag: String,
}

/// The production [`ReEmbedder`]: the `EmbeddingService`, so the configured
/// document prefix is applied exactly as every other document write applies
/// it (the prefix is part of the identity fingerprint).
struct ServiceReEmbedder {
    service: EmbeddingService,
}

impl ReEmbedder for ServiceReEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.service
                .warmup()
                .await
                .map_err(|error| StoreError::internal(error.to_string()))
        })
    }

    fn clear_cache(&self) {}

    fn embed(&self, input: &str) -> BoxFuture<'_, Result<Vec<f32>, StoreError>> {
        let input = input.to_owned();
        Box::pin(async move {
            self.service
                .embed(&input, Some(EmbedTask::Document))
                .await
                .map_err(|error| StoreError::internal(error.to_string()))
        })
    }
}

/// Runs one scope re-embed and returns the process exit code (0/64/74).
///
/// Owns a current-thread runtime so `src/main.rs` stays tokio-free. The
/// storage root is the CLI/env root (`HostConfig.storage_root`), never the
/// JSONC `storagePath` key; the encryption settings come from the resolved
/// config.
pub fn run(config: &Config, storage_root: &Path, request: &ReembedRequest) -> i32 {
    // 0. Validate the scope BEFORE any store is opened, any lock taken or any
    // file touched (exit 64). The test pins that a non-existent root stays
    // non-existent.
    let scoped = match extract_scope_from_container_tag(&request.container_tag) {
        Ok(scoped) => scoped,
        Err(_) => {
            structured_stderr(
                "reembed.invalid_scope",
                "error",
                "reembed",
                serde_json::json!({ "detail_short": true }),
            );
            return 64;
        }
    };
    let Some(hash) = scoped.hash else {
        structured_stderr(
            "reembed.invalid_scope",
            "error",
            "reembed",
            serde_json::json!({ "detail_short": true }),
        );
        return 64;
    };
    let scope = scoped.scope;
    let scope_hash = hash.as_str().to_string();

    let mut store_config: StoreConfig = mem_store::config::from_core(config);
    store_config.storage_path = storage_root.to_path_buf();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            structured_stderr(
                "reembed.failed",
                "error",
                "reembed",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error, "reembed runtime failed");
            return 74;
        }
    };

    let outcome = runtime.block_on(async move {
        // The re-embed swap renames shard files THROUGH the exclusivity
        // proof, so this process must own the root's engine lock (the same
        // lock a live engine owns). A lost lock is another process's root:
        // refuse (74), never mutate.
        let _engine_lock = match acquire_engine_lock(storage_root, Arc::new(SystemProcessProbe)) {
            Ok(EngineLockOutcome::Acquired(lock)) => lock,
            Ok(EngineLockOutcome::Lost { holder_pid }) => {
                tracing::warn!(
                    holder_pid,
                    "reembed: the storage root is owned by another process"
                );
                return Err(StoreError::internal(format!(
                    "storage root is owned by process {holder_pid}"
                )));
            }
            Err(error) => return Err(error),
        };
        let connections = Arc::new(ConnectionManager::new(
            storage_root.to_path_buf(),
            store_config.clone(),
            Platform::current(),
            Arc::new(TursoConnectFactory),
        ));
        let shards = Arc::new(ShardManager::new(
            storage_root.to_path_buf(),
            store_config.clone(),
            connections.clone(),
            Arc::new(SystemProcessProbe),
            Some(EmbeddingIdentity::from_config(config)),
        ));
        let registry: Arc<dyn ShardRegistry> = shards.clone();
        // W3 pins the encryption slot exactly like the engine's composition.
        let ready = Arc::new(build_ready_gate_with_encryption(
            storage_root,
            connections.clone(),
            shards.clone(),
            registry,
            &store_config,
            Vec::new(),
        ));
        // The ready gate runs OUTSIDE the re-embed's operation lock: its
        // legacy step acquires the same single lock file, so running it under
        // the re-embed lock would self-refuse (`migration_service.rs` keeps
        // its shipped `reembed_scope`, which calls `ensure_ready` inside).
        ready.ensure_ready().await?;
        // The composition is pinned to `EmbeddingService` (AC22): the raw
        // `Embedder` skips the document prefix that the identity covers.
        let re_embedder: Arc<dyn ReEmbedder> = Arc::new(ServiceReEmbedder {
            service: EmbeddingService::new(
                EmbeddingServiceOptions::from_config(config),
                embedder_from_config(config),
            ),
        });
        let migration = MigrationService::new(
            store_config.clone(),
            ready,
            shards,
            connections,
            re_embedder,
        );
        migration.reembed_scope(scope, &scope_hash).await
    });

    match outcome {
        Ok(result) if result.success => {
            print_completed(&result);
            0
        }
        Ok(result) => {
            print_completed(&result);
            structured_stderr(
                "reembed.failed",
                "error",
                "reembed",
                serde_json::json!({
                    "detail": result.error.as_deref().unwrap_or("re-embed did not complete"),
                }),
            );
            74
        }
        Err(error) => {
            structured_stderr(
                "reembed.failed",
                "error",
                "reembed",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error.message(), "reembed failed");
            74
        }
    }
}

/// Prints EXACTLY one stdout line: the completion report (never content and
/// never paths).
fn print_completed(result: &MigrationResult) {
    let line = serde_json::json!({
        "event": "reembed.completed",
        "success": result.success,
        "strategy": result.strategy,
        "re_embedded_memories": result.re_embedded_memories,
        "duration_ms": result.duration_ms,
    });
    println!("{line}");
}
