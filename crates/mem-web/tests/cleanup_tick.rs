//! Port-level oracles for the capture-triggered cleanup tick (issue 41).
//!
//! Every case drives `MaintenancePort::cleanup_tick` / `cleanup_status` through
//! the production adapter over a real fixture store and a temp sidecar, so the
//! gate, the sidecar seeding and the typed `Busy` arm are exercised where the
//! engine will run them.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::real_store::{RealStore, create_shard, fresh_manager, new_store, row_ids};
use mem_core::scope::Scope;
use mem_profile::prompt::UserPromptManager;
use mem_store::db::Value;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};
use mem_web::api::cleanup_sweep::{cleanup_sweep_path, load_stamp, save_stamp};
use mem_web::api::deduplication::{CleanupSweepConfig, production_deduplication_port_with_cleanup};
use mem_web::api::ports::{ApiServices, CleanupTick, MaintenancePort, PortError};

const PROJ_HASH: &str = "fedcba9876543210";
const PROJ_TAG: &str = "opencode_project_fedcba9876543210";
const RETENTION_DAYS: u32 = 30;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

async fn insert_memory(manager: &ShardManager, shard: &ShardInfo, id: &str, updated_at: i64) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    vector::insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: id.to_string(),
            content: format!("row {id}"),
            vector: vec![0.0; 768],
            tags_vector: None,
            container_tag: PROJ_TAG.to_string(),
            tags: None,
            memory_type: None,
            created_at: updated_at,
            updated_at,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
            source_prompt_created_at: None,
        },
    )
    .await
    .expect("insert vector");
    let count = mem_store::shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("set vector count");
}

async fn insert_expired_prompt(prompts: &UserPromptManager, id: &str, created_at: i64) {
    let db = prompts.ready().await.expect("prompts ready");
    db.execute(
        "INSERT INTO user_prompts (id, session_id, message_id, project_path, content, created_at, linked_memory_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text("sess-1".to_string()),
            Value::Text("msg-1".to_string()),
            Value::Text("/project".to_string()),
            Value::Text("prompt".to_string()),
            Value::Integer(created_at),
            Value::Null,
        ],
    )
    .await
    .expect("insert prompt");
}

fn config(enabled: bool, interval_seconds: u64, path: &Path) -> CleanupSweepConfig {
    CleanupSweepConfig {
        enabled,
        interval_seconds,
        sweep_path: path.to_path_buf(),
    }
}

fn port(
    manager: ShardManager,
    prompts: Arc<UserPromptManager>,
    cleanup: CleanupSweepConfig,
) -> Arc<dyn MaintenancePort> {
    production_deduplication_port_with_cleanup(
        manager,
        prompts,
        RETENTION_DAYS,
        true,
        0.9,
        cleanup,
        Arc::new(AtomicBool::new(false)),
        None,
    )
}

/// One expired row in one project shard; returns the shard and the store.
async fn expired_fixture(label: &str, memory_id: &str) -> (RealStore, ShardInfo, i64) {
    let store = new_store(label);
    let shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let cutoff = now - i64::from(RETENTION_DAYS) * 86_400_000;
    insert_memory(&store.manager, &shard, memory_id, cutoff - 10_000).await;
    (store, shard, now)
}

fn prompts_for(store: &RealStore) -> Arc<UserPromptManager> {
    Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ))
}

fn sweep_path(store: &RealStore) -> PathBuf {
    cleanup_sweep_path(&store.config.storage_path)
}

// ---------------------------------------------------------------------------
// Step 5: the degraded composition answers Unavailable on the new surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unavailable_port_answers_unavailable_for_the_cleanup_surface() {
    let services = ApiServices::unavailable();
    assert!(
        matches!(
            services.maintenance.cleanup_status().await,
            Err(PortError::Unavailable)
        ),
        "an unavailable port must not invent a cleanup status"
    );
    assert!(
        matches!(
            services.maintenance.cleanup_tick(now_millis()).await,
            Err(PortError::Unavailable)
        ),
        "an unavailable port must answer Unavailable, never Disabled"
    );
}

// ---------------------------------------------------------------------------
// Step 6: the port-level tick oracles
// ---------------------------------------------------------------------------

/// Two ticks one second apart: the first sweeps, the second is silent.
///
/// The distinguishing input is the row planted BETWEEN the calls: it survives,
/// so an in-memory gate that was never set reds here (the sweep would delete it).
#[tokio::test]
async fn test_due_tick_runs_once_then_not_due() {
    let (store, shard, now) = expired_fixture("cleanup_tick_due_once", "mem-idle-1").await;
    let prompts = prompts_for(&store);
    let cutoff = now - i64::from(RETENTION_DAYS) * 86_400_000;
    insert_expired_prompt(&prompts, "prompt-1", cutoff - 10_000).await;

    let port = port(
        store.manager.clone(),
        Arc::clone(&prompts),
        config(true, 86_400, &sweep_path(&store)),
    );

    match port.cleanup_tick(now).await.expect("tick 1") {
        CleanupTick::Ran(report) => {
            assert_eq!(
                report.deleted_count, 1,
                "the first tick sweeps the idle row"
            );
            assert_eq!(report.prompts_deleted, 1, "and the expired prompt");
        }
        other => panic!("the first tick must run, got {other:?}"),
    }

    // Plant one more expired row: the second tick must not touch it.
    insert_memory(&store.manager, &shard, "mem-idle-2", cutoff - 10_000).await;
    assert_eq!(
        port.cleanup_tick(now + 1_000).await.expect("tick 2"),
        CleanupTick::NotDue,
        "a tick inside the interval is silent"
    );
    assert!(
        row_ids(&store, &shard)
            .await
            .contains(&"mem-idle-2".to_string()),
        "the not-due tick must leave the planted row alive"
    );
}

/// The gate's durability channel: a new port over the same root is not due.
#[tokio::test]
async fn test_restart_inside_interval_is_not_due() {
    let (store, _shard, now) = expired_fixture("cleanup_tick_restart", "mem-restart").await;
    let prompts = prompts_for(&store);
    let path = sweep_path(&store);

    let first = port(
        store.manager.clone(),
        Arc::clone(&prompts),
        config(true, 86_400, &path),
    );
    assert!(
        matches!(
            first.cleanup_tick(now).await.expect("tick before restart"),
            CleanupTick::Ran(_)
        ),
        "the first engine must sweep"
    );
    drop(first);

    let restarted = port(
        fresh_manager(&store.config),
        Arc::clone(&prompts),
        config(true, 86_400, &path),
    );
    assert_eq!(
        restarted
            .cleanup_tick(now + 60_000)
            .await
            .expect("tick after restart"),
        CleanupTick::NotDue,
        "the sidecar, not just memory, carries the gate across a restart"
    );
}

/// `enabled:false` silences the tick and nothing else.
#[tokio::test]
async fn test_disabled_tick_never_runs_but_manual_cleanup_does() {
    let (store, shard, now) = expired_fixture("cleanup_tick_disabled", "mem-disabled").await;
    let prompts = prompts_for(&store);
    let port = port(
        store.manager.clone(),
        Arc::clone(&prompts),
        config(false, 86_400, &sweep_path(&store)),
    );

    assert_eq!(
        port.cleanup_tick(now).await.expect("disabled tick"),
        CleanupTick::Disabled
    );
    assert!(
        row_ids(&store, &shard)
            .await
            .contains(&"mem-disabled".to_string()),
        "a disabled tick deletes nothing"
    );

    let report = port
        .cleanup()
        .await
        .expect("the manual endpoint still runs");
    assert_eq!(
        report.deleted_count, 1,
        "the port is wired; only the tick is gated"
    );
}

/// A tick that loses the guard answers `Busy` and writes no stamp.
#[tokio::test]
async fn test_tick_losing_the_guard_is_busy_and_does_not_stamp() {
    let (store, _shard, now) = expired_fixture("cleanup_tick_busy", "mem-busy").await;
    let prompts = prompts_for(&store);
    let path = sweep_path(&store);
    let port = port(
        store.manager.clone(),
        Arc::clone(&prompts),
        config(true, 86_400, &path),
    );

    let (cleanup, tick) = tokio::join!(port.cleanup(), port.cleanup_tick(now));
    assert!(cleanup.is_ok(), "the manual run must win the guard");
    assert_eq!(
        tick.expect("the tick answers, never hangs"),
        CleanupTick::Busy,
        "the tick must learn Busy from the typed guard, not from a message"
    );
    assert_eq!(
        load_stamp(&path).expect("the sidecar reads"),
        None,
        "a Busy tick must not advance the stamp"
    );
}

/// A failed sweep leaves the stamp retryable: the next tick runs instead of
/// answering `NotDue`.
#[tokio::test]
async fn test_failed_tick_leaves_the_stamp_retryable() {
    let (store, shard, now) = expired_fixture("cleanup_tick_failed", "mem-failed").await;
    let prompts = prompts_for(&store);
    let path = sweep_path(&store);
    // A fresh manager: a cached shard handle would keep reading the unreadable
    // file and mask the failure.
    let port = port(
        fresh_manager(&store.config),
        Arc::clone(&prompts),
        config(true, 86_400, &path),
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shard.db_path, std::fs::Permissions::from_mode(0o000))
            .expect("the shard file is made unreadable");

        let failed = port.cleanup_tick(now).await;
        assert!(
            matches!(failed, Err(PortError::Store(_))),
            "an unreadable shard must surface as a store error, got {failed:?}"
        );
        assert_eq!(
            load_stamp(&path).expect("the sidecar reads"),
            None,
            "a failed sweep must not write the stamp"
        );

        std::fs::set_permissions(&shard.db_path, std::fs::Permissions::from_mode(0o644))
            .expect("the shard file is repaired");

        match port
            .cleanup_tick(now + 1_000)
            .await
            .expect("the retry runs")
        {
            CleanupTick::Ran(report) => {
                assert_eq!(
                    report.deleted_count, 1,
                    "the retry sweeps the row it could not read"
                )
            }
            other => panic!("the retry must run, not answer {other:?}"),
        }
    }
}

/// A stamp in the future is corrupt evidence, and corrupt evidence is due (C-2).
#[tokio::test]
async fn test_future_stamp_is_due() {
    let (store, _shard, now) = expired_fixture("cleanup_tick_future", "mem-future").await;
    let prompts = prompts_for(&store);
    let path = sweep_path(&store);
    save_stamp(&path, now + 365 * 86_400_000).expect("the future stamp is written");

    let port = port(
        store.manager.clone(),
        Arc::clone(&prompts),
        config(true, 86_400, &path),
    );
    assert!(
        matches!(
            port.cleanup_tick(now).await.expect("the tick runs"),
            CleanupTick::Ran(_)
        ),
        "a stamp in the future must be treated as due, never as 'not due forever'"
    );
}

/// The four facts, each from its own source: config, sidecar seed, live guard.
#[tokio::test]
async fn test_status_reports_the_four_fields() {
    let (store, _shard, now) = expired_fixture("cleanup_tick_status", "mem-status").await;
    let prompts = prompts_for(&store);
    let path = sweep_path(&store);
    let seeded = now - 3_600_000;
    save_stamp(&path, seeded).expect("the seed stamp is written");

    let port = port(
        store.manager.clone(),
        Arc::clone(&prompts),
        config(true, 7_200, &path),
    );

    let (cleanup, status) = tokio::join!(port.cleanup(), port.cleanup_status());
    assert!(cleanup.is_ok(), "the manual run succeeds");
    let status = status.expect("the status answers");
    assert!(status.enabled, "an enabled config must report true");
    assert_eq!(
        status.interval_seconds, 7_200,
        "the configured interval, not the default"
    );
    assert_eq!(
        status.last_run_ms,
        Some(seeded),
        "the sidecar seed, never a hardcoded None"
    );
    assert!(status.running, "a guard held right now must report running");

    // After a completed tick the in-memory gate carries the new stamp.
    let later = seeded + 7_200_001;
    let ticked = port.cleanup_tick(later).await.expect("the tick runs");
    assert!(
        matches!(ticked, CleanupTick::Ran(_)),
        "the interval elapsed since the seed, so the tick runs; got {ticked:?}"
    );
    let after = port.cleanup_status().await.expect("the status answers");
    assert_eq!(
        after.last_run_ms,
        Some(later),
        "the run that just happened is reported"
    );
    assert!(!after.running, "no guard is held once the run returned");
}
