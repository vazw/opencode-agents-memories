//! Purge orchestration + the `purge-session` CLI (W7.1, D6).
//!
//! # The one sanctioned erasure path
//!
//! `mem_bridge::purge::purge_session` is the ONLY place the two stores are
//! joined. It takes the SAME two locks `Consolidator::run` takes, in the same
//! order, BEFORE the read-only milestone pre-scan:
//!
//! 1. `mem_episodic::gate::try_acquire(storage_root)` — the in-process gate;
//! 2. `mem_store::operation_lock::acquire_turso_operation_lock(root, probe,
//!    "episodic-purge")` — the cross-process lock.
//!
//! Because the pre-scan happens under both locks, no milestone can be minted
//! between the scan and the deletes. Busy is a TYPED outcome — exit 75,
//! `purge.busy`, nothing read, nothing deleted.
//!
//! The M22 "is there a facts tier?" probe (`{storage_root}/facts.db` exists)
//! ALSO happens under both locks, immediately before the facts leg: a
//! consolidation that creates the file can only run while the purge does not
//! hold the gate, so the leg cannot be skipped for a database that appeared
//! after an unlocked pre-check.
//!
//! # The same-root contract
//!
//! The M22 probe is keyed on the EPISODIC root while the facts leg writes
//! through the caller's `FactsStore`, so the two must share one root:
//! `purge_session` refuses a `FactsStore` rooted elsewhere, and refuses
//! `None` while `{storage_root}/facts.db` exists. Left unguarded, the probe
//! and the leg disagree — the purge reports `skipped_no_database` (or
//! `erased`) while the facts survive in the other root, or it opens the
//! other root and creates a `facts.db` there, which a purge must never do.
//!
//! # Order and convergence
//!
//! The facts leg runs FIRST and the episodic leg second, both under the same
//! locks. A crash between the batches leaves the episodic objects intact and
//! the facts gone; a re-run finishes. Per-database atomicity is the strongest
//! promise across two files.
//!
//! The episodic leg now SCRUBS the purged contributor's own summary text out
//! of each milestone instead of deleting the milestone row, so the milestones
//! (and their merged recaps) survive. Their promoted facts must survive with
//! them: `purge_sources` is called with an EMPTY milestone-id list, because
//! deleting a surviving milestone's facts would destroy co-contributors'
//! durable knowledge (D12). Facts carrying a session/agent provenance row of
//! the purged triple are still erased by the triple arm.
//!
//! # What is never erased
//!
//! The legacy tier-1 `memories` shard, and the merged milestone recap with its
//! embedding (the scrub is text-only). Every success line carries a mandatory,
//! non-empty `not_erased` array naming both tiers, so `purge.completed` is
//! never readable as a complete erasure.
//!
//! # Exit codes
//!
//! `0` erased · `64` usage/confirmation/scope · `74` store-or-IO · `75` busy.

use std::path::Path;

use mem_episodic::EpisodicError;
use mem_episodic::purge::{EpisodicPurgeReport, SessionScope};
use mem_episodic::store::EpisodicStore;
use mem_facts::purge::FactsPurgeReport;
use mem_facts::store::FactsStore;
use mem_store::config::StoreConfig;
use mem_store::platform::{ProcessProbe, SystemProcessProbe};

use crate::engine::structured_stderr;

/// The operation-lock label the purge uses.
pub const PURGE_OPERATION: &str = "episodic-purge";

/// The request: the erasure scope, exactly as the operator supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeRequest {
    /// Project scope.
    pub project_id: String,
    /// Session scope.
    pub session_id: String,
    /// Agent scope (required: the triple is the key).
    pub agent_id: String,
}

impl PurgeRequest {
    /// The verbatim confirmation string the CLI requires.
    pub fn confirmation(&self) -> String {
        format!("{}:{}:{}", self.project_id, self.session_id, self.agent_id)
    }
}

/// Whether the facts leg ran or was skipped because there is no facts tier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FactsLeg {
    /// `{storage_root}/facts.db` existed and the leg ran.
    Ran,
    /// There is no facts tier (flags off): nothing opened or created.
    #[default]
    SkippedNoDatabase,
}

impl FactsLeg {
    /// The CLI/JSON string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ran => "ran",
            Self::SkippedNoDatabase => "skipped_no_database",
        }
    }
}

/// One tier a purge deliberately did NOT erase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotErasedTier {
    /// The tier name.
    pub tier: &'static str,
    /// Why it survives, and when it is in scope.
    pub reason: &'static str,
}

/// The single source of `not_erased` (the CLI serializes THIS constant).
///
/// A survivor the report does not name is the same lie one level up: the
/// contributor scrub keeps the milestone row, its merged summary and its
/// embedding, so both tiers are named.
pub const NOT_ERASED: [NotErasedTier; 2] = [
    NotErasedTier {
        tier: "memories-shard",
        reason: "no per-session provenance; Phase 4",
    },
    NotErasedTier {
        tier: "milestones-recap",
        reason: "the merged milestone summary and its embedding survive a contributor scrub; no re-summarization, no re-embed (owner ruling 2026-10-21)",
    },
];

/// What one purge did, plus what it deliberately left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PurgeReport {
    /// The episodic leg's counts.
    pub episodic: EpisodicPurgeReport,
    /// The facts leg's counts.
    pub facts: FactsPurgeReport,
    /// Whether the facts leg ran.
    pub facts_leg: FactsLeg,
}

impl PurgeReport {
    /// Whether every leg erased zero rows.
    pub fn is_empty(&self) -> bool {
        self.episodic.is_empty() && self.facts.is_empty()
    }

    /// The tiers this purge did NOT erase (always the two survivor entries —
    /// the shard and the merged milestone recap — so a success is never a
    /// complete-erasure claim).
    pub fn not_erased(&self) -> &'static [NotErasedTier] {
        &NOT_ERASED
    }
}

/// Why a purge did not complete.
#[derive(Debug, Clone)]
pub enum PurgeError {
    /// The scope failed validation (exit 64).
    InvalidScope(EpisodicError),
    /// Another consolidation/purge holds the locks (exit 75).
    Busy,
    /// A store, IO or same-root-contract failure (exit 74).
    Failed(String),
}

impl std::fmt::Display for PurgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScope(error) => write!(formatter, "{error}"),
            Self::Busy => formatter.write_str("another consolidation or purge holds the locks"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PurgeError {}

/// Erases one scope under the consolidator's locks.
///
/// # The same-root contract
///
/// `facts` must be rooted where `store` is: the M22 facts-tier probe reads
/// `store.storage_path()`, while the facts leg writes through `facts`. A
/// `FactsStore` rooted elsewhere is refused with [`PurgeError::Failed`] —
/// never silently skipped (an `erased` report while its rows survive) and
/// never opened (which would create a `facts.db` in the other root: a purge
/// never creates a database). `None` is accepted only while
/// `{store root}/facts.db` is absent. `run` composes both stores from one
/// `store_config`, so neither refusal is reachable through the CLI.
pub async fn purge_session(
    store: &EpisodicStore,
    facts: Option<&FactsStore>,
    request: &PurgeRequest,
) -> Result<PurgeReport, PurgeError> {
    purge_session_observed(store, facts, request, &|_| {}).await
}

/// The test/diagnostic seam: records each executed leg in order
/// (`"facts"`, then `"episodic"`).
#[doc(hidden)]
pub async fn purge_session_observed(
    store: &EpisodicStore,
    facts: Option<&FactsStore>,
    request: &PurgeRequest,
    on_leg: &dyn Fn(&'static str),
) -> Result<PurgeReport, PurgeError> {
    // 1. Validate BEFORE anything is opened, locked or read.
    let scope = SessionScope::new(&request.project_id, &request.session_id, &request.agent_id)
        .map_err(PurgeError::InvalidScope)?;

    // 1b. The same-root contract (module header): the M22 probe is keyed on
    // the episodic root, so a facts store rooted elsewhere would either be
    // skipped while its rows survive, or create a `facts.db` in the other
    // root. Refuse BEFORE any lock is taken or any file touched.
    if let Some(facts) = facts
        && facts.storage_path() != store.storage_path()
    {
        return Err(PurgeError::Failed(format!(
            "purge_session requires facts.storage_path() == store.storage_path() \
             (the same storage root): facts={} store={}",
            facts.storage_path().display(),
            store.storage_path().display()
        )));
    }

    // 2. Both locks, in the consolidator's order, before the pre-scan.
    let Some(_gate) = mem_episodic::gate::try_acquire(store.storage_path()) else {
        return Err(PurgeError::Busy);
    };
    let probe: std::sync::Arc<dyn ProcessProbe> = std::sync::Arc::new(SystemProcessProbe);
    let lock = match mem_store::operation_lock::acquire_turso_operation_lock(
        store.storage_path(),
        probe,
        PURGE_OPERATION,
    ) {
        Ok(lock) => lock,
        Err(mem_store::error::StoreError::MigrationLocked { .. }) => return Err(PurgeError::Busy),
        Err(mem_store::error::StoreError::Io {
            kind: Some(std::io::ErrorKind::AlreadyExists),
            ..
        }) => return Err(PurgeError::Busy),
        Err(error) => return Err(PurgeError::Failed(error.to_string())),
    };

    let result = erase_locked(store, facts, &scope, on_leg).await;
    lock.release();
    result
}

async fn erase_locked(
    store: &EpisodicStore,
    facts: Option<&FactsStore>,
    scope: &SessionScope,
    on_leg: &dyn Fn(&'static str),
) -> Result<PurgeReport, PurgeError> {
    // 3. Facts FIRST, then episodic (both under both locks).
    //
    // M22 decides the facts leg HERE, under both locks: a database that a
    // consolidation created after the caller asked for the purge is still
    // erased. When the file is absent nothing opens or creates it — and a
    // caller who supplied NO FactsStore over an existing database is
    // refused rather than reported as `skipped_no_database` (the same-root
    // contract).
    let facts_db = store.storage_path().join(mem_facts::schema::FACTS_DB_NAME);
    let has_facts_db = facts_db.exists();
    if facts.is_none() && has_facts_db {
        return Err(PurgeError::Failed(format!(
            "purge_session was given no FactsStore while the same storage root holds {}: \
             refusing to report an erasure that skips the facts tier",
            facts_db.display()
        )));
    }
    let facts = facts.filter(|_| has_facts_db);
    let (facts_report, facts_leg) = match facts {
        Some(facts) => {
            on_leg("facts");
            let report = facts
                .purge_sources(
                    &scope.project_id,
                    &scope.session_id,
                    &scope.agent_id,
                    // D12: the milestones SURVIVE (contributor scrub), so
                    // their promoted facts must survive too — passing the
                    // pre-scan ids would delete co-contributors' knowledge.
                    &[],
                )
                .await
                .map_err(|error| PurgeError::Failed(error.to_string()))?;
            (report, FactsLeg::Ran)
        }
        None => (FactsPurgeReport::default(), FactsLeg::SkippedNoDatabase),
    };

    on_leg("episodic");
    let episodic = store
        .purge_session(scope)
        .await
        .map_err(|error| PurgeError::Failed(error.to_string()))?;

    Ok(PurgeReport {
        episodic,
        facts: facts_report,
        facts_leg,
    })
}

// ---------------------------------------------------------------------------
// The CLI runner
// ---------------------------------------------------------------------------

/// Runs one purge and returns the process exit code (0/64/74/75).
///
/// Owns a current-thread runtime so `src/main.rs` stays tokio-free. The
/// storage root comes from the CLI/env resolution
/// (`HostConfig.storage_root`), NEVER from the JSONC `storagePath` key; the
/// encryption settings come from the resolved config.
pub fn run(config: &mem_core::config::Config, storage_root: &Path, request: &PurgeRequest) -> i32 {
    let mut store_config: StoreConfig = mem_store::config::from_core(config);
    store_config.storage_path = storage_root.to_path_buf();

    // 0. Validate the scope BEFORE any store is opened, any lock taken or any
    // file touched (exit 64).
    if let Err(_error) =
        SessionScope::new(&request.project_id, &request.session_id, &request.agent_id)
    {
        structured_stderr(
            "purge.invalid_scope",
            "error",
            "purge",
            serde_json::json!({ "detail_short": true }),
        );
        return 64;
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            structured_stderr(
                "purge.failed",
                "error",
                "purge",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error, "purge runtime failed");
            return 74;
        }
    };

    let outcome = runtime.block_on(async {
        let store = EpisodicStore::new(storage_root.to_path_buf(), store_config.clone());
        store
            .ready()
            .await
            .map_err(|error| PurgeError::Failed(error.to_string()))?;

        // M22: a root with no facts.db never gets one. The store is
        // constructed (it touches no disk until `ready`) and the existence
        // probe decides the leg INSIDE the locked region, where no
        // consolidation can create the file behind the purge's back.
        let facts_store = FactsStore::new(store_config);
        purge_session(&store, Some(&facts_store), request).await
    });

    match outcome {
        Ok(report) => {
            print_completed(request, &report);
            0
        }
        Err(PurgeError::InvalidScope(_)) => {
            structured_stderr(
                "purge.invalid_scope",
                "error",
                "purge",
                serde_json::json!({ "detail_short": true }),
            );
            64
        }
        Err(PurgeError::Busy) => {
            structured_stderr(
                "purge.busy",
                "warn",
                "purge",
                serde_json::json!({ "detail": "another consolidation or purge holds the locks" }),
            );
            75
        }
        Err(PurgeError::Failed(error)) => {
            structured_stderr(
                "purge.failed",
                "error",
                "purge",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error, "purge failed");
            74
        }
    }
}

/// Prints EXACTLY one stdout line on success: counts plus `not_erased`,
/// never content and never paths.
fn print_completed(request: &PurgeRequest, report: &PurgeReport) {
    let not_erased: Vec<serde_json::Value> = report
        .not_erased()
        .iter()
        .map(|entry| {
            serde_json::json!({
                "tier": entry.tier,
                "reason": entry.reason,
            })
        })
        .collect();
    let line = serde_json::json!({
        "event": "purge.completed",
        "project_id": request.project_id,
        "session_id": request.session_id,
        "agent_id": request.agent_id,
        "outcome": "erased",
        "counts": {
            "session_rows": report.episodic.session_rows,
            "raw_context": report.episodic.raw_context,
            "segments": report.episodic.segments,
            "inbox_rows": report.episodic.inbox_rows,
            "milestones_scrubbed": report.episodic.milestones_scrubbed,
            "facts": report.facts.facts,
            "fact_sources": report.facts.fact_sources,
        },
        "facts_leg": report.facts_leg.as_str(),
        "not_erased": not_erased,
    });
    println!("{line}");
}

/// Refuses a missing `--confirm` before any config load. Prints the expected
/// string (operator-supplied ids only) and exits 64.
pub fn refuse_missing_confirmation(expected: &str) -> ! {
    structured_stderr(
        "purge.confirmation_required",
        "error",
        "purge",
        serde_json::json!({ "expected": expected }),
    );
    std::process::exit(64);
}

/// Refuses a mismatched `--confirm` before any config load. Exits 64.
pub fn refuse_mismatched_confirmation(expected: &str) -> ! {
    structured_stderr(
        "purge.confirmation_mismatch",
        "error",
        "purge",
        serde_json::json!({ "expected": expected }),
    );
    std::process::exit(64);
}
