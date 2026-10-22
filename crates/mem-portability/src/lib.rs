//! mem-portability — the portable, versioned memory bundle (W6.4).
//!
//! Upstream's `src/services/memory-portability-service.ts` +
//! `src/shared/api/portability-schemas.ts` are the behavioural contract.
//! The port keeps the same JSON document and adds the two operations the
//! blueprint names: a **replace mode** and a **dry run**.
//!
//! The crate is split into three modules:
//!
//! - [`bundle`] — the wire shape (the versioned document, its tiers and
//!   the preserved unknown-field map);
//! - [`facts`] — the durable-fact seam ([`facts::FactStore`]) the service
//!   reads and writes; the production adapter lives in `mem-web`;
//! - [`service`] — the service itself ([`service::PortabilityService`])
//!   plus the bundle file I/O (the size-capped reader and the `0600`
//!   atomic writer).
//!
//! The service's dependencies are four seams: the facts, the profile
//! store, the episodic store and the embedder. Everything else — the
//! bundle's shape, the provenance stamps, the tier rules — lives in this
//! crate.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-portability";

pub mod bundle;
pub mod facts;
pub mod service;

pub use bundle::{
    Bundle, BundleScope, EmbeddingInfo, ExportedContributor, ExportedMemory, ExportedMilestone,
    ExportedProfile, MemoryExportDocument, PLUGIN_PACKAGE, PORTABILITY_SCHEMA_VERSION, PluginInfo,
    SchemaVersion, SourceInfo, StagingTier,
};
pub use facts::FactStore;
pub use service::{
    ExportOptions, ImportReport, MAX_IMPORT_BYTES, MILESTONE_EXPORT_LIMIT, PortabilityService,
    RejectedEntry, ReplaceMode, ResolvedScope, SESSION_EXPORT_LIMIT, SkippedEntry, parse_bundle,
    read_bundle, write_bundle,
};

/// The portability crate's failure surface.
///
/// Every error message that reaches a user is upstream's verbatim: the
/// "Import aborted: N memory id(s) already exist" abort message, the
/// "Unsupported export schemaVersion N" version message and the
/// "Invalid export document: …" validation message. The internal faults
/// carry a detail that is logged, never echoed (the D-9 posture of the
/// REST layer).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PortabilityError {
    /// The import aborted because ids already exist in the target (or
    /// appear twice in the bundle). **Zero writes.**
    #[error("Import aborted: {count} memory id(s) already exist")]
    ImportAborted {
        /// How many ids were rejected.
        count: usize,
        /// The rejected rows.
        rejected: Vec<service::RejectedEntry>,
    },
    /// The import aborted because milestone ids already exist in the
    /// target (or appear twice in the bundle). **Zero writes.**
    ///
    /// The milestone id is the table's primary key, so the collision is
    /// global — the same contract the fact tiers have, and it is checked
    /// BEFORE the first write.
    #[error("Import aborted: {count} milestone id(s) already exist")]
    ImportAbortedMilestones {
        /// How many ids were rejected.
        count: usize,
    },
    /// The bundle carries a schema version this build cannot read.
    #[error(
        "Unsupported export schemaVersion {0}. Upgrade opencode-agents-memory to import this file."
    )]
    UnsupportedSchemaVersion(u32),
    /// The bundle is structurally invalid.
    #[error("Invalid export document: {0}")]
    InvalidBundle(String),
    /// The bundle file is not JSON.
    #[error("Invalid JSON: {0}")]
    InvalidJson(String),
    /// The bundle file does not exist.
    #[error("Import file not found: {0}")]
    FileNotFound(String),
    /// The bundle file exceeds the size cap.
    #[error("Import file exceeds {max} byte limit", max = service::MAX_IMPORT_BYTES)]
    ImportTooLarge,
    /// The path is not safe to use (a symlink at the destination).
    #[error("Unsafe portability path: {0}")]
    UnsafePath(String),
    /// The container tag could not be resolved.
    #[error("Invalid scope: {0}")]
    InvalidScope(String),
    /// The durable-fact store refused the operation.
    #[error("portability store error: {0}")]
    Store(String),
    /// The profile store refused the operation.
    #[error("portability profile error: {0}")]
    Profile(String),
    /// The episodic store refused the operation.
    #[error("portability episodic error: {0}")]
    Episodic(String),
    /// The embedder refused the operation.
    #[error("portability embed error: {0}")]
    Embed(String),
    /// A file-system operation failed.
    #[error("portability io error: {0}")]
    Io(String),
}
