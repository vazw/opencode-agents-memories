//! New coverage for the incremental cleanup scope + hook (W3.3).
//!
//! Behavioural origin: upstream `user-memory-learning.ts` L15-60 (the interval
//! gate and the automatic-cleanup hook) combined with the issue's
//! "incremental, never a full scan" requirement (R6): the scope is the landed
//! trait plus its conflict-band neighbours from `mem_core::dedup`.
//!
//! Tests:
//! - `test_cleanup_scope_includes_the_landed_trait_and_its_conflict_band_neighbour`
//! - `test_incremental_cleanup_merges_the_conflict_pair_and_preserves_items_outside_the_scope`
//! - `test_incremental_cleanup_leaves_the_profile_untouched_when_nothing_is_merged_or_removed`

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use mem_profile::ai_cleanup::ProfileCleanupGateway;
use mem_profile::incremental_cleanup::{
    CleanupOutcome, IncrementalCleanup, ProfileCleanupHook, cleanup_scope,
};
use mem_profile::learning_service::AnalysisError;
use mem_profile::profile_store::{
    ProfileConfig, ProfileData, ProfileItem, ProfileStore, ProfileUpdates,
};
use mem_profile::provider_gateway::StructuredCompletion;
use mem_store::db::BoxFuture;

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

struct FakeCompletion {
    response: Value,
    calls: Arc<Mutex<usize>>,
}

impl FakeCompletion {
    fn returning(response: Value) -> Self {
        Self {
            response,
            calls: Arc::new(Mutex::new(0)),
        }
    }
}

impl StructuredCompletion for FakeCompletion {
    fn complete<'a>(
        &'a self,
        _system_prompt: &'a str,
        _user_prompt: &'a str,
        _schema: Value,
    ) -> BoxFuture<'a, Result<Value, AnalysisError>> {
        *self.calls.lock().expect("call counter") += 1;
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }
}

fn item(description: &str, category: &str, centroid: &[f32]) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        category: Some(category.to_string()),
        confidence: Some(0.6),
        frequency: Some(1),
        evidence: Some(vec!["e".to_string()]),
        last_seen: Some(0),
        centroid: Some(centroid.to_vec()),
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

fn landed(descriptions: &[(&str, &str)]) -> ProfileUpdates {
    ProfileUpdates {
        preferences: Some(
            descriptions
                .iter()
                .map(|(description, category)| ProfileItem {
                    description: description.to_string(),
                    category: Some(category.to_string()),
                    ..ProfileItem::default()
                })
                .collect(),
        ),
        patterns: Some(Vec::new()),
        workflows: Some(Vec::new()),
    }
}

fn empty_mapping_response() -> Value {
    json!({
        "preferences": [],
        "patterns": [],
        "workflows": [],
        "mapping": {"kept": [], "merged": [], "removed": []}
    })
}

/// Creates a three-item profile: two band neighbours (cos 0.8) and one item in
/// another category that must stay outside the scope.
async fn seed_profile(store: &ProfileStore) -> String {
    store
        .create_profile(
            "user@example.com",
            "Unknown",
            "unknown",
            "user@example.com",
            &data(vec![
                item("uses tabs", "style", &[1.0, 0.0]),
                item("prefers tab indentation", "style", &[0.8, 0.6]),
                item("outside item", "other", &[0.0, 1.0]),
            ]),
            3,
        )
        .await
        .expect("create profile")
}

#[test]
fn test_cleanup_scope_includes_the_landed_trait_and_its_conflict_band_neighbour() {
    let profile = data(vec![
        item("uses tabs", "style", &[1.0, 0.0]),
        item("prefers tab indentation", "style", &[0.8, 0.6]),
        // cos 0.6 with the second item — below the 0.65 band floor.
        item("far away", "style", &[0.0, 1.0]),
    ]);

    let scope = cleanup_scope(&profile, &landed(&[("uses tabs", "style")]));

    assert_eq!(
        scope,
        vec!["pref_0".to_string(), "pref_1".to_string()],
        "the landed trait and its in-band neighbour, nothing else"
    );
}

#[tokio::test]
async fn test_incremental_cleanup_merges_the_conflict_pair_and_preserves_items_outside_the_scope() {
    let dir = TempDir::new("incremental-merge");
    let store = Arc::new(ProfileStore::new(
        dir.path().to_path_buf(),
        common::store_config(dir.path().to_path_buf()),
        ProfileConfig::default(),
    ));
    let profile_id = seed_profile(&store).await;

    let completion = Arc::new(FakeCompletion::returning(json!({
        "preferences": [
            {"id": "pref_0", "category": "style", "description": "uses tabs"}
        ],
        "patterns": [],
        "workflows": [],
        "mapping": {"kept": ["pref_0"], "merged": [["pref_0", "pref_1"]], "removed": []}
    })));
    let gateway = Arc::new(ProfileCleanupGateway::new(Some(completion.clone()), None));
    let hook = ProfileCleanupHook::new(store.clone(), gateway, ProfileConfig::default());

    let outcome = hook
        .run_incremental(
            "user@example.com",
            &landed(&[("uses tabs", "style")]),
            0,
            1,
            1_000,
        )
        .await
        .expect("cleanup runs");

    assert_eq!(
        outcome,
        CleanupOutcome {
            merged: 1,
            removed: 0,
            updated: true,
        }
    );
    assert_eq!(*completion.calls.lock().expect("call counter"), 1);

    let stored = store
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(stored.version, 2, "one optimistic-version write");

    let persisted: ProfileData = serde_json::from_str(&stored.profile_data).expect("parse");
    let mut descriptions: Vec<&str> = persisted
        .preferences
        .iter()
        .map(|item| item.description.as_str())
        .collect();
    descriptions.sort_unstable();
    assert_eq!(
        descriptions,
        vec!["outside item", "uses tabs"],
        "the pair collapses to one item and the out-of-scope item survives"
    );
}

#[tokio::test]
async fn test_incremental_cleanup_leaves_the_profile_untouched_when_nothing_is_merged_or_removed() {
    let dir = TempDir::new("incremental-noop");
    let store = Arc::new(ProfileStore::new(
        dir.path().to_path_buf(),
        common::store_config(dir.path().to_path_buf()),
        ProfileConfig::default(),
    ));
    let profile_id = seed_profile(&store).await;

    let completion = Arc::new(FakeCompletion::returning(empty_mapping_response()));
    let gateway = Arc::new(ProfileCleanupGateway::new(Some(completion), None));
    let hook = ProfileCleanupHook::new(store.clone(), gateway, ProfileConfig::default());

    let outcome = hook
        .run_incremental(
            "user@example.com",
            &landed(&[("uses tabs", "style")]),
            0,
            1,
            1_000,
        )
        .await
        .expect("cleanup runs");

    assert_eq!(
        outcome,
        CleanupOutcome {
            merged: 0,
            removed: 0,
            updated: false,
        },
        "upstream's early return: no merges and no removals means no write"
    );

    let stored = store
        .get_profile_by_id(&profile_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    assert_eq!(stored.version, 1, "nothing was written");

    let persisted: ProfileData = serde_json::from_str(&stored.profile_data).expect("parse");
    let mut descriptions: Vec<&str> = persisted
        .preferences
        .iter()
        .map(|item| item.description.as_str())
        .collect();
    descriptions.sort_unstable();
    assert_eq!(
        descriptions,
        vec!["outside item", "prefers tab indentation", "uses tabs"],
        "every item is still there"
    );
}
