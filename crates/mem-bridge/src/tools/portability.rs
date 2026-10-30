//! Issue 70 wave 1 — the `export`/`import` wire methods and the bridge-owned
//! portability seam.
//!
//! `mem-bridge` may not depend on `mem-portability` (ADR-0001's enforced
//! ceiling), so the bridge names only these seam types; the composition root
//! implements [`PortabilityFactory`]/[`PortabilityBackend`] over the real
//! service (`src/portability_backend.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_embed::embedder::BoxFuture;

use crate::composition::ComposedRuntime;

/// One export request: the host directory and the client's absolute `.json`
/// destination.
pub struct ExportRequest {
    pub project_path: String,
    pub output_path: PathBuf,
}

/// What one export produced.
pub struct ExportOutcome {
    pub container_tag: String,
    pub count: usize,
    pub scope_hash: String,
}

/// One import request: the host directory, the client's absolute `.json`
/// source and the dry-run flag.
pub struct ImportRequest {
    pub project_path: String,
    pub input_path: PathBuf,
    pub dry_run: bool,
}

/// One row the import skipped or rejected, with a client-safe reason.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RowNote {
    pub id: String,
    pub reason: String,
}

/// What one import produced.
pub struct ImportOutcome {
    pub container_tag: String,
    pub dry_run: bool,
    pub imported: usize,
    pub skipped: Vec<RowNote>,
    pub rejected: Vec<RowNote>,
}

/// Every dependency the production backend needs, assembled ONCE from the
/// composed runtime and the resolved config.
pub struct PortabilityDeps {
    pub shards: Arc<mem_store::shard_manager::ShardManager>,
    pub profile: Arc<mem_profile::profile_store::ProfileStore>,
    pub episodic: Arc<mem_episodic::store::EpisodicStore>,
    pub embedder: Arc<dyn mem_embed::embedder::Embedder>,
    pub container_tag_prefix: String,
    pub embedding_model: String,
    pub embedding_dimensions: u32,
}

impl PortabilityDeps {
    /// The ONE deps assembly site: every field from the composed runtime or
    /// the resolved config, nothing re-derived.
    pub fn from_composed(composed: &ComposedRuntime, core: &mem_core::config::Config) -> Self {
        Self {
            shards: composed.shards.clone(),
            profile: composed.profile.clone(),
            episodic: composed.store.clone(),
            embedder: composed.embedder.clone(),
            container_tag_prefix: core.container_tag_prefix.clone(),
            embedding_model: core.embedding_model.clone(),
            embedding_dimensions: core.embedding_dimensions,
        }
    }
}

/// The composition root's factory: consulted inside `Engine::from_composed`,
/// never bypassed by a pre-built backend.
pub trait PortabilityFactory: Send + Sync {
    /// `None` = this build ships no portability backend; both methods then
    /// answer `-32001` and the engine logs `tools.portability_unavailable`.
    fn build(&self, deps: &PortabilityDeps) -> Option<Arc<dyn PortabilityBackend>>;
}

/// The bridge's view of the portability service.
pub trait PortabilityBackend: Send + Sync {
    fn export<'a>(
        &'a self,
        request: &'a ExportRequest,
    ) -> BoxFuture<'a, Result<ExportOutcome, PortabilityFault>>;

    fn import<'a>(
        &'a self,
        request: &'a ImportRequest,
    ) -> BoxFuture<'a, Result<ImportOutcome, PortabilityFault>>;
}

/// Which tier an abort came from (the tool picks upstream's exact sentence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortKind {
    Memory,
    Milestone,
}

/// The bridge's total fault surface over `mem_portability::PortabilityError`.
///
/// The internal class collapses to [`PortabilityFault::Store`] payload-free:
/// the root adapter logs the detail with `tracing::warn!` before mapping, so
/// nothing leaks and nothing is lost in the log.
#[derive(Debug, Clone)]
pub enum PortabilityFault {
    FileNotFound(String),
    InvalidJson(String),
    UnsupportedVersion(u32),
    InvalidBundle(String),
    TooLarge,
    UnsafePath(String),
    InvalidScope(String),
    ImportAborted {
        kind: AbortKind,
        count: usize,
        rejected: Vec<RowNote>,
    },
    Store,
}

use serde_json::{Value, json};

use crate::jsonrpc::Request;

use super::{ToolError, required_string};

/// The wire-level `export`: absolute `.json` path, then one backend call.
pub async fn export(
    backend: &dyn PortabilityBackend,
    request: &Request,
) -> Result<Value, ToolError> {
    let project_id = required_string(request, "project_id", "project_id")?;
    let output_path = required_string(request, "output_path", "path")?;
    if !is_absolute_json_path(output_path) {
        return Err(ToolError::invalid("path"));
    }
    let outcome = backend
        .export(&ExportRequest {
            project_path: project_id.to_string(),
            output_path: PathBuf::from(output_path),
        })
        .await
        .map_err(fault_reason)?;
    Ok(json!({
        "container_tag": outcome.container_tag,
        "count": outcome.count,
        "output_path": output_path,
        "scope_hash": outcome.scope_hash,
    }))
}

/// The wire-level `import`: absolute `.json` path, optional `dry_run`.
pub async fn import(
    backend: &dyn PortabilityBackend,
    request: &Request,
) -> Result<Value, ToolError> {
    let project_id = required_string(request, "project_id", "project_id")?;
    let input_path = required_string(request, "input_path", "path")?;
    if !is_absolute_json_path(input_path) {
        return Err(ToolError::invalid("path"));
    }
    let dry_run = super::optional_bool(request, "dry_run", "dry_run")?.unwrap_or(false);
    let outcome = backend
        .import(&ImportRequest {
            project_path: project_id.to_string(),
            input_path: PathBuf::from(input_path),
            dry_run,
        })
        .await
        .map_err(fault_reason)?;
    Ok(json!({
        "container_tag": outcome.container_tag,
        "dry_run": outcome.dry_run,
        "imported": outcome.imported,
        "rejected": notes(outcome.rejected),
        "skipped": notes(outcome.skipped),
    }))
}

/// Absolute and ending with the exact lowercase `.json`.
fn is_absolute_json_path(path: &str) -> bool {
    Path::new(path).is_absolute() && path.ends_with(".json")
}

fn notes(rows: Vec<RowNote>) -> Vec<Value> {
    rows.into_iter()
        .map(|note| json!({ "id": note.id, "reason": note.reason }))
        .collect()
}

/// The total fault map. `message` is client-safe by construction: the
/// client's own path, the bundle's version, or a fixed service sentence.
fn fault_reason(fault: PortabilityFault) -> ToolError {
    match fault {
        PortabilityFault::FileNotFound(path) => {
            ToolError::invalid("file_not_found").with_message(format!("Import file not found: {path}"))
        }
        PortabilityFault::InvalidJson(message) => {
            ToolError::invalid("invalid_json").with_message(message)
        }
        PortabilityFault::UnsupportedVersion(version) => ToolError::invalid("unsupported_version")
            .with_message(format!(
                "Unsupported export schemaVersion {version}. Upgrade opencode-agents-memory to import this file."
            ))
            .with_extra("version", json!(version)),
        PortabilityFault::InvalidBundle(message) => {
            ToolError::invalid("invalid_bundle").with_message(message)
        }
        PortabilityFault::TooLarge => ToolError::invalid("too_large"),
        PortabilityFault::UnsafePath(message) => {
            ToolError::invalid("unsafe_path").with_message(message)
        }
        PortabilityFault::InvalidScope(message) => {
            ToolError::invalid("invalid_scope").with_message(message)
        }
        PortabilityFault::ImportAborted {
            kind,
            count,
            rejected,
        } => {
            let (kind, message) = match kind {
                AbortKind::Memory => (
                    "memory",
                    format!("Import aborted: {count} memory id(s) already exist"),
                ),
                AbortKind::Milestone => (
                    "milestone",
                    format!("Import aborted: {count} milestone id(s) already exist"),
                ),
            };
            ToolError::invalid("import_aborted")
                .with_message(message)
                .with_extra("kind", json!(kind))
                .with_extra("count", json!(count))
                .with_extra("rejected", json!(notes(rejected)))
        }
        // Every internal class is payload-free on the wire; the root adapter
        // logs the detail before mapping.
        PortabilityFault::Store => ToolError::internal(),
    }
}
