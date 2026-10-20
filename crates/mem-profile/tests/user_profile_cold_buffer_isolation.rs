//! Port of upstream `tests/user-profile-cold-buffer-isolation.test.ts` (W3.1).
//!
//! Upstream case (traced 1:1 by name): "does not drain one user's buffered
//! items into another user's merge".
//!
//! The cold boundary buffers each user's observation under its own profile id;
//! a warm merge for one profile must drain only that profile's bucket. The
//! embedding boundary is mocked (no real model, no network).

mod common;

use mem_profile::profile_store::{
    EmbedBoundary, ProfileConfig, ProfileData, ProfileItem, ProfileStore, ProfileUpdates,
};

/// Upstream `coldEmbed = { isWarmedUp: false }`.
struct ColdEmbed;

impl EmbedBoundary for ColdEmbed {
    fn is_warmed_up(&self) -> bool {
        false
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>, mem_profile::ProfileError> {
        Err(mem_profile::ProfileError::Save(
            "cold boundary must not be asked to embed".to_string(),
        ))
    }
}

/// Upstream `warmEmbed = { isWarmedUp: true, embed: async () => new
/// Float32Array(8).fill(0.25) }`.
struct WarmEmbed;

impl EmbedBoundary for WarmEmbed {
    fn is_warmed_up(&self) -> bool {
        true
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>, mem_profile::ProfileError> {
        Ok(vec![0.25_f32; 8])
    }
}

struct Fixture {
    dir: std::path::PathBuf,
    store: ProfileStore,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let dir = common::temp_dir(prefix);
        let config = common::store_config(dir.clone());
        Self {
            store: ProfileStore::new(
                dir.clone(),
                config,
                ProfileConfig {
                    embedding_min_description_length: 5,
                    ..Default::default()
                },
            ),
            dir,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.dir);
    }
}

fn empty() -> ProfileData {
    ProfileData::default()
}

fn updates(preferences: Vec<ProfileItem>) -> ProfileUpdates {
    ProfileUpdates {
        preferences: Some(preferences),
        ..Default::default()
    }
}

fn observation(description: &str) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        category: Some("style".to_string()),
        ..Default::default()
    }
}

fn descriptions(preferences: &[ProfileItem]) -> Vec<String> {
    preferences
        .iter()
        .map(|item| item.description.clone())
        .collect()
}

#[tokio::test]
async fn test_warm_merge_drains_only_the_same_profiles_bucket() {
    let fixture = Fixture::new("coldbuf-isolation");
    let cold = ColdEmbed;
    let warm = WarmEmbed;

    // Cold start: each user's observation is buffered under its own profileId.
    fixture
        .store
        .merge_profile_data(
            &empty(),
            &updates(vec![observation("A prefers tabs over spaces")]),
            Some(&cold),
            Some("profile_A"),
        )
        .await
        .expect("cold push for A");
    fixture
        .store
        .merge_profile_data(
            &empty(),
            &updates(vec![observation("B prefers spaces over tabs")]),
            Some(&cold),
            Some("profile_B"),
        )
        .await
        .expect("cold push for B");

    // Warm merge for B must drain only B's bucket, never A's.
    let merged_b = fixture
        .store
        .merge_profile_data(
            &empty(),
            &updates(Vec::new()),
            Some(&warm),
            Some("profile_B"),
        )
        .await
        .expect("warm drain for B");
    let descriptions_b = descriptions(&merged_b.preferences);
    assert!(
        descriptions_b.contains(&"B prefers spaces over tabs".to_string()),
        "B's buffered observation must drain into B's merge: {descriptions_b:?}"
    );
    assert!(
        !descriptions_b.contains(&"A prefers tabs over spaces".to_string()),
        "A's buffered observation must never leak into B: {descriptions_b:?}"
    );

    // A's bucket is untouched by B's drain and drains only for A.
    let merged_a = fixture
        .store
        .merge_profile_data(
            &empty(),
            &updates(Vec::new()),
            Some(&warm),
            Some("profile_A"),
        )
        .await
        .expect("warm drain for A");
    let descriptions_a = descriptions(&merged_a.preferences);
    assert!(
        descriptions_a.contains(&"A prefers tabs over spaces".to_string()),
        "A's buffered observation must drain into A's merge: {descriptions_a:?}"
    );
    assert!(
        !descriptions_a.contains(&"B prefers spaces over tabs".to_string()),
        "B's buffered observation must never leak into A: {descriptions_a:?}"
    );

    // Both buckets are now empty and therefore omitted from the snapshot.
    let snapshot = fixture.store.cold_snapshot();
    assert!(
        snapshot.values().all(|bucket| bucket.is_empty()),
        "both drained buckets are empty: {snapshot:?}"
    );
}
