//! Port of upstream `tests/profile-cold-buffer-cross-process.test.ts` (W3.1).
//!
//! Cross-process cold-buffer safety: real, independent `ProfileStore`
//! instances over one shared storage dir simulate peer learning processes.
//! Every assertion goes through the public `merge_profile_data` /
//! `delete_profile` / `reset` surface and the on-disk snapshot; the embedding
//! boundary is mocked (no real model, no network).
//!
//! Upstream cases (traced 1:1 by name, 16 total):
//! - "sees a peer's bucket when the file mtime is held fixed"
//! - "still reads peer updates when the file mtime goes backwards"
//! - "does not resurrect a deleted cold-buffer file from a stale in-memory map"
//! - "fails closed on a corrupt cold-buffer file instead of overwriting it"
//! - "fails closed on an empty or whitespace-only cold-buffer file"
//! - "fails closed on invalid bucket shapes without touching the file bytes"
//! - "accepts a valid snapshot and a valid legacy flat file"
//! - "completes a save across byte-granular short writes (ascii + multibyte)"
//! - "throws on write() returning 0 and leaves the previous target bytes intact"
//! - "throws on a mid-payload write error and leaves the previous target bytes intact"
//! - "fails closed when the cold-buffer file is unreadable"
//! - "propagates save failures without corrupting the existing file"
//! - "keeps every pending buffer mutation from one merge round on disk"
//! - "reset() drops the previous storage's buckets"
//! - "deleteProfile removes the bucket from the current on-disk snapshot"
//! - "a warm merge drains a peer-buffered bucket through the real merge path"
//!
//! The four short-write cases run through the `fixture_child_entry`
//! subprocess (`docs/agents/testing.md` recipe) so the real `save_snapshot_with`
//! fault seam is driven in a child process exactly like upstream's
//! `bun --preload` worker, and each runs a 10-iteration repeat loop here.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use mem_profile::cold_buffer::{Bucket, COLD_BUFFER_FILE_NAME};
use mem_profile::profile_store::{
    EmbedBoundary, ProfileConfig, ProfileData, ProfileItem, ProfileStore, ProfileUpdates,
};

// ---------------------------------------------------------------------------
// Fixtures (upstream coldEmbed / warmEmbed / empty / pref)
// ---------------------------------------------------------------------------

struct ColdEmbed;

impl EmbedBoundary for ColdEmbed {
    fn is_warmed_up(&self) -> bool {
        false
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>, mem_profile::ProfileError> {
        Err(mem_profile::ProfileError::Save(
            "cold boundary must not embed".to_string(),
        ))
    }
}

struct WarmEmbed;

impl EmbedBoundary for WarmEmbed {
    fn is_warmed_up(&self) -> bool {
        true
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>, mem_profile::ProfileError> {
        Ok(vec![0.25_f32; 8])
    }
}

fn make_store(dir: &Path) -> ProfileStore {
    let config = common::store_config(dir.to_path_buf());
    ProfileStore::new(
        dir.to_path_buf(),
        config,
        ProfileConfig {
            embedding_min_description_length: 5,
            ..Default::default()
        },
    )
}

fn empty() -> ProfileData {
    ProfileData::default()
}

fn pref(description: &str) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        category: Some("style".to_string()),
        ..Default::default()
    }
}

fn updates(preferences: Vec<ProfileItem>) -> ProfileUpdates {
    ProfileUpdates {
        preferences: Some(preferences),
        ..Default::default()
    }
}

fn cold_buffer_file(dir: &Path) -> PathBuf {
    dir.join(COLD_BUFFER_FILE_NAME)
}

fn read_disk(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(cold_buffer_file(dir)).expect("read snapshot"))
        .expect("snapshot parses")
}

fn descriptions(bucket: &serde_json::Value) -> Vec<String> {
    bucket["preferences"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["description"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn tmp_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.contains(".tmp"))
                .collect()
        })
        .unwrap_or_default()
}

/// Real public merge that buffers a non-explicit observation and persists it.
async fn cold_push(store: &ProfileStore, profile_id: &str, description: &str) {
    store
        .merge_profile_data(
            &empty(),
            &updates(vec![pref(description)]),
            Some(&ColdEmbed),
            Some(profile_id),
        )
        .await
        .expect("cold push");
}

/// Pins the snapshot mtime to a fixed past timestamp (same-mtime / stalled
/// clock world) — `File::set_modified`, no extra crate.
fn pin_mtime(path: &Path, seconds_since_epoch: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime");
    file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds_since_epoch))
        .expect("set mtime");
}

/// True when the process bypasses DAC (euid 0 / CAP_DAC_OVERRIDE): the
/// permission-dependent cases cannot be proven then.
fn process_bypasses_file_modes() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and no side effects.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        Self(common::temp_dir(prefix))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Peer visibility (no mtime-based refresh)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_peer_bucket_visible_when_mtime_pinned() {
    let dir = TempDir::new("coldbuf-mtime-pinned");
    let a = make_store(dir.path()); // peer process
    let b = make_store(dir.path()); // this process

    cold_push(&b, "profile_B", "b own observation").await;
    assert_eq!(descriptions(&read_disk(dir.path())["profile_B"]).len(), 1);

    cold_push(&a, "profile_A", "a peer observation").await;
    pin_mtime(&cold_buffer_file(dir.path()), 1_000_000);

    cold_push(&b, "profile_C", "c later observation").await;

    let disk = read_disk(dir.path());
    assert!(descriptions(&disk["profile_A"]).contains(&"a peer observation".to_string()));
    assert!(descriptions(&disk["profile_B"]).contains(&"b own observation".to_string()));
    assert!(descriptions(&disk["profile_C"]).contains(&"c later observation".to_string()));
}

#[tokio::test]
async fn test_peer_updates_read_when_mtime_moves_backwards() {
    let dir = TempDir::new("coldbuf-mtime-backwards");
    let a = make_store(dir.path());
    let b = make_store(dir.path());

    cold_push(&b, "profile_B", "b first observation").await;
    cold_push(&a, "profile_A", "a peer observation after rollback").await;
    pin_mtime(&cold_buffer_file(dir.path()), 1);

    cold_push(&b, "profile_B", "b second observation").await;

    let disk = read_disk(dir.path());
    assert!(
        descriptions(&disk["profile_A"]).contains(&"a peer observation after rollback".to_string())
    );
    assert_eq!(descriptions(&disk["profile_B"]).len(), 2);
}

#[tokio::test]
async fn test_deleted_cold_buffer_file_not_resurrected_from_stale_map() {
    let dir = TempDir::new("coldbuf-deleted");
    let b = make_store(dir.path());
    cold_push(
        &b,
        "profile_GONE",
        "stale bucket entry that must not resurrect",
    )
    .await;
    assert!(read_disk(dir.path()).get("profile_GONE").is_some());

    std::fs::remove_file(cold_buffer_file(dir.path())).expect("unlink snapshot");

    cold_push(&b, "profile_NEW", "fresh after delete").await;

    let disk = read_disk(dir.path());
    assert!(disk.get("profile_GONE").is_none());
    assert!(descriptions(&disk["profile_NEW"]).contains(&"fresh after delete".to_string()));
}

// ---------------------------------------------------------------------------
// Fail-closed matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_corrupt_cold_buffer_fails_closed_untouched() {
    let dir = TempDir::new("coldbuf-corrupt");
    let corrupt = "{not valid json";
    std::fs::write(cold_buffer_file(dir.path()), corrupt).expect("seed corrupt");

    // Constructing a store over a corrupt cache must not panic (the plugin
    // import path stays fail-safe)...
    let store = make_store(dir.path());
    assert!(store.cold_buffer_load_error().is_some());

    // ...but a merge round must refuse to treat unknown data as empty.
    let error = store
        .merge_profile_data(
            &empty(),
            &updates(vec![pref("must not be persisted over unknown data")]),
            Some(&ColdEmbed),
            Some("profile_X"),
        )
        .await
        .expect_err("corrupt snapshot refuses the round");
    assert!(
        error.to_string().contains("refusing to overwrite"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(cold_buffer_file(dir.path())).expect("read"),
        corrupt
    );
}

#[tokio::test]
async fn test_blank_cold_buffer_fails_closed_for_each_blank_form() {
    for raw in ["", "   ", "\n\t"] {
        let dir = TempDir::new("coldbuf-blank");
        std::fs::write(cold_buffer_file(dir.path()), raw).expect("seed blank");
        let store = make_store(dir.path());

        let error = store
            .merge_profile_data(
                &empty(),
                &updates(vec![pref("never persisted")]),
                Some(&ColdEmbed),
                Some("profile_BLANK"),
            )
            .await
            .expect_err("blank snapshot refuses the round");
        assert!(
            error.to_string().contains("refusing to overwrite"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(cold_buffer_file(dir.path())).expect("read"),
            raw,
            "blank form {raw:?} must be byte-identical after the refusal"
        );
        assert!(tmp_leftovers(dir.path()).is_empty());
    }
}

#[tokio::test]
async fn test_invalid_bucket_shapes_fail_closed_without_touching_bytes() {
    let invalid = [
        r#"{"profile_N":null}"#,
        r#"{"profile_N":"oops"}"#,
        r#"{"profile_N":{"preferences":"nope"}}"#,
        r#"{"profile_N":{"preferences":[null]}}"#,
        r#"{"profile_N":{"patterns":[42]}}"#,
        r#"{"profile_N":{"workflows":[{"category":"x"}]}}"#,
        r#"{"preferences":[],"profile_N":{"patterns":[]}}"#,
        r#"{"preferences":"not-an-array"}"#,
    ];

    for raw in invalid {
        let dir = TempDir::new("coldbuf-shape");
        std::fs::write(cold_buffer_file(dir.path()), raw).expect("seed shape");
        let store = make_store(dir.path());

        let error = store
            .merge_profile_data(
                &empty(),
                &updates(vec![pref("never persisted")]),
                Some(&ColdEmbed),
                Some("profile_BADSHAPE"),
            )
            .await
            .expect_err("invalid shape refuses the round");
        assert!(!error.to_string().is_empty());
        assert_eq!(
            std::fs::read_to_string(cold_buffer_file(dir.path())).expect("read"),
            raw,
            "shape {raw} must be byte-identical after the refusal"
        );
        assert!(
            tmp_leftovers(dir.path()).is_empty(),
            "no tmp leftovers from a failed round ({raw})"
        );
    }
}

#[tokio::test]
async fn test_valid_snapshot_and_legacy_flat_file_accepted() {
    let dir = TempDir::new("coldbuf-accept");
    std::fs::write(
        cold_buffer_file(dir.path()),
        r#"{"profile_KEEP":{"preferences":[{"category":"style","description":"snapshot item kept"}],"patterns":[],"workflows":[]}}"#,
    )
    .expect("seed snapshot");

    let store = make_store(dir.path());
    cold_push(&store, "profile_KEEP", "second snapshot item").await;

    let disk = read_disk(dir.path());
    assert!(descriptions(&disk["profile_KEEP"]).contains(&"snapshot item kept".to_string()));
    assert!(descriptions(&disk["profile_KEEP"]).contains(&"second snapshot item".to_string()));

    // A valid legacy flat file is accepted (dropped by documented policy) and
    // a subsequent merge persists a fresh per-profile snapshot.
    let legacy_dir = TempDir::new("coldbuf-legacy");
    std::fs::write(
        cold_buffer_file(legacy_dir.path()),
        r#"{"preferences":[{"category":"style","description":"legacy item"}],"patterns":[],"workflows":[]}"#,
    )
    .expect("seed legacy");

    let legacy_store = make_store(legacy_dir.path());
    cold_push(&legacy_store, "profile_AFTER_LEGACY", "after legacy drop").await;

    let legacy_disk = read_disk(legacy_dir.path());
    assert!(
        descriptions(&legacy_disk["profile_AFTER_LEGACY"])
            .contains(&"after legacy drop".to_string())
    );
    assert!(
        legacy_disk.get("preferences").is_none(),
        "the legacy flat keys are never re-persisted"
    );
}

// ---------------------------------------------------------------------------
// Permission-dependent failures (self-skip when DAC is bypassed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unreadable_cold_buffer_fails_closed() {
    if process_bypasses_file_modes() {
        return;
    }
    let dir = TempDir::new("coldbuf-unreadable");
    let store = make_store(dir.path());
    cold_push(&store, "profile_P", "existing peer data").await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            cold_buffer_file(dir.path()),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod 000");
    }

    let result = store
        .merge_profile_data(
            &empty(),
            &updates(vec![pref("must not overwrite")]),
            Some(&ColdEmbed),
            Some("profile_Q"),
        )
        .await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            cold_buffer_file(dir.path()),
            std::fs::Permissions::from_mode(0o600),
        );
    }

    let error = result.expect_err("an unreadable snapshot refuses the round");
    assert!(error.to_string().contains("unreadable"), "{error}");
    let disk = read_disk(dir.path());
    assert!(descriptions(&disk["profile_P"]).contains(&"existing peer data".to_string()));
    assert!(disk.get("profile_Q").is_none());
}

#[tokio::test]
async fn test_save_failure_propagates_without_corrupting_target() {
    if process_bypasses_file_modes() {
        return;
    }
    let dir = TempDir::new("coldbuf-save-fail");
    let store = make_store(dir.path());
    cold_push(&store, "profile_S", "already persisted entry").await;
    let before = std::fs::read(cold_buffer_file(dir.path())).expect("read");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500))
            .expect("chmod dir 500");
    }

    let result = store
        .merge_profile_data(
            &empty(),
            &updates(vec![pref("entry that cannot be saved")]),
            Some(&ColdEmbed),
            Some("profile_T"),
        )
        .await;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700));
    }

    let error = result.expect_err("an unwritable dir propagates");
    assert!(error.to_string().contains("save failed"), "{error}");
    assert_eq!(
        std::fs::read(cold_buffer_file(dir.path())).expect("read"),
        before,
        "atomic write: the failed round leaves the previous content intact"
    );

    cold_push(&store, "profile_T", "retry entry").await;
    let disk = read_disk(dir.path());
    assert!(descriptions(&disk["profile_T"]).contains(&"retry entry".to_string()));
    assert!(descriptions(&disk["profile_S"]).contains(&"already persisted entry".to_string()));
}

// ---------------------------------------------------------------------------
// Round / reset / delete semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_one_merge_round_persists_every_buffer_mutation() {
    let dir = TempDir::new("coldbuf-multi");
    let store = make_store(dir.path());

    store
        .merge_profile_data(
            &empty(),
            &ProfileUpdates {
                preferences: Some(vec![pref("pref observation")]),
                patterns: Some(vec![pref("pattern observation")]),
                workflows: Some(vec![pref("workflow observation")]),
            },
            Some(&ColdEmbed),
            Some("profile_MULTI"),
        )
        .await
        .expect("multi-category round");

    let disk = read_disk(dir.path());
    let bucket = &disk["profile_MULTI"];
    assert_eq!(bucket["preferences"].as_array().map(Vec::len), Some(1));
    assert_eq!(bucket["patterns"].as_array().map(Vec::len), Some(1));
    assert_eq!(bucket["workflows"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn test_reset_drops_previous_storages_buckets() {
    let old_dir = TempDir::new("coldbuf-reset-old");
    let new_dir = TempDir::new("coldbuf-reset-new");

    let mut store = make_store(old_dir.path());
    cold_push(&store, "profile_OLD", "belongs to the old storage").await;
    assert!(
        std::fs::read_to_string(cold_buffer_file(old_dir.path()))
            .expect("read old")
            .contains("profile_OLD")
    );

    store.rebind(
        new_dir.path().to_path_buf(),
        common::store_config(new_dir.path().to_path_buf()),
    );
    store.reset();
    cold_push(&store, "profile_NEW", "belongs to the new storage").await;

    let disk = read_disk(new_dir.path());
    assert!(descriptions(&disk["profile_NEW"]).contains(&"belongs to the new storage".to_string()));
    assert!(
        disk.get("profile_OLD").is_none(),
        "the previous storage's buckets must not be written into the new storage"
    );
}

#[tokio::test]
async fn test_delete_profile_removes_bucket_from_current_snapshot() {
    let dir = TempDir::new("coldbuf-delete-profile");
    let a = make_store(dir.path());
    let b = make_store(dir.path()); // constructed before any file existed

    cold_push(&a, "profile_KEEP", "kept bucket").await;
    cold_push(&a, "profile_DOOM", "doomed bucket").await;

    b.delete_profile("profile_DOOM").await.expect("delete");

    let disk = read_disk(dir.path());
    assert!(disk.get("profile_DOOM").is_none());
    assert!(descriptions(&disk["profile_KEEP"]).contains(&"kept bucket".to_string()));
}

#[tokio::test]
async fn test_warm_merge_drains_peer_buffered_bucket_via_real_path() {
    let dir = TempDir::new("coldbuf-warm-drain");
    let a = make_store(dir.path()); // peer buffers while its embedding is cold
    let b = make_store(dir.path());

    cold_push(
        &a,
        "profile_W",
        "peer buffered a warm-drainable observation",
    )
    .await;
    pin_mtime(&cold_buffer_file(dir.path()), 1_000_000);

    let merged = b
        .merge_profile_data(
            &empty(),
            &updates(Vec::new()),
            Some(&WarmEmbed),
            Some("profile_W"),
        )
        .await
        .expect("warm merge");

    let descriptions: Vec<String> = merged
        .preferences
        .iter()
        .map(|item| item.description.clone())
        .collect();
    assert!(descriptions.contains(&"peer buffered a warm-drainable observation".to_string()));

    let disk = read_disk(dir.path());
    assert!(
        disk.get("profile_W").is_none(),
        "a drained bucket is omitted from disk, never resurrected"
    );
}

// ---------------------------------------------------------------------------
// Short-write subprocess cases (fixture_child_entry)
// ---------------------------------------------------------------------------

fn run_short_write_child(
    mode: &str,
    dir: &Path,
    description: &str,
    pre_existing: &str,
) -> serde_json::Value {
    let out = dir.join(format!("fixture-{mode}.out"));
    let storage = dir.display().to_string();
    let mut extra_env: Vec<(&str, String)> = vec![
        ("MEM_FIXTURE_STORAGE", storage),
        ("MEM_FIXTURE_PROFILE", "profile_SW".to_string()),
        ("MEM_FIXTURE_DESC", description.to_string()),
        ("MEM_FIXTURE_PRE", pre_existing.to_string()),
    ];
    let isolated = common::isolated_env(dir);
    for (key, value) in &isolated {
        extra_env.push((Box::leak(key.clone().into_boxed_str()), value.clone()));
    }

    let status = common::run_fixture_child(
        &format!("shortwrite:{mode}"),
        &out,
        &extra_env
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>(),
        Duration::from_secs(30),
    )
    .expect("child runs within the bounded deadline");
    assert!(status.0.success(), "child exited with {status:?}");

    let report_path = out.with_extension("json");
    let raw = std::fs::read_to_string(&report_path).unwrap_or_else(|error| {
        let stderr = std::fs::read_to_string(out.with_extension("stderr")).unwrap_or_default();
        panic!("child produced no report ({error}); stderr: {stderr}")
    });
    serde_json::from_str(&raw).expect("child report is JSON")
}

#[test]
fn test_short_write_ascii_and_utf8_complete_byte_accurate() {
    // Flake gate: 10 iterations per mode.
    for _ in 0..10 {
        for (mode, description) in [
            ("short-ascii", "plain ascii observation"),
            ("short-utf8", "用户偏好：简洁中文回答并附带代码示例"),
        ] {
            let dir = TempDir::new("coldbuf-shortwrite");
            let report = run_short_write_child(mode, dir.path(), description, "");

            assert_eq!(
                report["ok"],
                serde_json::Value::Bool(true),
                "{mode}: {report}"
            );
            assert_eq!(report["tmpLeft"], serde_json::json!(0), "{mode}: {report}");
            let target = report["targetBytes"].as_str().expect("target bytes");
            let parsed: serde_json::Value = serde_json::from_str(target).expect("target parses");
            assert_eq!(
                parsed["profile_SW"]["preferences"][0]["description"], description,
                "{mode}: the persisted description must round-trip byte-exactly"
            );
        }
    }
}

#[test]
fn test_zero_byte_write_stalls_and_preserves_target() {
    let pre_existing = r#"{"profile_PRE":{"preferences":[{"category":"style","description":"previous entry"}],"patterns":[],"workflows":[]}}"#;
    for _ in 0..10 {
        let dir = TempDir::new("coldbuf-zero");
        let report = run_short_write_child(
            "zero",
            dir.path(),
            "entry that cannot be written",
            pre_existing,
        );

        assert_eq!(report["ok"], serde_json::Value::Bool(false), "{report}");
        let error = report["error"].as_str().unwrap_or_default();
        assert!(error.contains("write stalled"), "{report}");
        assert_eq!(report["targetBytes"], serde_json::json!(pre_existing));
        assert_eq!(report["tmpLeft"], serde_json::json!(0));
    }
}

#[test]
fn test_mid_payload_write_error_preserves_target_and_clears_tmp() {
    let pre_existing = r#"{"profile_PRE":{"preferences":[{"category":"style","description":"previous entry"}],"patterns":[],"workflows":[]}}"#;
    for _ in 0..10 {
        let dir = TempDir::new("coldbuf-half");
        let report = run_short_write_child(
            "half",
            dir.path(),
            "entry that fails halfway through the payload",
            pre_existing,
        );

        assert_eq!(report["ok"], serde_json::Value::Bool(false), "{report}");
        assert_eq!(report["targetBytes"], serde_json::json!(pre_existing));
        assert_eq!(report["tmpLeft"], serde_json::json!(0));
    }
}

// ---------------------------------------------------------------------------
// Child entry (self re-exec — `docs/agents/testing.md` recipe)
// ---------------------------------------------------------------------------

/// Unreachable-by-default child entry; harmless no-op when the mode env is
/// unset. Executed by the parent through self re-exec
/// (`--exact fixture_child_entry --ignored`).
#[test]
#[ignore = "fixture subprocess entry — driven only via self re-exec"]
fn fixture_child_entry() {
    let Some(mode) = std::env::var_os(common::FIXTURE_CHILD_ENV) else {
        return;
    };
    let mode = mode.to_string_lossy().to_string();
    let Some(out) = std::env::var_os(common::FIXTURE_OUT_ENV) else {
        return;
    };
    let out = PathBuf::from(out);

    if let Some(fault_name) = mode.strip_prefix("shortwrite:") {
        let storage = std::env::var("MEM_FIXTURE_STORAGE").expect("storage");
        let profile_id = std::env::var("MEM_FIXTURE_PROFILE").expect("profile");
        let description = std::env::var("MEM_FIXTURE_DESC").expect("description");
        let pre_existing = std::env::var("MEM_FIXTURE_PRE").unwrap_or_default();
        let dir = PathBuf::from(&storage);
        let target = dir.join(COLD_BUFFER_FILE_NAME);

        if pre_existing.is_empty() {
            let _ = std::fs::remove_file(&target);
        } else {
            std::fs::write(&target, &pre_existing).expect("seed target");
        }

        let faults = match fault_name {
            "short-ascii" | "short-utf8" => mem_profile::cold_buffer::WriteFaults {
                chunk_limit: Some(3),
                ..Default::default()
            },
            "zero" => mem_profile::cold_buffer::WriteFaults {
                zero_at: Some(0),
                ..Default::default()
            },
            "half" => {
                let mut snapshot = mem_profile::cold_buffer::Snapshot::new();
                let mut bucket = Bucket::new();
                bucket.preferences.push(pref(&description));
                snapshot.insert(profile_id.clone(), bucket);
                let len = mem_profile::cold_buffer::serialize_snapshot(&snapshot)
                    .expect("serialize")
                    .len();
                mem_profile::cold_buffer::WriteFaults {
                    fail_at: Some(len / 2),
                    ..Default::default()
                }
            }
            other => panic!("unknown shortwrite fault mode: {other}"),
        };

        let mut snapshot = mem_profile::cold_buffer::Snapshot::new();
        let mut bucket = Bucket::new();
        bucket.preferences.push(pref(&description));
        snapshot.insert(profile_id, bucket);

        let result = mem_profile::cold_buffer::save_snapshot_with(&target, &snapshot, &faults);
        let target_bytes = std::fs::read_to_string(&target).ok();
        let tmp_left = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
                    .count()
            })
            .unwrap_or(0);

        let report = serde_json::json!({
            "ok": result.is_ok(),
            "error": result.err().map(|error| error.to_string()),
            "targetBytes": target_bytes,
            "tmpLeft": tmp_left,
        });
        let report_path = out.with_extension("json");
        std::fs::write(
            &report_path,
            serde_json::to_vec(&report).expect("serialize report"),
        )
        .expect("write report");
    }
}
