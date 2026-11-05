//! New coverage for the store-level decay seam (W3.3).
//!
//! Behavioural origin: upstream `user-profile-decay.test.ts` +
//! `user-profile-manager.ts::decayInMemory` L559-635. The tests drive the
//! public seam (`ProfileStore::decay_in_memory` / `ProfileStore::sweep_decay_daily`)
//! over a real sqlite-backed store under a temp dir, always with an injected
//! `now` — there is no sleep and no wall clock in any assertion.
//!
//! Justified additions over the 5 upstream cases: the α = 0.3 blend case
//! (verdict item 4), plus the two persisted-sweep cases (R5).
//!
//! Tests:
//! - `test_stale_low_evidence_items_are_removed_after_stale_days`
//! - `test_items_with_enough_evidence_are_kept_even_when_stale`
//! - `test_items_within_stale_days_are_kept_regardless_of_evidence`
//! - `test_custom_stale_days_and_min_evidence_config_are_respected`
//! - `test_confidence_one_migrates_continuously_instead_of_dropping_to_the_legacy_floor`
//! - `test_sync_confidence_blends_the_stored_prior_with_the_beta_observation_at_alpha_three_tenths`
//! - `test_daily_sweep_persists_decayed_confidence_and_bumps_the_profile_version`
//! - `test_daily_sweep_is_a_noop_within_its_interval`

mod common;

use std::path::PathBuf;

use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileItem, ProfileStore};

const DAY_MS: i64 = 86_400_000;

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        Self(common::temp_dir(prefix))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.0);
    }
}

/// A store over a fresh temp dir, with the default profile config unless a
/// caller overrides it (no DB is opened until the first async call).
fn store(dir: &std::path::Path) -> ProfileStore {
    ProfileStore::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
        ProfileConfig::default(),
    )
}

fn store_with(dir: &std::path::Path, config: ProfileConfig) -> ProfileStore {
    ProfileStore::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
        config,
    )
}

/// A preference with an explicit Beta posterior (so `lazy_migrate_alpha` is a
/// no-op) and a caller-chosen stored confidence.
fn item(description: &str, confidence: f64, evidence: usize, last_seen: i64) -> ProfileItem {
    ProfileItem {
        alpha: Some(5.0),
        beta: Some(1.0),
        ..base_item(description, confidence, evidence, last_seen)
    }
}

/// A legacy preference: no `alpha`/`beta`, so `lazy_migrate_alpha` seeds the
/// posterior from the stored confidence (upstream's pre-W3.1 shape).
fn legacy_item(description: &str, confidence: f64, evidence: usize, last_seen: i64) -> ProfileItem {
    base_item(description, confidence, evidence, last_seen)
}

fn base_item(description: &str, confidence: f64, evidence: usize, last_seen: i64) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        category: Some("style".to_string()),
        confidence: Some(confidence),
        frequency: Some(1),
        evidence: Some(
            (0..evidence)
                .map(|index| format!("evidence-{index}"))
                .collect(),
        ),
        last_seen: Some(last_seen),
        ..ProfileItem::default()
    }
}

fn data(items: Vec<ProfileItem>) -> ProfileData {
    ProfileData {
        preferences: items,
        patterns: Vec::new(),
        workflows: Vec::new(),
        learning_paths: None,
    }
}

#[test]
fn test_stale_low_evidence_items_are_removed_after_stale_days() {
    let dir = TempDir::new("decay-stale-removed");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let outcome = store.decay_in_memory(
        &data(vec![item("stale low evidence", 0.4, 1, now - 3 * DAY_MS)]),
        now,
    );

    assert!(outcome.has_changes);
    assert!(
        outcome.data.preferences.is_empty(),
        "a stale, low-evidence trait fades instead of becoming a permanent rule"
    );
}

#[test]
fn test_items_with_enough_evidence_are_kept_even_when_stale() {
    let dir = TempDir::new("decay-stale-kept");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let outcome = store.decay_in_memory(
        &data(vec![item("well evidenced", 0.6, 3, now - 10 * DAY_MS)]),
        now,
    );

    assert_eq!(outcome.data.preferences.len(), 1);
    assert_eq!(outcome.data.preferences[0].description, "well evidenced");
}

#[test]
fn test_items_within_stale_days_are_kept_regardless_of_evidence() {
    let dir = TempDir::new("decay-fresh-kept");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let outcome = store.decay_in_memory(&data(vec![item("fresh", 0.3, 1, now - DAY_MS)]), now);

    assert_eq!(outcome.data.preferences.len(), 1);
    assert_eq!(outcome.data.preferences[0].description, "fresh");
}

#[test]
fn test_custom_stale_days_and_min_evidence_config_are_respected() {
    let dir = TempDir::new("decay-custom-config");
    let store = store_with(
        dir.path(),
        ProfileConfig {
            stale_days: 1,
            min_evidence_for_retention: 5,
            ..ProfileConfig::default()
        },
    );
    let now = mem_profile::ids::now_millis();

    // 4 evidence < 5 and age 2 days > 1 day → removed (not the old
    // hardcoded 30-day/alpha gate).
    let outcome = store.decay_in_memory(
        &data(vec![item("custom threshold", 0.4, 4, now - 2 * DAY_MS)]),
        now,
    );

    assert!(outcome.has_changes);
    assert!(outcome.data.preferences.is_empty());
}

#[test]
fn test_confidence_one_migrates_continuously_instead_of_dropping_to_the_legacy_floor() {
    let dir = TempDir::new("decay-confidence-one");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    // A legacy item: confidence 1.0 with no alpha/beta. `lazy_migrate_alpha`
    // seeds alpha ≈ 100, so the α = 0.3 blend of the stored prior (1.0) with
    // the Beta estimator (≈ 0.9901) stays ≈ 0.995 — never a hard drop.
    let outcome = store.decay_in_memory(
        &data(vec![legacy_item("maximum confidence", 1.0, 3, now)]),
        now,
    );

    assert_eq!(outcome.data.preferences.len(), 1);
    let confidence = outcome.data.preferences[0]
        .confidence
        .expect("confidence is always re-derived");
    assert!(
        confidence > 0.98 && confidence <= 1.0,
        "confidence 1.0 migrates continuously, got {confidence}"
    );
}

#[test]
fn test_sync_confidence_blends_the_stored_prior_with_the_beta_observation_at_alpha_three_tenths() {
    let dir = TempDir::new("decay-ema-blend");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    // Stored prior 0.4, Beta posterior 9:1 ⇒ observation 0.9.
    // The α = 0.3 EMA applied to that pair is 0.3·0.9 + 0.7·0.4 = 0.55.
    // Issue 102 eliminated the duplicate sync_confidence in decay_items,
    // so the single blend 0.55 is returned.
    let mut probe = item("blended prior", 0.4, 3, now);
    probe.alpha = Some(9.0);
    probe.beta = Some(1.0);

    let outcome = store.decay_in_memory(&data(vec![probe]), now);

    let confidence = outcome.data.preferences[0]
        .confidence
        .expect("confidence is always re-derived");
    assert!(
        (confidence - 0.55).abs() < 1e-9,
        "expected the α = 0.3 blend 0.55, got {confidence}"
    );
    assert!(
        (confidence - 0.9).abs() > 0.1,
        "the Beta mean alone (0.9) is not the answer"
    );
}

#[tokio::test]
async fn test_daily_sweep_persists_decayed_confidence_and_bumps_the_profile_version() {
    let dir = TempDir::new("decay-sweep-persists");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    // 90 days old (well past the 30-day grace threshold) with enough evidence
    // to be retained: the frequency-1 half-life is 60 days, so the decay
    // factor is exp(-ln2·60d/60d) = 0.5 — 0.8 becomes 0.4.
    let profile_data = data(vec![item("decay me", 0.8, 3, now - 90 * DAY_MS)]);
    let profile_id = store
        .create_profile(
            "decay@example.com",
            "Unknown",
            "unknown",
            "decay@example.com",
            &profile_data,
            7,
        )
        .await
        .expect("create profile");

    let report = store.sweep_decay_daily(now).await.expect("sweep");

    assert_eq!(report.profiles_scanned, 1);
    assert_eq!(report.profiles_updated, 1);

    let stored = store
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(
        stored.version, 2,
        "the sweep persists through update_profile"
    );

    let persisted: ProfileData = serde_json::from_str(&stored.profile_data).expect("parse");
    let confidence = persisted.preferences[0]
        .confidence
        .expect("confidence persisted");
    // 0.8 with a 5:1 Beta posterior: sync gives 0.3·0.8333 + 0.7·0.8 = 0.81,
    // then the 90-day age (60-day frequency-1 half-life, one half-life past
    // the 30-day grace threshold) halves it to 0.405. With the duplicate
    // sync_confidence eliminated (Issue 102), the sweep persists 0.405.
    assert!(
        (confidence - 0.405).abs() < 1e-6,
        "the decayed confidence is persisted, got {confidence}"
    );
    assert!(
        confidence < 0.8,
        "the sweep decayed the stored confidence, got {confidence}"
    );

    let changelogs = store
        .get_profile_changelogs(&profile_id, 10)
        .await
        .expect("changelogs");
    assert!(
        changelogs
            .iter()
            .any(|row| row.change_summary == "Decay sweep: confidence decayed"),
        "the sweep writes through the normal changelog path: {changelogs:?}"
    );
}

#[tokio::test]
async fn test_daily_sweep_is_a_noop_within_its_interval() {
    let dir = TempDir::new("decay-sweep-interval");
    let store = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let profile_data = data(vec![item("decay me", 0.8, 3, now - 90 * DAY_MS)]);
    let profile_id = store
        .create_profile(
            "decay@example.com",
            "Unknown",
            "unknown",
            "decay@example.com",
            &profile_data,
            7,
        )
        .await
        .expect("create profile");

    let first = store.sweep_decay_daily(now).await.expect("first sweep");
    assert_eq!(first.profiles_updated, 1);

    // One millisecond later the daily gate is not crossed: nothing is scanned,
    // nothing is written.
    let second = store
        .sweep_decay_daily(now + 1)
        .await
        .expect("second sweep");
    assert_eq!(second.profiles_scanned, 0);
    assert_eq!(second.profiles_updated, 0);

    let stored = store
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(stored.version, 2, "the no-op sweep wrote nothing");
}

#[tokio::test]
async fn test_daily_sweep_survives_restart_within_interval() {
    let dir = TempDir::new("decay-restart-within");
    let store1 = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let profile_data = data(vec![item("decay me", 0.8, 3, now - 90 * DAY_MS)]);
    let profile_id = store1
        .create_profile(
            "decay@example.com",
            "Unknown",
            "unknown",
            "decay@example.com",
            &profile_data,
            7,
        )
        .await
        .expect("create profile");

    let report1 = store1.sweep_decay_daily(now).await.expect("sweep");
    assert_eq!(report1.profiles_scanned, 1);
    assert_eq!(report1.profiles_updated, 1);

    drop(store1);

    let store2 = store(dir.path());
    let report2 = store2
        .sweep_decay_daily(now + 3_600_000)
        .await
        .expect("sweep within interval");
    assert_eq!(report2.profiles_scanned, 0);
    assert_eq!(report2.profiles_updated, 0);

    let stored = store2
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(
        stored.version, 2,
        "profile version is unchanged when sweep is noop"
    );
}

#[tokio::test]
async fn test_daily_sweep_sweeps_after_restart_past_interval() {
    let dir = TempDir::new("decay-restart-past");
    let store1 = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let profile_data = data(vec![item("decay me", 0.8, 3, now - 90 * DAY_MS)]);
    let profile_id = store1
        .create_profile(
            "decay@example.com",
            "Unknown",
            "unknown",
            "decay@example.com",
            &profile_data,
            7,
        )
        .await
        .expect("create profile");

    let report1 = store1.sweep_decay_daily(now).await.expect("sweep");
    assert_eq!(report1.profiles_scanned, 1);
    assert_eq!(report1.profiles_updated, 1);

    drop(store1);

    let store2 = store(dir.path());
    let report2 = store2
        .sweep_decay_daily(now + 25 * 3_600_000)
        .await
        .expect("sweep past interval");
    assert!(report2.profiles_scanned >= 1);
    assert!(report2.profiles_updated >= 1);

    let stored = store2
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(
        stored.version, 3,
        "profile version bumped on second sweep after 25h"
    );

    let persisted: ProfileData = serde_json::from_str(&stored.profile_data).expect("parse");
    let confidence = persisted.preferences[0]
        .confidence
        .expect("confidence persisted");
    assert!(
        confidence < 0.405,
        "confidence decayed further after 25h, got {confidence}"
    );
}

#[tokio::test]
async fn test_daily_sweep_stamp_degrades_fail_safe_on_corrupt_file() {
    let dir = TempDir::new("decay-corrupt-stamp");
    let store1 = store(dir.path());
    let now = mem_profile::ids::now_millis();

    let profile_data = data(vec![item("decay me", 0.8, 3, now - 90 * DAY_MS)]);
    let profile_id = store1
        .create_profile(
            "decay@example.com",
            "Unknown",
            "unknown",
            "decay@example.com",
            &profile_data,
            7,
        )
        .await
        .expect("create profile");

    // Manually write corrupt content to the decay-sweep.json sidecar file
    std::fs::write(store1.decay_sweep_path(), b"invalid-json-payload")
        .expect("write corrupt stamp");

    let new_store = store(dir.path());
    assert_eq!(
        *new_store.last_decay_sweep.lock().unwrap(),
        None,
        "corrupt stamp must degrade fail-safe to None (due)"
    );

    let report = new_store
        .sweep_decay_daily(now)
        .await
        .expect("sweep succeeds despite corrupt stamp");
    assert!(
        report.profiles_scanned >= 1,
        "sweep must run (fail toward decaying, never skipping forever)"
    );

    // Assert new_store.decay_sweep_path() now contains valid JSON with timestamp now
    let raw = std::fs::read_to_string(new_store.decay_sweep_path()).expect("read stamp file");
    let loaded: mem_profile::decay_sweep::DecaySweepStamp =
        serde_json::from_str(&raw).expect("stamp file must now be valid json");
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.last_decay_sweep, now);

    // Second sweep at now + 1: assert profiles_scanned == 0
    let second_report = new_store
        .sweep_decay_daily(now + 1)
        .await
        .expect("second sweep");
    assert_eq!(
        second_report.profiles_scanned, 0,
        "second sweep within interval must be noop"
    );

    let stored = new_store
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(stored.version, 2);
}
