//! Turso ready gate — port of upstream `src/services/turso/ready.ts`
//! (blueprint W1.2 step 9).
//!
//! Upstream order: legacy migration, encryption migration, interrupted
//! swap recovery, shard warmup (`getAllShards("user", "")`). W1.2 ships
//! the recovery + warmup steps only: the legacy/encryption steps are W1.5
//! wiring and there are deliberately NO placeholder no-op steps (a silent
//! no-op would fake "migrated" forever).
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
        if self.state.lock().unwrap().ready {
            return Ok(());
        }
        let cell = {
            let mut state = self.state.lock().unwrap();
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
            let mut state = self.state.lock().unwrap();
            state.ready = true;
            state.in_flight = None;
        } else {
            let mut state = self.state.lock().unwrap();
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
    pub fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        state.ready = false;
        state.in_flight = None;
    }

    /// Registered step names in execution order.
    pub fn step_names(&self) -> Vec<&'static str> {
        self.steps.iter().map(|step| step.name).collect()
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

/// Upstream composition (legacy/encryption steps arrive in W1.5).
pub fn build_ready_gate(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    registry: Arc<dyn ShardRegistry>,
) -> ReadyGate {
    build_ready_gate_with(storage_path, connections, shards, registry, Vec::new())
}

/// Composition with injected PRE steps (W1.3/W1.5 injection point).
pub fn build_ready_gate_with(
    storage_path: &Path,
    connections: Arc<ConnectionManager>,
    shards: Arc<ShardManager>,
    registry: Arc<dyn ShardRegistry>,
    pre_steps: Vec<ReadyStep>,
) -> ReadyGate {
    let mut steps = pre_steps;
    steps.push(path_migrate_recovery_step(
        storage_path,
        connections,
        registry,
    ));
    steps.push(shard_warmup_step(shards));
    ReadyGate::new(steps)
}
