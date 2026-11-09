//! Issue 43 W6c — the erasure sweep runner (blueprint Steps 13–14).
//!
//! The sweep is the only code path that compacts and reaps: it takes the
//! root's engine lock, refuses under a live pre-drop holder, then per target
//! (`user-prompts.db`, `user-profiles.db`, `episodic.db`) holds the operation
//! lock, checkpoints, VACUUMs and stamps, and finally reaps the six `.bak`
//! families under the exclusive root. It never touches a `-tshm`.
//!
//! These tests exercise `sweep_observed` in process (the report + phase seam)
//! and `run`'s exit codes; the CLI dispatch gets its own arg-parse test in
//! `src/main.rs`.

mod common;

use common::{TestDir, store_config};
use mem_bridge::sweep::{SweepError, SweepFailReason, SweepRequest, Tier, run, sweep_observed};
use mem_core::config::{Config, ConfigPaths};
use mem_episodic::milestone::{Contributor, encode_contributors};
use mem_episodic::store::{EPISODIC_DB_NAME, EpisodicStore};
use mem_profile::profile_store::{ProfileConfig, ProfileStore, USER_PROFILES_DB_NAME};
use mem_profile::prompt::{USER_PROMPTS_DB_NAME, UserPromptManager};
use mem_store::db::{ConnectFactory as _, ConnectOptions, Value, turso_connect_factory};
use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::sweep_probe::SweepPreflightBlock;
use mem_store::vector::vector_to_text;

/// A resolved config over a throwaway project + home. The sweep reads
/// `storage_root` (passed explicitly) and the encryption knobs.
fn config_for(dir: &TestDir) -> Config {
    let project = common::temp_dir("sweep-config-project");
    let home = common::temp_dir("sweep-config-home");
    let mut config = Config::load(
        &ConfigPaths::for_project(&project, &home),
        &std::collections::BTreeMap::new(),
    )
    .expect("the default config resolves");
    config.storage_path = dir.path().to_path_buf();
    config
}

async fn seed_prompts(dir: &TestDir) {
    let store = UserPromptManager::new(
        dir.path().to_path_buf(),
        store_config(dir.path().to_path_buf()),
        1,
    );
    store.ready().await.expect("prompts db ready");
}

async fn seed_profiles(dir: &TestDir) {
    let store = ProfileStore::new(
        dir.path().to_path_buf(),
        store_config(dir.path().to_path_buf()),
        ProfileConfig::default(),
    );
    store.ready().await.expect("profiles db ready");
}

/// A milestone carrying one scrubbed (empty-summary) contributor — the counter
/// the sweep reports as `milestone_compacted`.
async fn seed_episodic(dir: &TestDir) {
    let store = EpisodicStore::new(
        dir.path().to_path_buf(),
        store_config(dir.path().to_path_buf()),
    );
    let db = store.ready().await.expect("episodic db ready");
    let contributors = vec![Contributor {
        session_row_id: "row-1".to_string(),
        session_id: "s-1".to_string(),
        parent_session_id: None,
        agent_id: "agent-a".to_string(),
        agent_name: "Agent A".to_string(),
        created_at: 11,
        summary: String::new(),
    }];
    db.execute(
        "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
        vec![
            Value::Text("milestone-1".to_string()),
            Value::Text("project-a".to_string()),
            Value::Text("agent-a".to_string()),
            Value::Text("merged summary".to_string()),
            Value::Text(encode_contributors(&contributors).expect("encode")),
            Value::Integer(1),
            Value::Integer(11),
            Value::Integer(11),
            Value::Text(vector_to_text(&[0.5_f32; 8]).expect("vector")),
            Value::Integer(99),
        ],
    )
    .await
    .expect("insert milestone");
}

/// Reads one target's `sweep_state` rows as `(tier, stamp)` pairs.
async fn sweep_state_rows(dir: &TestDir, name: &str) -> Vec<(String, i64)> {
    let path = dir.path().join(name);
    let opts =
        ConnectOptions::for_config(&store_config(dir.path().to_path_buf()), Platform::current());
    let db = turso_connect_factory()
        .connect(path, opts)
        .await
        .expect("open for the assertion");
    let rows = db
        .query(
            "SELECT tier, last_swept_unix_ms FROM sweep_state",
            Vec::new(),
        )
        .await
        .expect("read sweep_state");
    rows.iter()
        .map(|row| {
            (
                row.text("tier").unwrap_or_default(),
                row.i64("last_swept_unix_ms").unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn test_run_without_apply_exits_64() {
    let dir = TestDir::new("sweep-usage");
    let config = config_for(&dir);
    let request = SweepRequest { apply: false };
    assert_eq!(run(&config, dir.path(), &request), 64);
}

#[test]
fn test_run_refuses_a_live_engine_holder_with_75() {
    let dir = TestDir::new("sweep-busy");
    let config = config_for(&dir);
    let holder = acquire_engine_lock(dir.path(), std::sync::Arc::new(SystemProcessProbe))
        .expect("engine lock");
    let EngineLockOutcome::Acquired(lock) = holder else {
        panic!("a fresh temp root must be free");
    };
    let request = SweepRequest { apply: true };
    assert_eq!(run(&config, dir.path(), &request), 75);
    drop(lock);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sweep_vacuums_stamps_and_reports_all_three_targets() {
    let dir = TestDir::new("sweep-all-targets");
    seed_prompts(&dir).await;
    seed_profiles(&dir).await;
    seed_episodic(&dir).await;
    let config = config_for(&dir);

    let phases = std::sync::Mutex::new(Vec::<&'static str>::new());
    let report = sweep_observed(
        &config,
        dir.path(),
        &SweepRequest { apply: true },
        &|phase| {
            phases.lock().expect("phases").push(phase);
        },
    )
    .await
    .expect("the sweep succeeds");

    assert_eq!(
        report.vacuumed,
        vec![Tier::UserPrompts, Tier::UserProfiles, Tier::Episodic],
        "every existing target is vacuumed in D-19-6 order"
    );
    assert!(report.vacuums_failed.is_empty(), "no target failed");
    assert_eq!(
        report.stamped, report.vacuumed,
        "every vacuumed target is stamped"
    );
    assert_eq!(
        report.milestone_compacted, 1,
        "one scrubbed milestone is counted"
    );
    assert_eq!(
        phases.lock().expect("phases").as_slice(),
        [
            "locked",
            "checkpointed",
            "vacuumed",
            "released",
            "locked",
            "checkpointed",
            "vacuumed",
            "released",
            "locked",
            "checkpointed",
            "vacuumed",
            "released",
        ],
        "VACUUM runs strictly between locked and released, per target"
    );

    for name in [
        USER_PROMPTS_DB_NAME,
        USER_PROFILES_DB_NAME,
        EPISODIC_DB_NAME,
    ] {
        let rows = sweep_state_rows(&dir, name).await;
        assert_eq!(rows.len(), 1, "{name} carries exactly one stamp row");
        assert!(rows[0].1 > 0, "{name}'s stamp is a real timestamp");
    }
    assert_eq!(
        sweep_state_rows(&dir, USER_PROMPTS_DB_NAME).await[0].0,
        "user-prompts"
    );
    assert_eq!(
        sweep_state_rows(&dir, USER_PROFILES_DB_NAME).await[0].0,
        "user-profiles"
    );
    assert_eq!(
        sweep_state_rows(&dir, EPISODIC_DB_NAME).await[0].0,
        "episodic"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sweep_skips_absent_targets_without_failing() {
    let dir = TestDir::new("sweep-partial");
    seed_prompts(&dir).await;
    let config = config_for(&dir);

    let report = sweep_observed(&config, dir.path(), &SweepRequest { apply: true }, &|_| {})
        .await
        .expect("a missing target is not a failure");
    assert_eq!(report.vacuumed, vec![Tier::UserPrompts]);
    assert_eq!(report.stamped, vec![Tier::UserPrompts]);
    assert!(report.vacuums_failed.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sweep_refuses_a_live_multiprocess_marker_before_any_mutation() {
    let dir = TestDir::new("sweep-legacy-peer");
    seed_prompts(&dir).await;
    let config = config_for(&dir);

    let marker = format!(
        r#"{{"pid":{},"timestamp":"2026-11-09T00:00:00.000Z","operation":null,"start_time":null,"mode":"multiprocess"}}"#,
        std::process::id()
    );
    std::fs::write(dir.path().join(".wal-mode"), marker).expect("write the marker");

    let error = sweep_observed(&config, dir.path(), &SweepRequest { apply: true }, &|_| {})
        .await
        .expect_err("a live pre-drop marker refuses the sweep");
    assert_eq!(
        error,
        SweepError::Blocked(SweepPreflightBlock::LegacyPeerLive)
    );
    assert!(
        sweep_state_rows(&dir, USER_PROMPTS_DB_NAME)
            .await
            .is_empty(),
        "a refused sweep stamps nothing"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_bak_families_are_reaped_and_tshm_is_untouched() {
    let dir = TestDir::new("sweep-bak");
    seed_prompts(&dir).await;
    let config = config_for(&dir);

    let baks = [
        "user-prompts.db.schema-v1-1700000000000.bak",
        "user-prompts.db.legacy.bak",
        "user-prompts.db.pre-reembed-42.bak",
        "user-prompts.db.pre-encrypt-1700000000000.bak",
        "project_ab_shard_0.db.pre-path-migrate-7.bak",
        "project_ab_shard_0.db.quarantine-1234-1700000000000.bak",
    ];
    for name in baks {
        std::fs::write(dir.path().join(name), b"full database copy").expect("seed a backup");
    }
    std::fs::write(
        dir.path().join("user-prompts.db.pre-reembed-42.bak-wal"),
        b"backup wal",
    )
    .expect("seed a backup sidecar");
    let tshm = dir.path().join("user-prompts.db-tshm");
    std::fs::write(&tshm, b"stale sensor bytes").expect("seed the sensor");

    let report = sweep_observed(&config, dir.path(), &SweepRequest { apply: true }, &|_| {})
        .await
        .expect("the sweep succeeds");

    for name in baks {
        assert!(
            !dir.path().join(name).exists(),
            "{name} must be reaped by the sweep"
        );
    }
    assert!(
        !dir.path()
            .join("user-prompts.db.pre-reembed-42.bak-wal")
            .exists()
    );
    assert_eq!(report.baks_reaped, 7, "six backups plus their one sidecar");
    assert_eq!(
        report.baks_bytes,
        6 * b"full database copy".len() as u64 + b"backup wal".len() as u64
    );
    assert_eq!(
        std::fs::read(&tshm).expect("read the sensor"),
        b"stale sensor bytes",
        "the sweep never reads, rewrites or reaps a -tshm"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_failing_target_is_named_and_does_not_abort_the_others() {
    let dir = TestDir::new("sweep-named-failure");
    seed_prompts(&dir).await;
    seed_profiles(&dir).await;
    let profiles_db = dir.path().join(USER_PROFILES_DB_NAME);
    // Make the profiles database unopenable (Unix permissions). The sweep must
    // name that target and still sweep the others (D-19-5: no global abort).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&profiles_db, std::fs::Permissions::from_mode(0o000))
            .expect("make the target unopenable");
    }
    let config = config_for(&dir);

    let report = sweep_observed(&config, dir.path(), &SweepRequest { apply: true }, &|_| {})
        .await
        .expect("a per-target failure is reported, not propagated");
    assert_eq!(
        report.vacuumed,
        vec![Tier::UserPrompts],
        "the other target still runs"
    );
    assert_eq!(
        report.vacuums_failed.len(),
        1,
        "the failure is a named residual"
    );
    assert_eq!(report.vacuums_failed[0].0, Tier::UserProfiles);
    assert!(
        matches!(
            report.vacuums_failed[0].1,
            SweepFailReason::Open | SweepFailReason::Checkpoint
        ),
        "the residual carries a typed reason, got {:?}",
        report.vacuums_failed[0].1
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&profiles_db, std::fs::Permissions::from_mode(0o600));
    }
}

/// Whether `path` contains `needle` as a byte sequence. A missing file answers
/// `false` (the sweep may truncate or remove a WAL).
fn contains_bytes(path: &std::path::Path, needle: &[u8]) -> bool {
    std::fs::read(path)
        .map(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
        .unwrap_or(false)
}

/// Issue 43 W6c Step 17 group 1 — the erasure canary: a logically erased row's
/// dead bytes are gone from every sweep target and from a `.bak` copy after the
/// sweep, and each database still answers.
///
/// The precondition assertions are what keep this test non-vacuous: the marker
/// is proven to be on disk BEFORE the sweep, so the post-sweep absence is the
/// sweep's doing.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sweep_erases_the_canary_bytes_from_all_three_targets() {
    let dir = TestDir::new("sweep-canary");
    let marker = "W6C-CANARY-7f3a91";
    let config = config_for(&dir);

    // user-prompts: a marker row, logically erased before the sweep.
    {
        let store = UserPromptManager::new(
            dir.path().to_path_buf(),
            store_config(dir.path().to_path_buf()),
            1,
        );
        let db = store.ready().await.expect("prompts ready");
        db.execute(
            "INSERT INTO user_prompts (id, session_id, message_id, content, created_at) VALUES (?, ?, ?, ?, ?)",
            vec![
                Value::Text("canary-prompt".to_string()),
                Value::Text("s-1".to_string()),
                Value::Text("m-1".to_string()),
                Value::Text(marker.to_string()),
                Value::Integer(1),
            ],
        )
        .await
        .expect("insert the prompt canary");
        db.execute(
            "DELETE FROM user_prompts WHERE id = ?",
            vec![Value::Text("canary-prompt".to_string())],
        )
        .await
        .expect("erase the prompt row");
    }

    // user-profiles: a profile plus a changelog snapshot carrying the marker.
    {
        let store = ProfileStore::new(
            dir.path().to_path_buf(),
            store_config(dir.path().to_path_buf()),
            ProfileConfig::default(),
        );
        let db = store.ready().await.expect("profiles ready");
        db.execute(
            "INSERT INTO user_profiles (id, user_id, display_name, user_name, user_email, profile_data, created_at, last_analyzed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                Value::Text("canary-profile".to_string()),
                Value::Text("canary-user".to_string()),
                Value::Text("Canary".to_string()),
                Value::Text("canary".to_string()),
                Value::Text("canary@example.invalid".to_string()),
                Value::Text(format!(r#"{{"preferences":["{marker}"]}}"#)),
                Value::Integer(1),
                Value::Integer(1),
            ],
        )
        .await
        .expect("insert the profile canary");
        db.execute(
            "INSERT INTO user_profile_changelogs (id, profile_id, version, change_type, change_summary, profile_data_snapshot, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            vec![
                Value::Text("canary-changelog".to_string()),
                Value::Text("canary-profile".to_string()),
                Value::Integer(1),
                Value::Text("created".to_string()),
                Value::Text(marker.to_string()),
                Value::Text(format!(r#"{{"preferences":["{marker}"]}}"#)),
                Value::Integer(1),
            ],
        )
        .await
        .expect("insert the changelog canary");
        db.execute(
            "DELETE FROM user_profile_changelogs WHERE id = ?",
            vec![Value::Text("canary-changelog".to_string())],
        )
        .await
        .expect("erase the changelog row");
        db.execute(
            "DELETE FROM user_profiles WHERE id = ?",
            vec![Value::Text("canary-profile".to_string())],
        )
        .await
        .expect("erase the profile row");
    }

    // episodic: a milestone whose summary carries the marker, then deleted.
    {
        let store = EpisodicStore::new(
            dir.path().to_path_buf(),
            store_config(dir.path().to_path_buf()),
        );
        let db = store.ready().await.expect("episodic ready");
        db.execute(
            "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
            vec![
                Value::Text("canary-milestone".to_string()),
                Value::Text("project-a".to_string()),
                Value::Text("agent-a".to_string()),
                Value::Text(marker.to_string()),
                Value::Text("[]".to_string()),
                Value::Integer(1),
                Value::Integer(1),
                Value::Integer(1),
                Value::Text(vector_to_text(&[0.25_f32; 8]).expect("vector")),
                Value::Integer(1),
            ],
        )
        .await
        .expect("insert the milestone canary");
        db.execute(
            "DELETE FROM episodic_milestones WHERE id = ?",
            vec![Value::Text("canary-milestone".to_string())],
        )
        .await
        .expect("erase the milestone row");
    }

    // A `.bak` family copy carrying the marker: reaped, not just compacted.
    let bak = dir.path().join("user-prompts.db.schema-v1-1.bak");
    std::fs::write(&bak, format!("{marker} backup copy")).expect("seed the backup canary");

    // Non-vacuity: the marker really is on disk before the sweep.
    let prompts_path = dir.path().join(USER_PROMPTS_DB_NAME);
    let prompts_wal = std::path::PathBuf::from(format!("{}-wal", prompts_path.display()));
    assert!(
        contains_bytes(&prompts_path, marker.as_bytes())
            || contains_bytes(&prompts_wal, marker.as_bytes()),
        "precondition: the erased prompt's bytes are in the database or its WAL"
    );
    assert!(
        contains_bytes(&bak, marker.as_bytes()),
        "precondition: the backup copy carries the marker"
    );

    sweep_observed(&config, dir.path(), &SweepRequest { apply: true }, &|_| {})
        .await
        .expect("the sweep succeeds");

    for name in [
        USER_PROMPTS_DB_NAME,
        USER_PROFILES_DB_NAME,
        EPISODIC_DB_NAME,
    ] {
        let db_path = dir.path().join(name);
        assert!(
            !contains_bytes(&db_path, marker.as_bytes()),
            "{name} must not carry the marker after the sweep"
        );
        let wal = std::path::PathBuf::from(format!("{}-wal", db_path.display()));
        assert!(
            !contains_bytes(&wal, marker.as_bytes()),
            "{name}'s WAL must not carry the marker after the sweep"
        );
        let rows = sweep_state_rows(&dir, name).await;
        assert_eq!(rows.len(), 1, "{name} still answers and is stamped");
    }
    assert!(!bak.exists(), "the backup copy is reaped");
}

/// Issue 43 W6c Step 17 group 5 — the stamp table is empty before any sweep, so
/// the disclosure DTO's `notYetSwept` comes from data, never from a constant.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sweep_state_is_empty_before_any_sweep() {
    let dir = TestDir::new("sweep-stamp-empty");
    seed_prompts(&dir).await;
    assert!(
        sweep_state_rows(&dir, USER_PROMPTS_DB_NAME)
            .await
            .is_empty(),
        "a migrated, never-swept database has no stamp row"
    );
}
