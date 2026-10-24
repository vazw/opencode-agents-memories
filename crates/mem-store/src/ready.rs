//! Turso ready gate — port of upstream `src/services/turso/ready.ts`
//! (blueprint W1.2 step 9, W1.3 step 6 completes the composition).
//!
//! Upstream order: legacy migration (which itself recovers interrupted
//! re-embed swaps, then runs the engine migration — legacy-migrator.ts
//! L621-645), interrupted path-migrate swap recovery, shard warmup
//! (`getAllShards("user", "")`). W1.5 owns the encryption slot directly via
//! `build_ready_gate_with_encryption` (no injection via pre-steps); there are
//! deliberately NO placeholder no-op steps — a silent no-op would fake
//! "migrated" forever).
//!
//! Concurrency model: upstream `initPromise`/`isReady` become a shared
//! `OnceCell<Result>` installed under the gate state — concurrent waiters
//! join the in-flight future (same dedup pattern as the connection
//! manager, step 4). Failure clears both the cell and `ready`, so a retry
//! re-runs EVERY step (upstream `initPromise = null` in the catch).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::OnceCell;

use crate::connection::ConnectionManager;
use crate::db::BoxFuture;
use crate::error::StoreError;
use crate::scope::Scope;
use crate::shard_manager::{ShardManager, ShardRegistry};

type StepRun = Arc<dyn Fn() -> BoxFuture<'static, Result<(), StoreError>> + Send + Sync>;

/// One readiness step; `run` yields a FRESH future per call so a retry
/// re-executes it (upstream re-entrancy).
pub struct ReadyStep {
    pub name: &'static str,
    pub run: StepRun,
}

type InitCell = OnceCell<Result<(), StoreError>>;

#[derive(Default)]
struct GateState {
    ready: bool,
    in_flight: Option<Arc<InitCell>>,
}

/// The gate.
pub struct ReadyGate {
    steps: Vec<ReadyStep>,
    state: Mutex<GateState>,
}

impl ReadyGate {
    pub fn new(steps: Vec<ReadyStep>) -> Self {
        Self {
            steps,
            state: Mutex::new(GateState::default()),
        }
    }

    /// Upstream `ensureTursoReady`.
    pub async fn ensure_ready(&self) -> Result<(), StoreError> {
        // GateState { ready, in_flight }: cross-field; a stale ready skips every migration step (ADR-0013 D-2 D2-a)
        if self
            .state
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "state",
                operation: "ensure_ready",
            })?
            .ready
        {
            return Ok(());
        }
        let cell = {
            // GateState: claim-or-install the init cell; a cell whose initializer panicked never completes (ADR-0013 D-2 D2-a)
            let mut state = self.state.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "state",
                operation: "ensure_ready",
            })?;
            if state.ready {
                return Ok(());
            }
            match state.in_flight.as_ref() {
                Some(cell) => Arc::clone(cell),
                None => {
                    let cell = Arc::new(InitCell::new());
                    state.in_flight = Some(cell.clone());
                    cell
                }
            }
        };
        let result: Result<(), StoreError> = cell.get_or_init(|| self.run_steps()).await.clone();
        if result.is_ok() {
            // GateState: ready = true then in_flight = None — the pair is the invariant (ADR-0013 D-2 D2-a)
            let mut state = self.state.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "state",
                operation: "ensure_ready",
            })?;
            state.ready = true;
            state.in_flight = None;
        } else {
            // GateState: clear in_flight after a failed init so a retry re-runs every step (ADR-0013 D-2 D2-a)
            let mut state = self.state.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "state",
                operation: "ensure_ready",
            })?;
            state.in_flight = None;
        }
        result
    }

    async fn run_steps(&self) -> Result<(), StoreError> {
        for step in &self.steps {
            (step.run)().await?;
        }
        Ok(())
    }

    /// Upstream `resetTursoReady`.
    pub fn reset(&self) -> Result<(), StoreError> {
        // GateState: reset() clears both fields — the gate that decides whether the steps run again (ADR-0013 D-2 D2-c)
        let mut state = self.state.lock().map_err(|_| StoreError::LockPoisoned {
            lock: "state",
            operation: "reset",
        })?;
        state.ready = false;
        state.in_flight = None;
        Ok(())
    }

    /// Registered step names in execution order.
    pub fn step_names(&self) -> Vec<&'static str> {
        self.steps.iter().map(|step| step.name).collect()
    }
}

/// Upstream legacy-migration half of the ready pipeline (W1.3 step 6):
/// `recoverInterruptedReembedSwaps → runTursoEngineMigration → marker /
/// lock / migrate / reconcile` via [`crate::legacy_migrator`] — the two
/// upstream `ready.ts` calls (`ensureTursoMigrated` + the legacy sweep)
/// collapse into the single ported entry point.
pub fn legacy_migration_step(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
) -> ReadyStep {
    let storage: PathBuf = storage_path.to_path_buf();
    ReadyStep {
        name: "legacy-migration",
        run: Arc::new(move || {
            let (storage, connections, shards) =
                (storage.clone(), connections.clone(), shards.clone());
            Box::pin(async move {
                let sleep = Arc::new(crate::platform::system_sleep) as crate::platform::SleepFn;
                crate::legacy_migrator::run_legacy_turso_migration(
                    &storage,
                    &shards,
                    &connections,
                    &sleep,
                )
                .await
            })
        }),
    }
}

/// Upstream `shardPathMigrationService.recoverInterruptedSwap()` step.
pub fn path_migrate_recovery_step(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    registry: Arc<dyn ShardRegistry>,
) -> ReadyStep {
    let storage: PathBuf = storage_path.to_path_buf();
    ReadyStep {
        name: "path-migrate-recovery",
        run: Arc::new(move || {
            let (storage, connections, registry) =
                (storage.clone(), connections.clone(), registry.clone());
            Box::pin(async move {
                crate::shard_path_migration::recover_interrupted_swap(
                    &storage,
                    &connections,
                    registry.as_ref(),
                )
                .await
            })
        }),
    }
}

/// Upstream `tursoShardManager.getAllShards("user", "")` warmup step.
pub fn shard_warmup_step(shards: Arc<ShardManager>) -> ReadyStep {
    ReadyStep {
        name: "shard-warmup",
        run: Arc::new(move || {
            let shards = shards.clone();
            Box::pin(async move {
                // The warmup has its own side effect upstream: it forces
                // the metadata db + shard registry open/initialized.
                shards.get_all_shards(Scope::User, None).await?;
                Ok(())
            })
        }),
    }
}

/// Upstream composition WITHOUT the encryption slot: injected pre-steps
/// (none) ++ legacy ++ recovery ++ warmup. Encryption is deliberately not
/// wired here — use [`build_ready_gate_with_encryption`] for encrypted
/// configs; this builder stays for W1.3 compatibility and unencrypted
/// defaults.
pub fn build_ready_gate(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    registry: Arc<dyn ShardRegistry>,
) -> ReadyGate {
    build_ready_gate_with(storage_path, connections, shards, registry, Vec::new())
}

/// Composition with injected PRE steps. The encryption slot is NOT part of
/// this list — [`build_ready_gate_with_encryption`] supersedes this builder
/// for encrypted configs (it appends the encryption step between legacy and
/// recovery itself).
pub fn build_ready_gate_with(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    registry: Arc<dyn ShardRegistry>,
    pre_steps: Vec<ReadyStep>,
) -> ReadyGate {
    let mut steps = pre_steps;
    // Upstream order (ready.ts + legacy-migrator.ts L621-645): the legacy
    // step internally recovers re-embed swaps and runs the engine
    // migration before touching any legacy shard.
    steps.push(legacy_migration_step(
        storage_path,
        connections.clone(),
        shards.clone(),
    ));
    steps.push(path_migrate_recovery_step(
        storage_path,
        connections,
        registry,
    ));
    steps.push(shard_warmup_step(shards));
    ReadyGate::new(steps)
}

/// Composition with the encryption slot wired in (blueprint W1.5): injected
/// PRE steps (ADDITIONAL steps only — never the standard ones) ++ legacy ++
/// encryption ++ recovery ++ warmup.
///
/// CONTRACT (pinned by the blueprint): this builder ALWAYS appends the four
/// standard steps itself, so callers must NOT re-add any of them through
/// `pre_steps` — that would double-add relative to [`build_ready_gate_with`],
/// which also appends legacy/recovery/warmup. It supersedes
/// [`build_ready_gate_with`] for encrypted configs; `build_ready_gate_with`
/// stays for W1.3 compatibility and unencrypted defaults.
pub fn build_ready_gate_with_encryption(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    registry: Arc<dyn ShardRegistry>,
    config: &crate::config::StoreConfig,
    pre_steps: Vec<ReadyStep>,
) -> ReadyGate {
    let mut steps = pre_steps;
    steps.push(legacy_migration_step(
        storage_path,
        connections.clone(),
        shards.clone(),
    ));
    // The encryption slot sits between the legacy step and swap recovery,
    // exactly as upstream `ready.ts` orders it.
    steps.push(crate::encryption_migrator::encryption_migration_step(
        storage_path,
        connections.clone(),
        shards.clone(),
        config,
    ));
    steps.push(path_migrate_recovery_step(
        storage_path,
        connections,
        registry,
    ));
    steps.push(shard_warmup_step(shards));
    ReadyGate::new(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[tokio::test]
    async fn test_ready_gate_propagates_poisoned_state_lock() {
        let gate = ReadyGate::new(Vec::new());
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = gate.state.lock().expect("fresh lock");
            panic!("poison");
        }));
        assert!(gate.state.is_poisoned(), "state lock must be poisoned");

        let error = gate
            .ensure_ready()
            .await
            .expect_err("a poisoned gate must refuse to claim readiness");
        assert!(matches!(
            error,
            StoreError::LockPoisoned {
                lock: "state",
                operation: "ensure_ready",
            }
        ));
    }
}
