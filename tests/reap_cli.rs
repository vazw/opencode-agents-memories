//! W7 — the `reap-shards` CLI process contract (ADR-0023 D-23-8).
//!
//! Six acceptance tests over the REAL root binary: a seeded root is reaped by
//! a child process. `CARGO_BIN_EXE_opencode-agents-memory` is defined for this
//! package, which is why the suite lives in the root crate.
//!
//! The children run with a scratch `HOME` and an explicit `--storage-root`, so
//! the developer's config and root never participate. Every seeded handle and
//! manager is dropped BEFORE the child spawns: a live parent handle or engine
//! lock would make the child refuse.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, ConnectOptions, TursoConnectFactory};
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::scope::Scope;
use mem_store::shard_facts;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};

const HASH: &str = "a1b2c3d4e5f6000a";
const TAG: &str = "opencode_project_a1b2c3d4e5f6000a";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("reap-cli-{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the temp dir");
    dir
}

fn scratch_home(label: &str) -> PathBuf {
    let home = temp_dir(label);
    std::fs::create_dir_all(&home).expect("create the scratch home");
    home
}

fn run_async<F: std::future::Future>(future: F) -> F::Output {
    mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the test runtime")
        .block_on(future)
}

fn seed_config(root: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: 2,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

struct Rig {
    config: StoreConfig,
    connections: Arc<ConnectionManager>,
    manager: ShardManager,
}

fn rig(root: &Path) -> Rig {
    let config = seed_config(root);
    let connections = Arc::new(ConnectionManager::new(
        root.to_path_buf(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        root.to_path_buf(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
        None,
    );
    Rig {
        config,
        connections,
        manager,
    }
}

impl Rig {
    fn path(&self, index: u32) -> PathBuf {
        self.manager
            .get_shard_path(Scope::Project, HASH, index)
            .expect("shard path")
    }

    async fn inactive(&self, index: u32) -> ShardInfo {
        let path = self.path(index);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the scope dir");
        }
        let options = ConnectOptions::for_config(&self.config, Platform::current());
        let handle = TursoConnectFactory
            .connect(path.clone(), options)
            .await
            .expect("open the inactive shard");
        self.manager
            .init_shard_db(
                handle.as_ref(),
                self.config.embedding_dimensions,
                &self.config.embedding_model,
            )
            .await
            .expect("initialize the inactive shard");
        drop(handle);
        self.manager
            .register_existing_shard(Scope::Project, HASH, index, &path, 0, false)
            .await
            .expect("register the inactive shard")
    }

    /// A non-empty, inactive shard whose registry and `shard_stats` counts are
    /// both pinned to 1 (`sync_count` then `set_vector_count`).
    async fn inactive_non_empty(&self, index: u32, id: &str) -> ShardInfo {
        let path = self.path(index);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the scope dir");
        }
        let options = ConnectOptions::for_config(&self.config, Platform::current());
        let handle = TursoConnectFactory
            .connect(path.clone(), options)
            .await
            .expect("open the non-empty shard");
        self.manager
            .init_shard_db(
                handle.as_ref(),
                self.config.embedding_dimensions,
                &self.config.embedding_model,
            )
            .await
            .expect("initialize the non-empty shard");
        vector::insert_vector(handle.as_ref(), &record(id))
            .await
            .expect("insert the second shard's row");
        let count = shard_facts::sync_count(handle.as_ref())
            .await
            .expect("sync the second shard's count");
        assert_eq!(count, 1);
        drop(handle);
        let shard = self
            .manager
            .register_existing_shard(Scope::Project, HASH, index, &path, count, false)
            .await
            .expect("register the non-empty shard");
        self.manager
            .set_vector_count(shard.id, count)
            .await
            .expect("pin the registry count");
        shard
    }
}

fn record(id: &str) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: format!("content {id}"),
        vector: vec![0.1, 0.2],
        tags_vector: None,
        container_tag: TAG.to_string(),
        tags: None,
        memory_type: Some("note".to_string()),
        created_at: 1_700_000_000_000,
        updated_at: 1_700_000_000_000,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
    }
}

/// Seeds the T1 shape: an active anchor at index 1 with one row, an empty
/// inactive candidate at index 0. All handles are dropped before returning.
fn seed_t1_shape(root: &Path) -> ShardInfo {
    run_async(async {
        let rig = rig(root);
        let anchor = rig
            .manager
            .create_shard(Scope::Project, HASH, 1)
            .await
            .expect("create the anchor");
        let db = rig
            .manager
            .open_shard(&anchor.db_path)
            .await
            .expect("open the anchor");
        vector::insert_vector(db.as_ref(), &record("anchor-row"))
            .await
            .expect("insert the anchor row");
        drop(db);
        let candidate = rig.inactive(0).await;
        drop(rig.manager);
        drop(rig.connections);
        candidate
    })
}

/// Seeds ONLY the active anchor at index 1 with one row (CLI-14's first act).
fn seed_anchor_only(root: &Path) -> ShardInfo {
    run_async(async {
        let rig = rig(root);
        let anchor = rig
            .manager
            .create_shard(Scope::Project, HASH, 1)
            .await
            .expect("create the anchor");
        let db = rig
            .manager
            .open_shard(&anchor.db_path)
            .await
            .expect("open the anchor");
        vector::insert_vector(db.as_ref(), &record("anchor-row"))
            .await
            .expect("insert the anchor row");
        drop(db);
        drop(rig.manager);
        drop(rig.connections);
        anchor
    })
}

/// Creates the candidate AFTER CLI-14's dry-run child has settled the sidecars.
///
/// The dry child's legacy migration wrote the root marker and the anchor's
/// complete per-shard sidecar, but the candidate did not exist then.
/// `all_shards_complete` requires a complete `.turso-migrate.json` for every
/// shard file, so without one the `--apply` child would re-run
/// `migrate_storage` and write that sidecar INSIDE the `0o555` scope dir
/// (EACCES) before the reap could run. Recording the candidate's completion
/// here is exactly what `migrate_memory_shard` writes for an empty,
/// metadata-valid shard; it makes the `--apply` child take the marker branch
/// (reconcile only), which is the precondition CLI-14 needs.
///
/// `ShardMigrationSidecar` is `#[serde(rename_all = "camelCase")]` and
/// `read_sidecar` requires `expectedCount`, `importedCount` and `status`; a
/// snake_case body is unparseable and makes the apply child re-run
/// `migrate_storage` (rewriting this file and the root marker).
fn seed_candidate_after_dry(root: &Path) -> (PathBuf, PathBuf) {
    let candidate = run_async(async {
        let rig = rig(root);
        let candidate = rig.inactive(0).await;
        drop(rig.manager);
        drop(rig.connections);
        candidate
    });
    let sidecar = PathBuf::from(format!(
        "{}.turso-migrate.json",
        candidate.db_path.display()
    ));
    std::fs::write(
        &sidecar,
        r#"{"sourceCount":0,"expectedCount":0,"importedCount":0,"skippedCount":0,"status":"complete"}"#,
    )
    .expect("write the candidate's complete migration sidecar");
    (candidate.db_path, sidecar)
}

/// CLI-12's richer shape: anchor 2 (active, one row), a non-empty second
/// shard at 1, and the empty candidate at 0.
fn seed_apply_shape(root: &Path) -> (ShardInfo, ShardInfo) {
    run_async(async {
        let rig = rig(root);
        let anchor = rig
            .manager
            .create_shard(Scope::Project, HASH, 2)
            .await
            .expect("create the anchor");
        let db = rig
            .manager
            .open_shard(&anchor.db_path)
            .await
            .expect("open the anchor");
        vector::insert_vector(db.as_ref(), &record("anchor-row"))
            .await
            .expect("insert the anchor row");
        drop(db);
        let second = rig.inactive_non_empty(1, "second-row").await;
        let candidate = rig.inactive(0).await;
        drop(rig.manager);
        drop(rig.connections);
        (second, candidate)
    })
}

fn run_child(home: &Path, root: &Path, extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .args(["--mode=reap-shards", "--storage-root"])
        .arg(root)
        .args(extra)
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.output().expect("spawn the reap child")
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn report(output: &Output) -> mem_store::serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one JSON line on stdout, got {stdout:?}"
    );
    mem_store::serde_json::from_str(lines[0]).expect("the stdout line is JSON")
}

fn ids(value: &mem_store::serde_json::Value, field: &str) -> Vec<i64> {
    value[field]
        .as_array()
        .unwrap_or_else(|| panic!("`{field}` is an array: {value}"))
        .iter()
        .map(|id| id.as_i64().expect("a shard id is an integer"))
        .collect()
}

fn skipped(value: &mem_store::serde_json::Value) -> &Vec<mem_store::serde_json::Value> {
    value["skipped"].as_array().expect("`skipped` is an array")
}

// ---------------------------------------------------------------------------
// CLI-11 — dry run is the default
// ---------------------------------------------------------------------------

#[test]
fn test_reap_shards_dry_run_is_the_default() {
    let home = scratch_home("dry-home");
    let root = temp_dir("dry-root");
    let candidate = seed_t1_shape(&root);

    let output = run_child(&home, &root, &[]);

    assert_eq!(output.status.code(), Some(0), "dry run exits 0: {output:?}");
    let report = report(&output);
    assert_eq!(report["dry_run"], mem_store::serde_json::json!(true));
    assert!(
        ids(&report, "candidates").contains(&candidate.id),
        "the empty candidate is reported: {report}"
    );
}

// ---------------------------------------------------------------------------
// CLI-12 — apply deletes, and a second apply is a no-op
// ---------------------------------------------------------------------------

#[test]
fn test_reap_shards_apply_deletes_and_a_second_apply_is_a_no_op() {
    let home = scratch_home("apply-home");
    let root = temp_dir("apply-root");
    let (second, candidate) = seed_apply_shape(&root);

    let first = run_child(&home, &root, &["--apply"]);
    assert_eq!(first.status.code(), Some(0), "apply exits 0: {first:?}");
    let first_report = report(&first);
    assert_eq!(first_report["dry_run"], mem_store::serde_json::json!(false));
    assert!(
        ids(&first_report, "reaped").contains(&candidate.id),
        "the empty candidate is reaped: {first_report}"
    );
    assert!(
        skipped(&first_report).iter().any(|skip| {
            skip["shard_id"] == mem_store::serde_json::json!(second.id)
                && skip["reason"] == mem_store::serde_json::json!("non_empty")
                && skip["rows"] == mem_store::serde_json::json!(1)
        }),
        "the non-empty second shard is named with its row count: {first_report}"
    );
    assert!(
        first_report["bytes_freed"].as_u64().unwrap_or(0) > 0,
        "removing the candidate freed apparent bytes: {first_report}"
    );

    let second_run = run_child(&home, &root, &["--apply"]);
    assert_eq!(second_run.status.code(), Some(0), "second apply exits 0");
    let second_report = report(&second_run);
    assert_eq!(
        ids(&second_report, "candidates"),
        Vec::<i64>::new(),
        "nothing is left to consider: {second_report}"
    );
    assert_eq!(ids(&second_report, "reaped"), Vec::<i64>::new());
    assert!(
        skipped(&second_report).iter().any(|skip| {
            skip["shard_id"] == mem_store::serde_json::json!(second.id)
                && skip["reason"] == mem_store::serde_json::json!("non_empty")
        }),
        "the non-empty shard is still named: {second_report}"
    );
    assert_eq!(
        second_report["bytes_freed"],
        mem_store::serde_json::json!(0)
    );
}

// ---------------------------------------------------------------------------
// CLI-13 — usage exits 64 before touching the root
// ---------------------------------------------------------------------------

#[test]
fn test_reap_shards_usage_exits_64_before_touching_the_root() {
    let home = scratch_home("usage-home");
    let fixture = temp_dir("usage-fixture");
    for extra in [
        vec!["--apply=yes"],
        vec!["--apply", "false"],
        vec!["--scope", "x"],
        // A bare foreign flag at end-of-argv parses to `None`; presence, not
        // value, is the usage rule.
        vec!["--scope"],
        // The data-safety shape: apply plus a filter the old guard dropped.
        vec!["--apply", "--scope"],
    ] {
        let root = fixture.join(format!("absent-{}", extra.join("-").replace('=', "-")));
        let output = run_child(&home, &root, &extra);
        assert_eq!(
            output.status.code(),
            Some(64),
            "usage exits 64 for {extra:?}: {output:?}"
        );
        assert!(
            combined(&output).contains("usage:"),
            "a `usage:` line names the invocation for {extra:?}: {output:?}"
        );
        assert!(
            !root.exists(),
            "the bad invocation never creates the root for {extra:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CLI-14 — a file-operation failure exits 1 with a completed report
// ---------------------------------------------------------------------------

#[test]
fn test_reap_shards_file_failure_exits_1() {
    use std::os::unix::fs::PermissionsExt;

    let home = scratch_home("file-failure-home");
    let root = temp_dir("file-failure-root");
    // Order is load-bearing: anchor -> dry-run child (settles sidecars) ->
    // create the candidate -> chmod -> apply. The dry-run child runs before
    // the candidate exists, so M8 reddens only T8.
    seed_anchor_only(&root);
    let dry = run_child(&home, &root, &[]);
    assert_eq!(dry.status.code(), Some(0), "the settling dry run succeeds");
    let (candidate_db, sidecar) = seed_candidate_after_dry(&root);
    let marker = root.join(mem_store::legacy_migrator::MIGRATION_MARKER);
    let marker_before = std::fs::read(&marker).expect("the dry child wrote the root marker");
    let sidecar_before = std::fs::read(&sidecar).expect("the fixture wrote the sidecar");

    let scope_dir = root.join("projects");
    let original = std::fs::metadata(&scope_dir)
        .expect("scope dir metadata")
        .permissions();
    let mut locked = original.clone();
    locked.set_mode(0o555);
    std::fs::set_permissions(&scope_dir, locked).expect("chmod the scope dir 0o555");

    let output = run_child(&home, &root, &["--apply"]);

    std::fs::set_permissions(&scope_dir, original).expect("restore the scope dir");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a completed report with a file failure exits 1: {output:?}"
    );
    let text = combined(&output);
    assert!(
        text.contains("reap.file_delete_failed"),
        "the failed file is named: {text}"
    );
    assert!(
        !text.contains("reap.failed"),
        "a file failure is not a store failure: {text}"
    );
    // The decisive marker-branch check: a `migrate_storage` run rewrites the
    // root marker (a new `completedAt` and the candidate entry); the marker
    // branch returns before it. The sidecar staying compact is corroborating.
    let marker_after = std::fs::read(&marker).expect("the apply child leaves the marker readable");
    assert_eq!(
        String::from_utf8_lossy(&marker_after),
        String::from_utf8_lossy(&marker_before),
        "the apply child must take the marker branch, not rewrite migrate_storage"
    );
    let sidecar_after =
        std::fs::read(&sidecar).expect("the apply child leaves the sidecar readable");
    assert_eq!(
        String::from_utf8_lossy(&sidecar_after),
        String::from_utf8_lossy(&sidecar_before),
        "the complete sidecar must not be rewritten by the apply child"
    );
    assert!(
        candidate_db.exists(),
        "the candidate .db survives the blocked unlink"
    );
}

// ---------------------------------------------------------------------------
// CLI-15 — a live engine refuses the reap
// ---------------------------------------------------------------------------

#[test]
fn test_reap_shards_refuses_when_an_engine_holds_the_root() {
    let home = scratch_home("engine-home");
    let root = temp_dir("engine-root");
    let lock =
        match mem_store::engine_lock::acquire_engine_lock(&root, Arc::new(SystemProcessProbe))
            .expect("acquire the engine lock")
        {
            mem_store::engine_lock::EngineLockOutcome::Acquired(lock) => lock,
            mem_store::engine_lock::EngineLockOutcome::Lost { holder_pid } => {
                panic!("a fresh test root is free, holder {holder_pid}")
            }
        };

    let output = run_child(&home, &root, &["--apply"]);
    drop(lock);

    assert_eq!(
        output.status.code(),
        Some(74),
        "a lost engine lock exits 74: {output:?}"
    );
    assert!(
        combined(&output).contains("reap.refused"),
        "the refusal is named: {output:?}"
    );
}

// ---------------------------------------------------------------------------
// CLI-16 — a held operation lock refuses the reap as busy
// ---------------------------------------------------------------------------

#[test]
fn test_reap_shards_refuses_when_the_operation_lock_is_held() {
    let home = scratch_home("oplock-home");
    let root = temp_dir("oplock-root");
    let lock = mem_store::operation_lock::acquire_turso_operation_lock(
        &root,
        Arc::new(SystemProcessProbe),
        "reap-cli-test",
    )
    .expect("hold the operation lock");

    let output = run_child(&home, &root, &["--apply"]);
    drop(lock);

    assert_eq!(
        output.status.code(),
        Some(75),
        "a held operation lock exits 75: {output:?}"
    );
    assert!(
        combined(&output).contains("reap.busy"),
        "the busy refusal is named: {output:?}"
    );
}
