//! W7 — retired-shard reaping battery (ADR-0023).
//!
//! Ten rules, one per test, each paired with the mutant that reddens exactly
//! it (Step 9 of the blueprint). The fixture partition is stated once here:
//! **T2 is the only test that pins the `active` reason**; every other fixture
//! with an active anchor asserts only that the anchor is present in `skipped`
//! and that its row/file survive.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, ConnectOptions, TursoConnectFactory};
use mem_store::delete_guard::ExclusiveRoot;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::scope::Scope;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};

const HASH: &str = "a1b2c3d4e5f60009";
const TAG: &str = "opencode_project_a1b2c3d4e5f60009";

// ---------------------------------------------------------------------------
// The rig: a temp root, the managers over it, and the live exclusivity proof
// ---------------------------------------------------------------------------

struct Rig {
    config: StoreConfig,
    connections: Arc<ConnectionManager>,
    manager: ShardManager,
    /// Kept alive for the whole test: the reaper needs the engine-lock proof.
    _proof: Arc<ExclusiveRoot>,
}

fn rig(prefix: &str) -> Rig {
    let config = common::test_store_config(prefix, 2, 1000);
    // The proof and the ConnectionManager must be built with the SAME path:
    // `ExclusiveRoot::current` matches the storage root exactly.
    let proof = common::exclusive_root(&config.storage_path);
    let connections = Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections.clone(),
        Arc::new(SystemProcessProbe),
        None,
    );
    Rig {
        config,
        connections,
        manager,
        _proof: proof,
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

impl Rig {
    fn path(&self, index: u32) -> PathBuf {
        self.manager
            .get_shard_path(Scope::Project, HASH, index)
            .expect("shard path")
    }

    /// Opens `path` through the real factory and runs `init_shard_db` — the
    /// writable creation the fixture uses for a non-anchor shard.
    async fn init_file(&self, path: &Path) {
        // The factory creates the database file but not its scope directory;
        // the fixture owns the directory layout, not the reaper.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the scope directory");
        }
        let options =
            ConnectOptions::for_config(&self.config, mem_store::platform::Platform::current());
        let handle = TursoConnectFactory
            .connect(path.to_path_buf(), options)
            .await
            .expect("open the shard file read-write");
        self.manager
            .init_shard_db(
                handle.as_ref(),
                self.config.embedding_dimensions,
                &self.config.embedding_model,
            )
            .await
            .expect("initialize the shard file");
    }

    /// Creates an ACTIVE anchor at `index` and inserts `rows` rows.
    async fn anchor(&self, index: u32, rows: &[&str]) -> ShardInfo {
        let shard = self
            .manager
            .create_shard(Scope::Project, HASH, index)
            .await
            .expect("create the active anchor");
        if !rows.is_empty() {
            let db = self
                .manager
                .open_shard(&shard.db_path)
                .await
                .expect("open the anchor");
            for id in rows {
                vector::insert_vector(db.as_ref(), &record(id))
                    .await
                    .expect("insert an anchor row");
            }
        }
        // Release the manager's cached writable handle: a live handle answers
        // the `-tshm` byte-0 query `Held` for this same process, which would
        // make an otherwise-eligible skip indistinguishable from a holder.
        self.connections
            .close_all()
            .await
            .expect("close the anchor's cached handle");
        shard
    }

    /// Creates an INACTIVE shard file at `index`, inserts `rows` with
    /// `vector::insert_vector` and registers it with registry count 0. No
    /// `sync_count` runs, so `shard_stats` has no row — T3's distinguishing
    /// input.
    async fn inactive_with_rows(&self, index: u32, rows: &[&str]) -> ShardInfo {
        let path = self.path(index);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the scope directory");
        }
        let options =
            ConnectOptions::for_config(&self.config, mem_store::platform::Platform::current());
        let handle = TursoConnectFactory
            .connect(path.clone(), options)
            .await
            .expect("open the candidate file read-write");
        self.manager
            .init_shard_db(
                handle.as_ref(),
                self.config.embedding_dimensions,
                &self.config.embedding_model,
            )
            .await
            .expect("initialize the candidate file");
        for id in rows {
            vector::insert_vector(handle.as_ref(), &record(id))
                .await
                .expect("insert a candidate row");
        }
        drop(handle);
        self.manager
            .register_existing_shard(Scope::Project, HASH, index, &path, 0, false)
            .await
            .expect("register the inactive shard")
    }

    /// Creates an INACTIVE, empty shard file at `index` and registers it.
    async fn inactive(&self, index: u32) -> ShardInfo {
        let path = self.path(index);
        self.init_file(&path).await;
        self.manager
            .register_existing_shard(Scope::Project, HASH, index, &path, 0, false)
            .await
            .expect("register the inactive shard")
    }
}

/// Every regular file in the candidate's target set that exists BEFORE the
/// reap, with its size.
fn captured_targets(db_path: &Path) -> Vec<(PathBuf, u64)> {
    let mut targets = vec![db_path.to_path_buf()];
    for suffix in ["-wal", "-shm", "-tshm"] {
        targets.push(PathBuf::from(format!("{}{suffix}", db_path.display())));
    }
    targets
        .into_iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(&path).ok()?;
            metadata
                .is_file()
                .then_some((path, metadata.len()))
                .filter(|(_, size)| *size > 0)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// T1 — an eligible retired shard is fully removed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_eligible_retired_shard_is_fully_removed() {
    let rig = rig("w7-t1");
    let anchor = rig.anchor(1, &["anchor-row"]).await;
    let candidate = rig.inactive(0).await;
    // Capture AFTER the fixture: the writable creation leaves the `-tshm`
    // sidecar (and a real `-wal`); the fixture never creates or truncates the
    // `-tshm` itself, because a truncation makes the read-only open reject it.
    let captured = captured_targets(&candidate.db_path);
    assert!(
        captured
            .iter()
            .any(|(path, _)| path.to_string_lossy().ends_with("-tshm")),
        "the writable creation must leave a -tshm sidecar: {captured:?}"
    );
    let expected_bytes: u64 = captured.iter().map(|(_, size)| *size).sum();

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("reap the retired shard");

    assert_eq!(report.candidates, vec![candidate.id]);
    assert_eq!(report.reaped, vec![candidate.id]);
    assert!(
        report.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is considered and listed in skipped: {:?}",
        report.skipped
    );
    assert!(
        !report
            .skipped
            .iter()
            .any(|skip| skip.shard_id == candidate.id),
        "a reaped candidate is not skipped: {:?}",
        report.skipped
    );

    // The anchor survives, row and file.
    assert!(
        rig.manager
            .get_shard_by_id(anchor.id)
            .await
            .expect("anchor row read")
            .is_some(),
        "the active anchor's row survives"
    );
    assert!(anchor.db_path.exists(), "the active anchor's file survives");

    // The candidate's row and every captured target are gone.
    let remaining = rig
        .manager
        .get_all_shards(Scope::Project, Some(HASH))
        .await
        .expect("registry read");
    assert!(
        !remaining.iter().any(|shard| shard.id == candidate.id),
        "the reaped row is gone from the registry"
    );
    for (path, _) in &captured {
        assert!(!path.exists(), "the captured target survives: {path:?}");
    }
    assert_eq!(
        report.bytes_freed, expected_bytes,
        "bytes_freed is the sum of the pre-existing regular-file sizes"
    );
}

// ---------------------------------------------------------------------------
// T3 — a shard with rows is never reaped (the ONLY `non_empty`-reason pin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_shard_with_rows_is_never_reaped() {
    let rig = rig("w7-t3");
    let anchor = rig.anchor(1, &["anchor-row"]).await;
    let candidate = rig.inactive_with_rows(0, &["candidate-row"]).await;

    // The fixture's distinguishing input, asserted before the reap: one row,
    // NO `shard_stats` row, and registry count 0. A count read from either of
    // the latter would classify the shard as empty.
    let stored = rig.manager.get_shard_by_id(candidate.id).await.unwrap();
    assert_eq!(
        stored.expect("the candidate row").vector_count,
        0,
        "the registry count is 0, not the file's COUNT(*)"
    );
    let options = ConnectOptions::for_config(&rig.config, Platform::current());
    let handle = TursoConnectFactory
        .connect(candidate.db_path.clone(), options)
        .await
        .expect("open the candidate");
    let count = mem_store::shard_facts::read_count(handle.as_ref())
        .await
        .expect("read COUNT(*)");
    assert_eq!(count, 1, "the file holds exactly one row");
    let stored_count = mem_store::shard_facts::read_stored_count(handle.as_ref())
        .await
        .expect("read the stored count");
    assert_eq!(stored_count, 0, "an absent shard_stats row collapses to 0");
    let stats_rows = mem_store::db::query_one(
        handle.as_ref(),
        "SELECT COUNT(*) as count FROM shard_stats",
        Vec::new(),
    )
    .await
    .expect("count the shard_stats rows");
    assert_eq!(
        stats_rows.and_then(|row| row.i64("count")),
        Some(0),
        "the fixture writes no shard_stats row"
    );
    drop(handle);

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("reap over the one-row candidate");

    assert!(report.candidates.is_empty());
    assert!(report.reaped.is_empty());
    assert!(
        report.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is listed in skipped: {:?}",
        report.skipped
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.shard_id == candidate.id
                && skip.reason == mem_store::reaping::ReapSkipReason::NonEmpty { rows: 1 }),
        "the candidate is skipped with the measured row count: {:?}",
        report.skipped
    );
    assert!(
        rig.manager
            .get_shard_by_id(candidate.id)
            .await
            .unwrap()
            .is_some(),
        "the candidate row survives"
    );
    assert!(candidate.db_path.exists(), "the candidate file survives");
}

// ---------------------------------------------------------------------------
// T4 — the highest shard of a zero-active tuple is never reaped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_highest_shard_of_a_zero_active_tuple_is_never_reaped() {
    let rig = rig("w7-t4");
    let lower = rig.inactive(0).await;
    let highest = rig.inactive(1).await;
    assert!(
        !rig.manager
            .get_active_shard(Scope::Project, HASH)
            .await
            .expect("active read")
            .is_some(),
        "the fixture's tuple has zero active rows"
    );

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("reap over a zero-active tuple");

    assert_eq!(report.candidates, vec![lower.id]);
    assert_eq!(report.reaped, vec![lower.id]);
    assert_eq!(
        report.skipped,
        vec![mem_store::reaping::SkippedShard {
            shard_id: highest.id,
            reason: mem_store::reaping::ReapSkipReason::HighestWithoutActive,
        }],
        "only the reactivation target is skipped"
    );

    assert!(
        !rig.manager
            .get_all_shards(Scope::Project, Some(HASH))
            .await
            .expect("registry read")
            .iter()
            .any(|shard| shard.id == lower.id),
        "the lower row is reaped"
    );
    assert!(
        rig.manager
            .get_shard_by_id(highest.id)
            .await
            .expect("highest read")
            .is_some(),
        "the highest row survives"
    );
    assert!(highest.db_path.exists(), "the highest file survives");
}

// ---------------------------------------------------------------------------
// T5 — the highest inactive shard BESIDE an active row is reaped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_highest_inactive_shard_beside_an_active_row_is_reaped() {
    let rig = rig("w7-t5");
    let anchor = rig.anchor(0, &["anchor-row"]).await;
    let highest = rig.inactive(1).await;

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("reap beside an active row");

    assert_eq!(report.candidates, vec![highest.id]);
    assert_eq!(report.reaped, vec![highest.id]);
    assert!(
        report.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is listed in skipped: {:?}",
        report.skipped
    );
    assert!(
        !report
            .skipped
            .iter()
            .any(|skip| skip.shard_id == highest.id),
        "the highest inactive row beside an active row is not skipped: {:?}",
        report.skipped
    );
    assert!(
        rig.manager
            .get_shard_by_id(anchor.id)
            .await
            .expect("anchor read")
            .is_some(),
        "the active anchor survives"
    );
    assert!(anchor.db_path.exists());
    assert!(
        !rig.manager
            .get_all_shards(Scope::Project, Some(HASH))
            .await
            .expect("registry read")
            .iter()
            .any(|shard| shard.id == highest.id),
        "the highest inactive row is gone"
    );
    assert!(!highest.db_path.exists(), "the highest file is gone");
}

// ---------------------------------------------------------------------------
// T6 — an unopenable shard is skipped with a reason
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unopenable_shard_is_skipped_with_a_reason() {
    let rig = rig("w7-t6");
    let anchor = rig.anchor(2, &["anchor-row"]).await;
    let garbage = rig.inactive(0).await;
    std::fs::write(&garbage.db_path, b"not a database").expect("overwrite with garbage");
    let missing = rig.inactive(1).await;
    std::fs::remove_file(&missing.db_path).expect("remove the missing row's file");

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("an unopenable shard is a skip, not an Err");

    assert!(report.candidates.is_empty());
    assert!(report.reaped.is_empty());
    assert!(
        report.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is listed in skipped: {:?}",
        report.skipped
    );
    for shard in [&garbage, &missing] {
        assert!(
            report.skipped.iter().any(|skip| skip.shard_id == shard.id
                && skip.reason == mem_store::reaping::ReapSkipReason::Unopenable),
            "shard {} is skipped as unopenable: {:?}",
            shard.id,
            report.skipped
        );
        assert!(
            rig.manager
                .get_shard_by_id(shard.id)
                .await
                .expect("row read")
                .is_some(),
            "the unopenable row {} survives",
            shard.id
        );
    }
    assert_eq!(
        std::fs::read(&garbage.db_path).expect("read the garbage file"),
        b"not a database",
        "the garbage file is untouched"
    );
    assert!(!missing.db_path.exists(), "the missing file stays missing");
}

// ---------------------------------------------------------------------------
// T7 — a live -tshm holder is skipped
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_live_tshm_holder_is_skipped() {
    use std::os::unix::io::AsRawFd;

    let rig = rig("w7-t7");
    let anchor = rig.anchor(1, &["anchor-row"]).await;
    let candidate = rig.inactive(0).await;
    let tshm = PathBuf::from(format!("{}-tshm", candidate.db_path.display()));
    assert!(tshm.exists(), "the writable creation leaves a -tshm");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tshm)
        .expect("open the -tshm for the shared lock");
    // SAFETY: `libc::flock` is plain old data; all-zero is a valid pattern and
    // every meaningful field is overwritten below (the open_guard precedent).
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_RDLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 0;
    lock.l_len = 1;
    lock.l_pid = 0;
    // SAFETY: `file` owns the fd for the whole test; `lock` is a fully
    // initialised `libc::flock` and `F_OFD_SETLK` does not block.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &mut lock) };
    assert_eq!(
        result,
        0,
        "the shared OFD lock must be taken (errno {:?})",
        std::io::Error::last_os_error()
    );

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("a live holder is a skip, not an Err");

    assert!(report.candidates.is_empty());
    assert!(report.reaped.is_empty());
    assert!(
        report.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is listed in skipped: {:?}",
        report.skipped
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.shard_id == candidate.id
                && skip.reason == mem_store::reaping::ReapSkipReason::LiveTshmHolder),
        "the candidate is skipped as live-held: {:?}",
        report.skipped
    );
    assert!(
        rig.manager
            .get_shard_by_id(candidate.id)
            .await
            .expect("row read")
            .is_some(),
        "the live-held row survives"
    );
    assert!(candidate.db_path.exists(), "the .db survives");
    assert!(tshm.exists(), "the -tshm survives");
}

// ---------------------------------------------------------------------------
// T8 — a dry run deletes nothing and reports the same candidates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dry_run_deletes_nothing_and_reports_the_same_candidates() {
    let rig = rig("w7-t8");
    let anchor = rig.anchor(1, &["anchor-row"]).await;
    let candidate = rig.inactive(0).await;

    let dry = rig
        .manager
        .reap_retired_shards(true)
        .await
        .expect("dry run");

    assert_eq!(dry.candidates, vec![candidate.id]);
    assert!(dry.reaped.is_empty(), "a dry run reaps nothing");
    assert!(
        dry.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is listed in skipped: {:?}",
        dry.skipped
    );
    assert_eq!(dry.bytes_freed, 0);
    assert!(
        rig.manager
            .get_shard_by_id(candidate.id)
            .await
            .expect("row read")
            .is_some(),
        "the dry run leaves the candidate row intact"
    );
    assert!(
        candidate.db_path.exists(),
        "the dry run leaves the candidate .db intact"
    );

    let apply = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("apply after the dry run");

    assert_eq!(
        apply.reaped, dry.candidates,
        "the apply reaps exactly the dry run's candidates"
    );
    assert!(
        rig.manager
            .get_shard_by_id(candidate.id)
            .await
            .expect("row read")
            .is_none(),
        "the apply removes the candidate row"
    );
    assert!(!candidate.db_path.exists(), "the apply removes the .db");
}

// ---------------------------------------------------------------------------
// T9 — a second run is a no-op, and a file with no row is ignored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_second_run_is_a_no_op_and_ignores_a_file_with_no_row() {
    let rig = rig("w7-t9");
    let anchor = rig.anchor(1, &["anchor-row"]).await;
    let candidate = rig.inactive(0).await;
    let orphan_path = rig.path(2);
    rig.init_file(&orphan_path).await;
    assert!(orphan_path.exists(), "the orphan file exists");
    assert!(
        rig.manager
            .get_shard_by_path(&orphan_path)
            .await
            .expect("registry lookup")
            .is_none(),
        "the orphan has no registry row"
    );

    let first = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("first apply");
    assert_eq!(first.candidates, vec![candidate.id]);
    assert_eq!(first.reaped, vec![candidate.id]);
    assert!(
        orphan_path.exists(),
        "the orphan file survives the first run"
    );

    let second = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("second apply");
    assert!(second.candidates.is_empty());
    assert!(second.reaped.is_empty());
    assert!(
        second.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is still listed in skipped: {:?}",
        second.skipped
    );
    assert_eq!(second.bytes_freed, 0);
    assert!(
        orphan_path.exists(),
        "the orphan file survives the second run"
    );
}

// ---------------------------------------------------------------------------
// T10 — a file-delete failure after the row is gone is reported
// ---------------------------------------------------------------------------

/// Restores a directory's original permissions on drop (also on panic).
struct RestorePermissions {
    path: PathBuf,
    permissions: std::fs::Permissions,
}

impl Drop for RestorePermissions {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.permissions.clone());
    }
}

#[tokio::test]
async fn test_file_delete_failure_after_the_row_is_gone_is_reported() {
    use std::os::unix::fs::PermissionsExt;

    let rig = rig("w7-t10");
    let anchor = rig.anchor(1, &["anchor-row"]).await;
    let candidate = rig.inactive(0).await;
    let scope_dir = candidate
        .db_path
        .parent()
        .expect("the candidate has a scope directory")
        .to_path_buf();
    let original = std::fs::metadata(&scope_dir)
        .expect("scope dir metadata")
        .permissions();
    let probe = scope_dir.join(".unlink-probe");
    std::fs::write(&probe, b"probe").expect("write the unlink probe");
    let mut locked = original.clone();
    locked.set_mode(0o555);
    std::fs::set_permissions(&scope_dir, locked.clone()).expect("chmod the scope dir 0o555");
    let guard = RestorePermissions {
        path: scope_dir.clone(),
        permissions: original.clone(),
    };

    // The positive control: a non-root uid cannot unlink inside a 0o555
    // directory, so the `.db` failure below is the permission bit, not a
    // missing file. Root would make this fixture pass vacuously.
    let control = std::fs::remove_file(&probe);
    assert!(
        matches!(&control, Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied),
        "this fixture needs a non-root uid: the probe unlink did not fail with EACCES ({control:?})"
    );
    std::fs::set_permissions(&scope_dir, original.clone()).expect("restore the scope dir");
    std::fs::remove_file(&probe).expect("remove the probe");
    std::fs::set_permissions(&scope_dir, locked).expect("re-apply 0o555");

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("a file failure is a report, not an Err");
    drop(guard);

    assert_eq!(
        report.candidates,
        vec![candidate.id],
        "a failing candidate stays in candidates"
    );
    assert!(
        report.reaped.is_empty(),
        "a candidate with a failed file removal is not reaped"
    );
    assert!(
        report.skipped.iter().any(|skip| skip.shard_id == anchor.id),
        "the active anchor is listed in skipped: {:?}",
        report.skipped
    );
    let failure = report
        .skipped
        .iter()
        .find(|skip| skip.shard_id == candidate.id)
        .expect("the failed candidate is additionally listed in skipped");
    match &failure.reason {
        mem_store::reaping::ReapSkipReason::FileDeleteFailed { failures } => {
            assert!(
                failures
                    .iter()
                    .any(|failed| failed.path == candidate.db_path),
                "the .db unlink is the reported failure: {failures:?}"
            );
        }
        other => panic!("expected file_delete_failed, got {other:?}"),
    }
    assert!(
        rig.manager
            .get_shard_by_id(candidate.id)
            .await
            .expect("row read")
            .is_none(),
        "the row delete succeeded before the file removal"
    );
    assert!(
        candidate.db_path.exists(),
        "the .db survives the failed unlink"
    );
    assert_eq!(report.bytes_freed, 0, "nothing was freed");
}

// ---------------------------------------------------------------------------
// T2 — an active shard is never reaped (the ONLY `active`-reason pin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_active_shard_is_never_reaped() {
    let rig = rig("w7-t2");
    let anchor = rig.anchor(0, &[]).await;

    let report = rig
        .manager
        .reap_retired_shards(false)
        .await
        .expect("reap over an active-only root");

    assert_eq!(
        report.skipped,
        vec![mem_store::reaping::SkippedShard {
            shard_id: anchor.id,
            reason: mem_store::reaping::ReapSkipReason::Active,
        }]
    );
    assert!(report.candidates.is_empty());
    assert!(report.reaped.is_empty());
    assert_eq!(report.bytes_freed, 0);
    assert!(
        rig.manager
            .get_shard_by_id(anchor.id)
            .await
            .expect("anchor row read")
            .is_some()
    );
    assert!(anchor.db_path.exists());
}
