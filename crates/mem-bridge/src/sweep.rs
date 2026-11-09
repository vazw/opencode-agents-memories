//! Issue 43 W6c — the erasure sweep (ADR-0019 D-19-5/D-19-6).
//!
//! The sweep compacts and reaps; it never selects a row. It:
//!
//! 1. takes the storage root's **engine lock** (a live engine ⇒ clean exit 75,
//!    and `.bak` reaping needs the `ExclusiveRoot` proof the lock mints);
//! 2. refuses under a live **pre-drop holder** ([`mem_store::sweep_probe`]:
//!    a live `multiprocess` `.wal-mode` marker, or a held `-tshm` sensor);
//! 3. per target — `user-prompts.db`, `user-profiles.db`, `episodic.db` — holds
//!    the turso operation lock, checkpoints (`TRUNCATE`), `VACUUM`s and stamps
//!    `sweep_state`, in that order, releasing the lock only after the stamp;
//! 4. reaps the six `.bak` families under the exclusive root;
//! 5. prints one `sweep.completed` line and exits 0.
//!
//! A failing target is a **named residual** in `vacuums_failed`, never a global
//! abort: a starved sweep delays physical erasure, it cannot corrupt
//! (D-19-5). Exit codes follow the purge table: `0` swept · `64` usage ·
//! `74` store-or-IO · `75` busy/blocked.
//!
//! The vault, the host transcript, logs, exports, media, `facts.db` and the
//! memories shards are outside the sweep by construction; `not_erased` names
//! the two tiers that survive a purge so the report is never readable as a
//! complete erasure.

use std::path::Path;
use std::sync::Arc;

use mem_episodic::store::EPISODIC_DB_NAME;
use mem_profile::profile_store::USER_PROFILES_DB_NAME;
use mem_profile::prompt::USER_PROMPTS_DB_NAME;
use mem_store::config::StoreConfig;
use mem_store::db::{
    ConnectFactory as _, ConnectOptions, Db, Value, resolve_database_encryption,
    turso_connect_factory,
};
use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
use mem_store::operation_lock::acquire_turso_operation_lock;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::sweep_probe::{SweepPreflightBlock, sweep_preflight_database};

use crate::engine::structured_stderr;
use crate::purge::{NOT_ERASED, NotErasedTier};

/// The operation-lock label every target's critical section records.
const OPERATION_LABEL: &str = "erasure-sweep";

/// One database in the sweep set (D-19-6 order: prompts, profiles, episodic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// `user-prompts.db` — the prompt history.
    UserPrompts,
    /// `user-profiles.db` — the database the watermark scrub wrote to.
    UserProfiles,
    /// `episodic.db` — the milestone contributor rewrite.
    Episodic,
}

impl Tier {
    /// The three tiers, in sweep order.
    pub const ALL: [Tier; 3] = [Tier::UserPrompts, Tier::UserProfiles, Tier::Episodic];

    /// The label the report and the `sweep_state` row use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserPrompts => "user-prompts",
            Self::UserProfiles => "user-profiles",
            Self::Episodic => "episodic",
        }
    }

    /// The database file name inside the storage root.
    #[must_use]
    pub fn db_name(self) -> &'static str {
        match self {
            Self::UserPrompts => USER_PROMPTS_DB_NAME,
            Self::UserProfiles => USER_PROFILES_DB_NAME,
            Self::Episodic => EPISODIC_DB_NAME,
        }
    }
}

/// Why one target did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepFailReason {
    /// The operation lock is held by another operation.
    Busy,
    /// The target could not be opened through `for_sweep`.
    Open,
    /// The WAL checkpoint failed.
    Checkpoint,
    /// `VACUUM` failed (or the milestone count could not be read).
    Vacuum,
    /// The sweep stamp could not be written.
    Stamp,
}

impl SweepFailReason {
    /// The label the report uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Open => "open",
            Self::Checkpoint => "checkpoint",
            Self::Vacuum => "vacuum",
            Self::Stamp => "stamp",
        }
    }
}

/// The CLI request. The sweep set is code-defined, so `apply` is the only
/// field: the sweep mutates, so `--apply` is mandatory (the reap precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepRequest {
    /// Whether the operator confirmed the mutation (`--apply`).
    pub apply: bool,
}

/// What one sweep did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Targets checkpointed, vacuumed and stamped.
    pub vacuumed: Vec<Tier>,
    /// Targets that failed, with the typed reason (a named residual).
    pub vacuums_failed: Vec<(Tier, SweepFailReason)>,
    /// `.bak` files removed under the exclusive root.
    pub baks_reaped: u64,
    /// Their total size before removal.
    pub baks_bytes: u64,
    /// The tiers a purge deliberately leaves (the purge's own list).
    pub not_erased: &'static [NotErasedTier],
    /// Targets whose `sweep_state` row was written.
    pub stamped: Vec<Tier>,
    /// Milestones whose `contributors` already carry a scrubbed contributor.
    pub milestone_compacted: u64,
}

impl Default for SweepReport {
    fn default() -> Self {
        Self {
            vacuumed: Vec::new(),
            vacuums_failed: Vec::new(),
            baks_reaped: 0,
            baks_bytes: 0,
            not_erased: &NOT_ERASED,
            stamped: Vec::new(),
            milestone_compacted: 0,
        }
    }
}

impl SweepReport {
    /// The tiers a sweep deliberately does not erase.
    #[must_use]
    pub fn not_erased(&self) -> &'static [NotErasedTier] {
        self.not_erased
    }
}

/// Why a sweep did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepError {
    /// `--apply` is missing.
    Usage,
    /// A live engine (or a live operation) holds the root.
    Busy,
    /// A live pre-drop holder could be running, or the sensor could not answer.
    Blocked(SweepPreflightBlock),
    /// Store-or-IO failure outside a single target.
    Failed(String),
}

/// Runs one sweep and returns the process exit code (0/64/74/75).
///
/// Owns a current-thread runtime so `src/main.rs` stays tokio-free; the storage
/// root comes from the CLI/env resolution, never from the JSONC key.
pub fn run(config: &mem_core::config::Config, storage_root: &Path, request: &SweepRequest) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            structured_stderr(
                "sweep.failed",
                "error",
                "sweep",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error, "sweep runtime failed");
            return 74;
        }
    };

    let outcome = runtime.block_on(sweep_observed(config, storage_root, request, &|_| {}));
    match outcome {
        Ok(report) => {
            print_completed(&report);
            0
        }
        Err(SweepError::Usage) => {
            structured_stderr(
                "sweep.usage",
                "error",
                "sweep",
                serde_json::json!({ "detail": "--apply is required; the sweep mutates" }),
            );
            64
        }
        Err(SweepError::Busy) => {
            structured_stderr(
                "sweep.busy",
                "warn",
                "sweep",
                serde_json::json!({
                    "detail": "a live engine holds this storage root; stop it and re-run"
                }),
            );
            75
        }
        Err(SweepError::Blocked(block)) => {
            let reason = match block {
                SweepPreflightBlock::LegacyPeerLive => "legacy-peer-live",
                SweepPreflightBlock::ProbeUnknown { .. } => "probe-unknown",
            };
            structured_stderr(
                "sweep.blocked",
                "warn",
                "sweep",
                serde_json::json!({ "reason": reason }),
            );
            75
        }
        Err(SweepError::Failed(error)) => {
            structured_stderr(
                "sweep.failed",
                "error",
                "sweep",
                serde_json::json!({ "detail_short": true }),
            );
            tracing::warn!(error = %error, "sweep failed");
            74
        }
    }
}

/// The test/diagnostic seam: records each executed phase in order
/// (`"locked"`, `"checkpointed"`, `"vacuumed"`, `"released"`, repeated per
/// target) — the seam that makes the held-through-`VACUUM` invariant
/// deterministic.
#[doc(hidden)]
pub async fn sweep_observed(
    config: &mem_core::config::Config,
    storage_root: &Path,
    request: &SweepRequest,
    on_phase: &dyn Fn(&'static str),
) -> Result<SweepReport, SweepError> {
    if !request.apply {
        return Err(SweepError::Usage);
    }

    let mut store_config: StoreConfig = mem_store::config::from_core(config);
    store_config.storage_path = storage_root.to_path_buf();

    // 1. The engine lock: a live engine is a clean 75, and `.bak` reaping needs
    //    the `ExclusiveRoot` proof this lock registers.
    let engine_lock = match acquire_engine_lock(storage_root, Arc::new(SystemProcessProbe)) {
        Ok(EngineLockOutcome::Acquired(lock)) => lock,
        Ok(EngineLockOutcome::Lost { .. }) => return Err(SweepError::Busy),
        Err(error) => return Err(SweepError::Failed(error.to_string())),
    };

    // 2. The preflight cross-check, before any target is opened or mutated.
    for tier in Tier::ALL {
        let db_path = storage_root.join(tier.db_name());
        if !db_path.is_file() {
            continue;
        }
        if let Err(block) = sweep_preflight_database(storage_root, &db_path) {
            return Err(SweepError::Blocked(block));
        }
    }

    // 3. Resolve the key material ONCE: a missing key is a store error (74),
    //    never a per-target `open` residual that hides a config failure.
    let encryption = resolve_database_encryption(&store_config)
        .map_err(|error| SweepError::Failed(error.to_string()))?;

    let mut report = SweepReport::default();

    // 4. Per target, in D-19-6 order.
    for tier in Tier::ALL {
        let db_path = storage_root.join(tier.db_name());
        if !db_path.is_file() {
            continue;
        }
        if let Err(reason) = sweep_target(
            &store_config,
            storage_root,
            tier,
            &db_path,
            encryption.clone(),
            &mut report,
            on_phase,
        )
        .await
        {
            report.vacuums_failed.push((tier, reason));
        }
    }

    // 5. The `.bak` families, under the engine lock's exclusive proof.
    let reap = mem_store::bak_reaping::reap_backup_files(storage_root)
        .map_err(|error| SweepError::Failed(error.to_string()))?;
    report.baks_reaped = reap.reaped;
    report.baks_bytes = reap.bytes;

    drop(engine_lock);
    Ok(report)
}

/// One target's critical section: operation lock → open → checkpoint →
/// `VACUUM` → stamp → release. The lock is released only after the stamp, so a
/// second sweep cannot interleave between the compaction and the stamp.
async fn sweep_target(
    store_config: &StoreConfig,
    storage_root: &Path,
    tier: Tier,
    db_path: &Path,
    encryption: Option<mem_store::db::EncryptionOptions>,
    report: &mut SweepReport,
    on_phase: &dyn Fn(&'static str),
) -> Result<(), SweepFailReason> {
    let lock =
        acquire_turso_operation_lock(storage_root, Arc::new(SystemProcessProbe), OPERATION_LABEL)
            .map_err(|_| SweepFailReason::Busy)?;
    on_phase("locked");

    let mut opts = ConnectOptions::for_sweep(store_config, Platform::current());
    opts.encryption = encryption;
    let db = turso_connect_factory()
        .connect(db_path.to_path_buf(), opts)
        .await
        .map_err(|_| SweepFailReason::Open)?;

    // The episodic leg additionally counts the milestones whose contributors
    // already carry a scrubbed contributor — before VACUUM, in the same lock
    // scope. `VACUUM` does the physical rewrite (D-19-5: no row selection).
    if tier == Tier::Episodic {
        let exists = mem_episodic::milestone_compaction::milestone_table_exists(db.as_ref())
            .await
            .map_err(|_| SweepFailReason::Checkpoint)?;
        if exists {
            report.milestone_compacted = report.milestone_compacted.saturating_add(
                mem_episodic::milestone_compaction::count_scrubbed_milestones(db.as_ref())
                    .await
                    .map_err(|_| SweepFailReason::Checkpoint)?,
            );
        }
    }

    // `PRAGMA wal_checkpoint(TRUNCATE)` returns a row, so it is a query.
    db.query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await
        .map_err(|_| SweepFailReason::Checkpoint)?;
    on_phase("checkpointed");

    db.execute("VACUUM", Vec::new())
        .await
        .map_err(|_| SweepFailReason::Vacuum)?;
    on_phase("vacuumed");

    stamp(db.as_ref(), tier)
        .await
        .map_err(|_| SweepFailReason::Stamp)?;
    report.stamped.push(tier);

    drop(db);
    lock.release();
    on_phase("released");
    report.vacuumed.push(tier);
    Ok(())
}

/// Writes the tier's `sweep_state` row inside the target's lock scope.
///
/// The migration creates the table; the idempotent `CREATE TABLE IF NOT EXISTS`
/// here keeps the stamp truthful for a database written by a build that has
/// not run the new migration leg yet (no schema-version change: plain DDL).
async fn stamp(db: &dyn Db, tier: Tier) -> Result<(), mem_store::error::StoreError> {
    db.execute(mem_store::schema_migrations::SWEEP_STATE_DDL, Vec::new())
        .await?;
    db.execute(
        "INSERT OR REPLACE INTO sweep_state (tier, last_swept_unix_ms) VALUES (?, ?)",
        vec![
            Value::Text(tier.as_str().to_string()),
            Value::Integer(now_unix_millis()),
        ],
    )
    .await?;
    Ok(())
}

fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Prints EXACTLY one stdout line on success: labels and counts, never content
/// and never paths (the `purge.completed` grammar).
fn print_completed(report: &SweepReport) {
    let labels =
        |tiers: &[Tier]| -> Vec<&'static str> { tiers.iter().map(|tier| tier.as_str()).collect() };
    let failures: Vec<serde_json::Value> = report
        .vacuums_failed
        .iter()
        .map(|(tier, reason)| {
            serde_json::json!({ "tier": tier.as_str(), "reason": reason.as_str() })
        })
        .collect();
    let not_erased: Vec<serde_json::Value> = report
        .not_erased()
        .iter()
        .map(|entry| serde_json::json!({ "tier": entry.tier, "reason": entry.reason }))
        .collect();
    let line = serde_json::json!({
        "event": "sweep.completed",
        "outcome": "swept",
        "vacuumed": labels(&report.vacuumed),
        "vacuums_failed": failures,
        "stamped": labels(&report.stamped),
        "baks_reaped": report.baks_reaped,
        "baks_bytes": report.baks_bytes,
        "milestone_compacted": report.milestone_compacted,
        "not_erased": not_erased,
    });
    println!("{line}");
}
