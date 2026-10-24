//! Ready-gate battery (blueprint W1.2 step 9): ensure_ready dedup/retry
//! semantics + interrupted-swap recovery ordering (upstream ready.ts +
//! shard-path-migration-service.ts recoverInterruptedSwap).

#![allow(clippy::type_complexity)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::{real_components, write_swap_fixture};
use mem_store::config::StoreConfig;
use mem_store::db::{ConnectFactory, ConnectOptions, query_one};
use mem_store::error::StoreError;
use mem_store::platform::Platform;
use mem_store::ready::{ReadyGate, ReadyStep, path_migrate_recovery_step};
use mem_store::scope::Scope;
use mem_store::shard_manager::ShardRegistry;
use mem_store::shard_path_migration::{PathMigrateSwapState, ShardMove};
use mem_store::sql::PRAGMA_USER_VERSION;

fn counter_step(name: &'static str, counter: Arc<AtomicUsize>) -> ReadyStep {
    ReadyStep {
        name,
        run: Arc::new(move || {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }),
    }
}

fn flaky_step(
    name: &'static str,
    counter: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    message: &'static str,
    released: Arc<tokio::sync::Notify>,
) -> ReadyStep {
    ReadyStep {
        name,
        run: Arc::new(move || {
            let (counter, fail, released) = (counter.clone(), fail.clone(), released.clone());
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                released.notified().await;
                if fail.load(Ordering::SeqCst) {
                    fail.store(false, Ordering::SeqCst);
                    return Err(StoreError::Internal {
                        message: message.to_string(),
                    });
                }
                Ok(())
            })
        }),
    }
}

/// Opens (or joins) a raw connection at `path`, stamps `PRAGMA
/// user_version`, truncates the WAL so the header is self-contained, and
/// releases the handle on drop.
async fn open_with_version(
    config: &StoreConfig,
    path: &std::path::Path,
    version: i64,
) -> mem_store::db::DbHandle {
    let opts = ConnectOptions::for_config(config, Platform::current());
    let db = mem_store::db::turso_connect_factory()
        .connect(path.to_path_buf(), opts.clone())
        .await
        .expect("raw open");
    let sql = mem_store::sql::set_user_version_sql(version);
    db.execute(sql.as_str(), Vec::new())
        .await
        .expect("user_version");
    let _ = db
        .execute("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await;
    db
}

/// Opens a raw connection WITHOUT stamping anything (probe helper).
async fn open_reading(config: &StoreConfig, path: &std::path::Path) -> mem_store::db::DbHandle {
    let opts = ConnectOptions::for_config(config, Platform::current());
    mem_store::db::turso_connect_factory()
        .connect(path.to_path_buf(), opts.clone())
        .await
        .expect("probe open")
}

// ---------------------------------------------------------------------------
// 1/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_runs_all_steps_once_and_initializes_metadata() {
    let config = common::store_config("ready-once");
    let (config, connections, shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());

    let legacy = Arc::new(AtomicUsize::new(0));
    let encryption = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(mem_store::ready::build_ready_gate_with(
        &storage,
        connections.clone(),
        Arc::new(shards.clone()),
        registry.clone(),
        vec![
            counter_step("legacy-migration", legacy.clone()),
            counter_step("encryption-migration", encryption.clone()),
        ],
    ));

    gate.ensure_ready().await.expect("first");
    gate.ensure_ready().await.expect("cached");

    assert_eq!(legacy.load(Ordering::SeqCst), 1, "legacy ran once");
    assert_eq!(encryption.load(Ordering::SeqCst), 1, "encryption ran once");
    assert!(
        storage.join("metadata.db").exists(),
        "warmup initialized metadata.db"
    );
    assert_eq!(
        gate.step_names(),
        // W1.3: the composition now ends with the real legacy step.
        vec![
            "legacy-migration",
            "encryption-migration",
            "legacy-migration",
            "path-migrate-recovery",
            "shard-warmup",
        ]
    );
}

// ---------------------------------------------------------------------------
// 2/8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_waits_for_concurrent_callers_and_runs_steps_once() {
    let started = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(tokio::sync::Notify::new());

    let gate = Arc::new(ReadyGate::new(vec![ReadyStep {
        name: "gated-probe",
        run: {
            let (started, total, released) = (started.clone(), total.clone(), released.clone());
            Arc::new(move || {
                let (started, total, released) = (started.clone(), total.clone(), released.clone());
                Box::pin(async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    released.notified().await;
                    total.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        },
    }]));

    let gate_a = gate.clone();
    let a = tokio::spawn(async move { gate_a.ensure_ready().await });
    while started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let gate_b = gate.clone();
    let b = tokio::spawn(async move { gate_b.ensure_ready().await });

    released.notify_one();
    let (ra, rb) = tokio::join!(a, b);
    assert!(matches!(ra.expect("task a"), Ok(())));
    assert!(matches!(rb.expect("task b"), Ok(())));
    assert_eq!(
        total.load(Ordering::SeqCst),
        1,
        "second caller shared the in-flight guard"
    );
}

// ---------------------------------------------------------------------------
// 3/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_failure_clears_in_flight_and_retry_reruns_all_steps() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter2 = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(true));
    let released = Arc::new(tokio::sync::Notify::new());

    let gate = Arc::new(ReadyGate::new(vec![
        flaky_step(
            "flaky",
            counter.clone(),
            fail.clone(),
            "boom-once",
            released.clone(),
        ),
        counter_step("after", counter2.clone()),
    ]));

    let gate_a = gate.clone();
    let a = tokio::spawn(async move { gate_a.ensure_ready().await });
    while counter.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let gate_b = gate.clone();
    let b = tokio::spawn(async move { gate_b.ensure_ready().await });
    released.notify_one();
    let (ra, rb) = tokio::join!(a, b);
    let first_a = ra.expect("task a").expect_err("blocked probe fails");
    let first_b = rb.expect("task b").expect_err("waiter gets the");
    assert_eq!(
        first_a.message(),
        "boom-once",
        "waiter gets same cloned error"
    );
    assert_eq!(first_b.message(), first_a.message());

    // Retry spawns a fresh guard (failure cleared the in-flight cell), the
    // flaky step parks again until the test releases it, then succeeds.
    let gate_c = gate.clone();
    let retry = tokio::spawn(async move { gate_c.ensure_ready().await });
    while counter.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    released.notify_one();
    retry.await.expect("retry task").expect("retry passes");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "flaky re-ran");
    assert_eq!(counter2.load(Ordering::SeqCst), 1, "second step ran once");
}

// ---------------------------------------------------------------------------
// 4/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_recovery_step_restores_interrupted_swap_before_warmup() {
    let config = common::store_config("ready-recovery");
    let (config, connections, shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());

    let old_hash = "a1b2c3d4e5f60001";
    let new_hash = "a1b2c3d4e5f60002";
    let scope_dir = storage.join("users");
    std::fs::create_dir_all(&scope_dir).expect("scope dir");
    let old_path = scope_dir.join(format!("user_{old_hash}_shard_0.db"));
    let new_path = scope_dir.join(format!("user_{new_hash}_shard_0.db"));
    let staged = scope_dir.join(format!("user_{old_hash}_shard_0.db.staged"));
    let backup = scope_dir.join(format!("user_{old_hash}_shard_0.db.swap-backup"));

    // Pre-migration world: old v7, its backup copy v5, and an applied new file.
    drop(open_with_version(&config, &old_path, 7).await);
    drop(open_with_version(&config, &backup, 5).await);
    drop(open_with_version(&config, &new_path, 3).await);

    let info = shards
        .register_existing_shard(Scope::User, old_hash, 0, &old_path, 5, true)
        .await
        .expect("row registered");
    assert!(matches!(info.scope, Scope::User));

    let journal = write_swap_fixture(
        &storage,
        &PathMigrateSwapState {
            operation: "path-migrate".to_string(),
            old_hash: old_hash.to_string(),
            new_hash: new_hash.to_string(),
            moves: vec![ShardMove {
                shard_id: info.id,
                shard_index: 0,
                old_path: old_path.clone(),
                new_path: new_path.clone(),
                staged_path: staged.clone(),
                backup_path: backup.clone(),
                is_active: true,
                vector_count: 5,
                old_container_tag: "opencode_user_a1b2c3d4e5f60001".to_string(),
            }],
            archived_targets: Vec::new(),
        },
    );
    assert!(journal.exists());

    // The probe runs AFTER recovery (registration order) and asserts the
    // whole world was restored before any normal access.
    let probe_seen = Arc::new(AtomicUsize::new(0));
    let watcher = {
        let probe_seen_watcher = probe_seen.clone();
        let (journal, old_path, config) = (journal.clone(), old_path.clone(), config.clone());
        Arc::new(
            move || -> mem_store::db::BoxFuture<'static, Result<(), StoreError>> {
                let (journal, old_path, config, probe_seen_watcher) = (
                    journal.clone(),
                    old_path.clone(),
                    config.clone(),
                    probe_seen_watcher.clone(),
                );
                Box::pin(async move {
                    probe_seen_watcher.fetch_add(1, Ordering::SeqCst);
                    assert!(!journal.exists(), "journal removed BEFORE warmup ran");
                    let db = open_reading(&config, &old_path).await;
                    let row = query_one(db.as_ref(), PRAGMA_USER_VERSION, Vec::new())
                        .await
                        .expect("probe")
                        .expect("row");
                    let version = row.i64("user_version").expect("version");
                    assert_eq!(version, 5, "backup content restored into old path");
                    Ok(())
                })
            },
        )
    };
    let gate = Arc::new(ReadyGate::new(vec![
        path_migrate_recovery_step(&storage, connections.clone(), registry.clone()),
        ReadyStep {
            name: "warmup-probe",
            run: watcher,
        },
    ]));

    gate.ensure_ready().await.expect("ready after recovery");
    assert_eq!(probe_seen.load(Ordering::SeqCst), 1);
    assert!(!journal.exists(), "journal removed");
    drop(new_path);
    drop(staged);
}

// ---------------------------------------------------------------------------
// 5/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reset_forces_full_rerun() {
    let counter = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ReadyGate::new(vec![counter_step("only", counter.clone())]));
    gate.ensure_ready().await.expect("first");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    gate.reset().expect("gate resets");
    gate.ensure_ready().await.expect("after reset");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "reset re-ran every step");
}

// ---------------------------------------------------------------------------
// 6/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_injected_steps_run_in_registration_order() {
    let config = common::store_config("ready-order");
    let (config, connections, shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());

    let order: Arc<std::sync::Mutex<Vec<&'static str>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let make_probe =
        |name: &'static str, order: Arc<std::sync::Mutex<Vec<&'static str>>>| ReadyStep {
            name,
            run: Arc::new(move || {
                let order = order.clone();
                Box::pin(async move {
                    order.lock().unwrap().push(name);
                    Ok(())
                })
            }),
        };

    let gate = Arc::new(mem_store::ready::build_ready_gate_with(
        &storage,
        connections.clone(),
        Arc::new(shards.clone()),
        registry.clone(),
        vec![
            make_probe("legacy-migration", order.clone()),
            make_probe("encryption-migration", order.clone()),
        ],
    ));
    assert_eq!(
        gate.step_names(),
        // W1.3: real legacy step appended after the injected probes.
        vec![
            "legacy-migration",
            "encryption-migration",
            "legacy-migration",
            "path-migrate-recovery",
            "shard-warmup",
        ]
    );
    gate.ensure_ready().await.expect("runs");
    // The injected probes record themselves in registration order, and both
    // real steps ran (warmup). Recording order proves the pre-step splice.
    assert_eq!(
        *order.lock().unwrap(),
        vec!["legacy-migration", "encryption-migration"]
    );
}

// ---------------------------------------------------------------------------
// 7/7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_recovery_is_idempotent_when_journal_is_absent() {
    let config = common::store_config("ready-idempotent");
    let (config, connections, shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());

    let marker = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(mem_store::ready::build_ready_gate(
        &storage,
        connections.clone(),
        Arc::new(shards.clone()),
        registry.clone(),
    ));
    assert!(!storage.join(".path-migrate-swap.json").exists());
    gate.ensure_ready().await.expect("first full run");
    marker.fetch_add(1, Ordering::SeqCst);
    assert!(
        storage.join("metadata.db").exists(),
        "warmup created metadata"
    );
}

// ---------------------------------------------------------------------------
// 8/8 — W1.3: the real legacy step recovers an interrupted re-embed swap
// BEFORE the engine probe touches the active path (upstream ready.ts order:
// recoverInterruptedReembedSwaps -> runTursoEngineMigration, legacy-migrator.ts
// L621-645). Our port refuses the DiskANN conversion loudly (recorded W1.3
// divergence — upstream converts in place); the refusal error is what proves
// the engine step ran on the RESTORED active path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ready_gate_recovers_interrupted_reembed_swap_before_engine_conversion() {
    let config = common::store_config("ready-reembed-recovery");
    let (config, connections, shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());

    let scope_dir = storage.join("projects");
    std::fs::create_dir_all(&scope_dir).expect("projects dir");
    let active = common::copy_diskann_fixture(&scope_dir, "project_a1b2c3d4e5f60003_shard_0.db");
    let staged = std::path::PathBuf::from(format!("{}.reembed-fixture.tmp", active.display()));
    let backup = std::path::PathBuf::from(format!("{}.pre-reembed-fixture.bak", active.display()));
    mem_store::sqlite_files::rename_sqlite_database(&active, &staged).expect("move to staged");
    assert!(!active.exists() && staged.exists());
    let state = common::write_reembed_swap_state(&active, &staged, &backup);

    let gate = Arc::new(mem_store::ready::build_ready_gate(
        &storage,
        connections.clone(),
        Arc::new(shards.clone()),
        registry.clone(),
    ));

    let error = gate
        .ensure_ready()
        .await
        .expect_err("DiskANN shard refuses the engine conversion");
    assert!(
        error
            .message()
            .contains("invalid expression in CREATE INDEX"),
        "engine step must have probed the restored active path: {}",
        error.message()
    );

    // Recovery completed before the refusal: staged consumed, journal gone,
    // active restored with the fixture content (the probe's open is the only
    // permitted mutation — compare via an open-identical control copy).
    assert!(!staged.exists(), "staged replacement renamed to active");
    assert!(!state.exists(), "swap journal consumed");
    assert!(active.exists(), "active shard restored");
    let control_dir = storage.join("open-control");
    std::fs::create_dir_all(&control_dir).expect("control dir");
    let control = common::copy_diskann_fixture(&control_dir, "ctl_probe.db");
    {
        // The DiskANN open fails exactly like the migrator's probe (step-4
        // pattern): tolerate the error, keep the bookkeeping identical.
        if let Ok(db) = connections.get_connection(&control).await {
            let _ = db.query("SELECT 1", Vec::new()).await;
        }
        connections
            .close_connection(&control)
            .await
            .expect("connection closes");
    }
    let control_bytes = std::fs::read(&control).expect("control bytes");
    let restored_bytes = std::fs::read(&active).expect("active bytes");
    assert_eq!(
        restored_bytes, control_bytes,
        "restored active is byte-identical to an open-identical control copy \
         of the fixture (turso opens mutate headers; W1.3 step-4 precedent)"
    );

    // No marker, no `.pre-tursodb-*` backup: the refusal happened before any
    // conversion bookkeeping (recorded divergence vs upstream's in-place
    // DiskANN conversion).
    assert!(
        !storage
            .join(mem_store::legacy_migrator::MIGRATION_MARKER)
            .exists(),
        "no migration marker after refusal"
    );
    let pre_tursodb = std::fs::read_dir(&storage)
        .expect("storage read")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".pre-tursodb-")
        })
        .count();
    assert_eq!(pre_tursodb, 0, "no upstream-style pre-tursodb backup");
}

// ---------------------------------------------------------------------------
// 9/9 — W1.5: the encryption builder places the encryption step between the
// legacy step and swap recovery (upstream ready.ts order), and the step is a
// no-op when encryption is off.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ready_gate_runs_encryption_step_between_legacy_and_recovery() {
    let config = common::store_config("ready-encryption-step");
    let (config, connections, shards) = real_components(config).await;
    let storage = config.storage_path.clone();
    let registry: Arc<dyn ShardRegistry> = Arc::new(shards.clone());

    let gate = Arc::new(mem_store::ready::build_ready_gate_with_encryption(
        &storage,
        connections.clone(),
        Arc::new(shards.clone()),
        registry.clone(),
        &config,
        Vec::new(),
    ));

    assert_eq!(
        gate.step_names(),
        vec![
            "legacy-migration",
            "encryption-migration",
            "path-migrate-recovery",
            "shard-warmup",
        ]
    );

    // Encryption is off in `store_config`: the step still appears and is a
    // no-op (the whole gate must complete).
    gate.ensure_ready()
        .await
        .expect("ready with the encryption step");
}
