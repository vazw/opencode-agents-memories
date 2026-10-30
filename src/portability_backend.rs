//! Issue 70 wave 1 — the composition root's portability adapter.
//!
//! `mem-bridge` may not depend on `mem-portability` (ADR-0001's enforced
//! ceiling), so the bridge owns only the seam
//! ([`PortabilityFactory`]/[`PortabilityBackend`] in
//! `mem_bridge::tools::portability`). This module is the production
//! implementation the root's engine assembly hands to
//! `Engine::from_composed`:
//!
//! * [`ProductionPortabilityFactory`] builds a backend over the real
//!   [`PortabilityService`], configured with the RESOLVED container-tag prefix
//!   and embedding identity;
//! * the backend maps every service variant onto the bridge's total fault
//!   surface. The internal class (`Store`/`Profile`/`Episodic`/`Embed`/`Io`)
//!   collapses to a payload-free `Store` fault; the detail is logged here with
//!   `mem_store::tracing::warn!` so nothing leaks onto the wire and nothing is lost in
//!   the log.

use std::path::Path;
use std::sync::Arc;

use mem_bridge::tools::portability::{
    AbortKind, ExportOutcome, ExportRequest, ImportOutcome, ImportRequest, PortabilityBackend,
    PortabilityDeps, PortabilityFactory, PortabilityFault, RowNote,
};
use mem_embed::embedder::BoxFuture;
use mem_portability::{
    ExportOptions, PortabilityError, PortabilityService, ReplaceMode, read_bundle, write_bundle,
};
use mem_web::api::store_adapter::ProductionFacts;

/// The composition root's factory: the engine seam consults it exactly once.
pub struct ProductionPortabilityFactory;

impl PortabilityFactory for ProductionPortabilityFactory {
    fn build(&self, deps: &PortabilityDeps) -> Option<Arc<dyn PortabilityBackend>> {
        let service = PortabilityService::new(
            Arc::new(ProductionFacts::new(deps.shards.clone())),
            deps.profile.clone(),
            deps.episodic.clone(),
            deps.embedder.clone(),
        )
        // Both overrides are load-bearing: without the prefix the bundle's
        // `source.containerTag` names the default scope, and without the
        // embedding identity the bundle records a model this build never used.
        .with_container_tag_prefix(&deps.container_tag_prefix)
        .with_embedding_info(&deps.embedding_model, deps.embedding_dimensions);
        Some(Arc::new(ProductionPortabilityBackend { service }))
    }
}

/// The bridge seam over the real portability service.
struct ProductionPortabilityBackend {
    service: PortabilityService,
}

impl PortabilityBackend for ProductionPortabilityBackend {
    fn export<'a>(
        &'a self,
        request: &'a ExportRequest,
    ) -> BoxFuture<'a, Result<ExportOutcome, PortabilityFault>> {
        Box::pin(async move {
            let bundle = self
                .service
                .export(
                    Path::new(&request.project_path),
                    // Upstream parity: no staging tier on export.
                    ExportOptions {
                        include_staging: false,
                    },
                )
                .await
                .map_err(production_fault)?;
            write_bundle(&request.output_path, &bundle).map_err(production_fault)?;
            Ok(ExportOutcome {
                container_tag: bundle.source.container_tag.clone(),
                count: bundle.memories.len(),
                scope_hash: bundle.source.scope_hash.clone(),
            })
        })
    }

    fn import<'a>(
        &'a self,
        request: &'a ImportRequest,
    ) -> BoxFuture<'a, Result<ImportOutcome, PortabilityFault>> {
        Box::pin(async move {
            let bundle = read_bundle(&request.input_path).map_err(production_fault)?;
            // `Merge` only (upstream's abort-on-duplicate); the port's
            // overwrite mode stays REST-only.
            let report = self
                .service
                .import(
                    Path::new(&request.project_path),
                    bundle,
                    ReplaceMode::Merge,
                    request.dry_run,
                )
                .await
                .map_err(production_fault)?;
            Ok(ImportOutcome {
                container_tag: report.container_tag,
                dry_run: request.dry_run,
                imported: report.imported,
                skipped: report
                    .skipped
                    .into_iter()
                    .map(|entry| RowNote {
                        id: entry.id,
                        reason: entry.reason,
                    })
                    .collect(),
                rejected: report
                    .rejected
                    .into_iter()
                    .map(|entry| RowNote {
                        id: entry.id,
                        reason: entry.reason,
                    })
                    .collect(),
            })
        })
    }
}

/// The total fault map over `PortabilityError`'s fourteen variants. No
/// wildcard arm: a new service variant is a compile error here.
fn production_fault(error: PortabilityError) -> PortabilityFault {
    match error {
        PortabilityError::ImportAborted { count, rejected } => PortabilityFault::ImportAborted {
            kind: AbortKind::Memory,
            count,
            rejected: rejected
                .into_iter()
                .map(|entry| RowNote {
                    id: entry.id,
                    reason: entry.reason,
                })
                .collect(),
        },
        PortabilityError::ImportAbortedMilestones { count } => PortabilityFault::ImportAborted {
            kind: AbortKind::Milestone,
            count,
            rejected: Vec::new(),
        },
        PortabilityError::UnsupportedSchemaVersion(version) => {
            PortabilityFault::UnsupportedVersion(version)
        }
        PortabilityError::InvalidBundle(message) => PortabilityFault::InvalidBundle(message),
        PortabilityError::InvalidJson(message) => PortabilityFault::InvalidJson(message),
        PortabilityError::FileNotFound(path) => PortabilityFault::FileNotFound(path),
        PortabilityError::ImportTooLarge => PortabilityFault::TooLarge,
        PortabilityError::UnsafePath(path) => PortabilityFault::UnsafePath(path),
        PortabilityError::InvalidScope(message) => PortabilityFault::InvalidScope(message),
        internal @ (PortabilityError::Store(_)
        | PortabilityError::Profile(_)
        | PortabilityError::Episodic(_)
        | PortabilityError::Embed(_)
        | PortabilityError::Io(_)) => {
            // The detail is client-unsafe (a store path, a key, transcript
            // text); it stays in the log.
            mem_store::tracing::warn!(error = %internal, "portability internal fault collapsed");
            PortabilityFault::Store
        }
    }
}
