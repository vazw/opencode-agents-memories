//! Port of upstream `tests/profile-write.test.ts` (W3.1).
//!
//! The upstream file exercises the explicit-preference write path through the
//! underlying manager (no live plugin context). Cases, traced 1:1 by name:
//! - "creates a profile with an explicit preference when none exists"
//! - "merges a new explicit preference into an existing profile without
//!   clobbering other prefs"
//! - "deduplicates when the same explicit preference is written twice,
//!   boosting confidence"
//! - "returns null profile for unknown user (no auto-create on read)"
//! - "changelog entry is recorded on explicit preference write"
//!
//! Upstream's `CONFIG.storagePath` mutation becomes the explicit
//! `storage_path` argument (the established W1.x/W2.x divergence), so two
//! stores over one directory model two peer processes.

mod common;

use mem_profile::profile_store::{
    EmbedBoundary, ProfileChangelog, ProfileData, ProfileItem, ProfileStore, ProfileUpdates,
};

const MIN_DESC_LEN: u32 = 5;

struct Fixture {
    dir: std::path::PathBuf,
    store: ProfileStore,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let dir = common::temp_dir(prefix);
        let config = common::store_config(dir.clone());
        let profile_config = mem_profile::profile_store::ProfileConfig {
            embedding_min_description_length: MIN_DESC_LEN,
            ..Default::default()
        };
        Self {
            store: ProfileStore::new(dir.clone(), config, profile_config),
            dir,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.dir);
    }
}

/// Upstream `empty()`.
fn empty() -> ProfileData {
    ProfileData::default()
}

/// Upstream `{ category, description, confidence, frequency, evidence,
/// lastSeen }` literal.
fn explicit_pref(description: &str) -> ProfileItem {
    ProfileItem {
        description: description.to_string(),
        category: Some("explicit".to_string()),
        confidence: Some(1.0),
        frequency: Some(1),
        evidence: Some(vec!["manual-write".to_string()]),
        last_seen: Some(mem_profile::ids::now_millis()),
        ..Default::default()
    }
}

fn updates(preferences: Vec<ProfileItem>) -> ProfileUpdates {
    ProfileUpdates {
        preferences: Some(preferences),
        ..Default::default()
    }
}

/// Parses a stored `profile_data` JSON payload.
fn parse_data(profile: &mem_profile::profile_store::Profile) -> ProfileData {
    serde_json::from_str(&profile.profile_data).expect("profile_data parses")
}

#[tokio::test]
async fn test_create_profile_with_explicit_preference_when_none_exists() {
    let fixture = Fixture::new("profile-create");
    let user_id = "test@example.com";

    fixture
        .store
        .create_profile(
            user_id,
            "Test User",
            "testuser",
            user_id,
            &ProfileData {
                preferences: vec![explicit_pref("Prefer concise answers")],
                ..Default::default()
            },
            0,
        )
        .await
        .expect("create profile");

    let profile = fixture
        .store
        .get_active_profile(user_id)
        .await
        .expect("read profile")
        .expect("profile exists");
    let data = parse_data(&profile);

    assert_eq!(data.preferences.len(), 1);
    assert_eq!(data.preferences[0].description, "Prefer concise answers");
    assert_eq!(data.preferences[0].confidence, Some(1.0));
    assert!(
        data.preferences[0]
            .evidence
            .as_ref()
            .is_some_and(|evidence| evidence.contains(&"manual-write".to_string())),
        "evidence must survive the round trip"
    );
}

#[tokio::test]
async fn test_merge_new_explicit_preference_into_existing_profile_without_clobbering() {
    let fixture = Fixture::new("profile-merge-new");
    let user_id = "test@example.com";

    // Seed with one AI-learned preference.
    fixture
        .store
        .create_profile(
            user_id,
            "Test User",
            "testuser",
            user_id,
            &ProfileData {
                preferences: vec![ProfileItem {
                    description: "Uses TypeScript".to_string(),
                    category: Some("style".to_string()),
                    confidence: Some(0.8),
                    frequency: Some(1),
                    evidence: Some(vec!["observed".to_string()]),
                    last_seen: Some(mem_profile::ids::now_millis()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            3,
        )
        .await
        .expect("create profile");

    let existing = fixture
        .store
        .get_active_profile(user_id)
        .await
        .expect("read")
        .expect("exists");
    let existing_data = parse_data(&existing);

    let merged = fixture
        .store
        .merge_profile_data(
            &existing_data,
            &updates(vec![explicit_pref("Always use numbered lists")]),
            None,
            None,
        )
        .await
        .expect("merge");

    assert!(
        fixture
            .store
            .update_profile(
                &existing.id,
                &merged,
                0,
                "Explicit preference added: Always use numbered lists",
            )
            .await
            .expect("update"),
        "the optimistic-version update must win on the first attempt"
    );

    let updated = fixture
        .store
        .get_active_profile(user_id)
        .await
        .expect("read")
        .expect("exists");
    let updated_data = parse_data(&updated);

    assert_eq!(updated_data.preferences.len(), 2);
    let descriptions: Vec<&str> = updated_data
        .preferences
        .iter()
        .map(|item| item.description.as_str())
        .collect();
    assert!(descriptions.contains(&"Uses TypeScript"));
    assert!(descriptions.contains(&"Always use numbered lists"));
    assert_eq!(updated.version, 2, "version bumps by exactly one");
}

#[tokio::test]
async fn test_duplicate_explicit_preference_deduplicates_and_boosts_confidence_capped() {
    let fixture = Fixture::new("profile-merge-dup");
    let user_id = "test@example.com";
    let description = "Prefer short answers";

    fixture
        .store
        .create_profile(
            user_id,
            "Test User",
            "testuser",
            user_id,
            &ProfileData {
                preferences: vec![explicit_pref(description)],
                ..Default::default()
            },
            0,
        )
        .await
        .expect("create profile");

    let first = fixture
        .store
        .get_active_profile(user_id)
        .await
        .expect("read")
        .expect("exists");
    let first_data = parse_data(&first);

    let merged = fixture
        .store
        .merge_profile_data(
            &first_data,
            &updates(vec![explicit_pref(description)]),
            None,
            None,
        )
        .await
        .expect("merge");

    fixture
        .store
        .update_profile(&first.id, &merged, 0, "Explicit preference added")
        .await
        .expect("update");

    let second = fixture
        .store
        .get_active_profile(user_id)
        .await
        .expect("read")
        .expect("exists");
    let second_data = parse_data(&second);

    let matching: Vec<&ProfileItem> = second_data
        .preferences
        .iter()
        .filter(|item| item.description == description)
        .collect();
    assert_eq!(matching.len(), 1, "deduplicated by (category, description)");

    let confidence = matching[0].confidence.expect("confidence present");
    assert!(
        confidence <= 1.0,
        "confidence is capped at 1.0, got {confidence}"
    );
    assert!(
        confidence > 0.6,
        "confidence must stay above the upstream floor, got {confidence}"
    );
    assert_eq!(
        matching[0].frequency,
        Some(2),
        "a confirmed duplicate increments frequency"
    );
}

#[tokio::test]
async fn test_get_active_profile_returns_null_for_unknown_user_no_autocreate() {
    let fixture = Fixture::new("profile-missing");

    assert!(
        fixture
            .store
            .get_active_profile("nobody@example.com")
            .await
            .expect("read")
            .is_none(),
        "reading an unknown user must not auto-create a profile"
    );
}

#[tokio::test]
async fn test_changelog_entry_recorded_on_explicit_preference_write() {
    let fixture = Fixture::new("profile-changelog");
    let user_id = "test@example.com";
    let summary = "Explicit preference added: Use snake_case";

    fixture
        .store
        .create_profile(user_id, "Test User", "testuser", user_id, &empty(), 0)
        .await
        .expect("create profile");

    let profile = fixture
        .store
        .get_active_profile(user_id)
        .await
        .expect("read")
        .expect("exists");
    let data = parse_data(&profile);

    let merged = fixture
        .store
        .merge_profile_data(
            &data,
            &updates(vec![explicit_pref("Use snake_case")]),
            None,
            None,
        )
        .await
        .expect("merge");

    fixture
        .store
        .update_profile(&profile.id, &merged, 0, summary)
        .await
        .expect("update");

    let changelogs: Vec<ProfileChangelog> = fixture
        .store
        .get_profile_changelogs(&profile.id, 10)
        .await
        .expect("changelogs");

    let last = changelogs.first().expect("at least one changelog row");
    assert_eq!(last.change_summary, summary);
    assert_eq!(last.change_type, "update");
}

/// A warm embedding boundary whose `embed` result is seeded, used by the
/// later cold-buffer slices — kept here so the seam's shape is pinned by this
/// file too.
#[allow(dead_code)]
struct WarmEmbed {
    vector: Vec<f32>,
}

impl EmbedBoundary for WarmEmbed {
    fn is_warmed_up(&self) -> bool {
        true
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>, mem_profile::ProfileError> {
        Ok(self.vector.clone())
    }
}
