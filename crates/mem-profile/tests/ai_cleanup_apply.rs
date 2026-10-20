//! Port of `tests/ai-cleanup-apply.test.ts` (9 upstream cases) plus the PRD's
//! α = 0.3 merge-fold case.
//!
//! Every apply assertion is pure: `merge_cleanup_into_profile` and
//! `rebuild_profile_using` are total functions over their arguments, and the
//! guarantee under test is "no data loss outside the merged pair" — only the
//! ids in scope can change disposition, and a rejected merge/removal restores
//! the original items from `old_profile_data`.
//!
//! Tests:
//! - `test_partial_apply_preserves_unselected_items_when_applying_a_scoped_cleanup`
//! - `test_rejected_removal_restores_the_item`
//! - `test_accepted_removal_removes_only_the_selected_item`
//! - `test_rejected_merge_restores_the_merge_sources`
//! - `test_items_added_after_analysis_are_preserved`
//! - `test_unset_include_ids_replaces_the_whole_profile`
//! - `test_duplicate_descriptions_remove_only_the_selected_occurrence`
//! - `test_rejected_merge_does_not_duplicate_items_whose_metadata_changed`
//! - `test_rebuild_profile_using_does_not_preserve_merge_sources_as_unmentioned`
//! - `test_merged_pair_confidence_is_the_ema_of_target_and_source_at_alpha_three_tenths`

use std::collections::BTreeMap;

use mem_profile::ai_cleanup::{CleanupMapping, MergedGroup, rebuild_profile_using};
use mem_profile::cleanup_apply::{CleanupApplyArgs, merge_cleanup_into_profile};
use mem_profile::profile_store::{ProfileData, ProfileItem};

fn pref(description: &str) -> ProfileItem {
    ProfileItem {
        category: Some("style".to_string()),
        description: description.to_string(),
        confidence: Some(0.5),
        evidence: Some(vec!["e1".to_string()]),
        ..ProfileItem::default()
    }
}

fn base_profile(descriptions: &[&str]) -> ProfileData {
    ProfileData {
        preferences: descriptions.iter().map(|d| pref(d)).collect(),
        patterns: Vec::new(),
        workflows: Vec::new(),
        learning_paths: None,
    }
}

fn descriptions(profile: &ProfileData) -> Vec<&str> {
    profile
        .preferences
        .iter()
        .map(|item| item.description.as_str())
        .collect()
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn groups(values: &[&[&str]]) -> Vec<Vec<String>> {
    values.iter().map(|group| ids(group)).collect()
}

fn merged(values: &[(&[&str], &str)]) -> Vec<MergedGroup> {
    values
        .iter()
        .map(|(ids, result)| MergedGroup {
            ids: ids.iter().map(|id| id.to_string()).collect(),
            result: result.to_string(),
        })
        .collect()
}

fn index(entries: &[(&str, ProfileItem)]) -> BTreeMap<String, ProfileItem> {
    entries
        .iter()
        .map(|(id, item)| (id.to_string(), item.clone()))
        .collect()
}

#[test]
fn test_partial_apply_preserves_unselected_items_when_applying_a_scoped_cleanup() {
    let old_profile = base_profile(&[
        "keep-0", "keep-1", "scope-a", "scope-b", "keep-4", "keep-5", "keep-6", "keep-7", "keep-8",
        "keep-9",
    ]);
    let cleaned = base_profile(&["scope-a-merged"]);
    let all_merged = merged(&[(&["pref_2", "pref_3"], "scope-a-merged")]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &old_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_2", "pref_3"])),
        accepted_merged: &groups(&[&["pref_2", "pref_3"]]),
        accepted_removed: &[],
        all_merged: &all_merged,
        all_removed_ids: &[],
        explicit_acceptance: true,
    });

    let descriptions = descriptions(&result);
    assert!(descriptions.contains(&"keep-0"));
    assert!(descriptions.contains(&"keep-9"));
    assert!(descriptions.contains(&"scope-a-merged"));
    assert!(!descriptions.contains(&"scope-a"));
    assert!(!descriptions.contains(&"scope-b"));
    assert_eq!(
        descriptions
            .iter()
            .filter(|description| description.starts_with("keep-"))
            .count(),
        8,
        "everything outside the scope is preserved: {descriptions:?}"
    );
}

#[test]
fn test_rejected_removal_restores_the_item() {
    let old_profile = base_profile(&["keep", "remove-me"]);
    let cleaned = base_profile(&["keep"]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &old_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_0", "pref_1"])),
        accepted_merged: &[],
        accepted_removed: &[],
        all_merged: &[],
        all_removed_ids: &ids(&["pref_1"]),
        explicit_acceptance: true,
    });

    let descriptions = descriptions(&result);
    assert!(descriptions.contains(&"keep"));
    assert!(
        descriptions.contains(&"remove-me"),
        "a rejected removal restores the item: {descriptions:?}"
    );
}

#[test]
fn test_accepted_removal_removes_only_the_selected_item() {
    let old_profile = base_profile(&["keep", "remove-me", "outside"]);
    let cleaned = base_profile(&["keep"]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &old_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_0", "pref_1"])),
        accepted_merged: &[],
        accepted_removed: &ids(&["pref_1"]),
        all_merged: &[],
        all_removed_ids: &ids(&["pref_1"]),
        explicit_acceptance: true,
    });

    let descriptions = descriptions(&result);
    assert!(descriptions.contains(&"keep"));
    assert!(descriptions.contains(&"outside"));
    assert!(!descriptions.contains(&"remove-me"));
}

#[test]
fn test_rejected_merge_restores_the_merge_sources() {
    let old_profile = base_profile(&["target", "source", "outside"]);
    let cleaned = base_profile(&["target-merged"]);
    let all_merged = merged(&[(&["pref_0", "pref_1"], "target-merged")]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &old_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_0", "pref_1"])),
        accepted_merged: &[],
        accepted_removed: &[],
        all_merged: &all_merged,
        all_removed_ids: &[],
        explicit_acceptance: true,
    });

    let descriptions = descriptions(&result);
    assert!(descriptions.contains(&"outside"));
    assert!(descriptions.contains(&"source"));
    assert!(descriptions.contains(&"target"));
    assert!(!descriptions.contains(&"target-merged"));
}

#[test]
fn test_items_added_after_analysis_are_preserved() {
    let old_profile = base_profile(&["scope-a", "keep"]);
    let current_profile = base_profile(&["scope-a", "keep", "brand-new"]);
    let cleaned = base_profile(&["scope-a-clean"]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &current_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_0"])),
        accepted_merged: &[],
        accepted_removed: &[],
        all_merged: &[],
        all_removed_ids: &[],
        explicit_acceptance: false,
    });

    let descriptions = descriptions(&result);
    assert!(descriptions.contains(&"keep"));
    assert!(
        descriptions.contains(&"brand-new"),
        "an item added after the analysis must survive: {descriptions:?}"
    );
    assert!(descriptions.contains(&"scope-a-clean"));
    assert!(!descriptions.contains(&"scope-a"));
}

#[test]
fn test_unset_include_ids_replaces_the_whole_profile() {
    let old_profile = base_profile(&["a", "b", "c"]);
    let cleaned = base_profile(&["a-clean"]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &old_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: None,
        accepted_merged: &[],
        accepted_removed: &[],
        all_merged: &[],
        all_removed_ids: &ids(&["pref_1", "pref_2"]),
        explicit_acceptance: false,
    });

    assert_eq!(descriptions(&result), vec!["a-clean"]);
}

#[test]
fn test_duplicate_descriptions_remove_only_the_selected_occurrence() {
    let old_profile = base_profile(&["duplicate", "duplicate", "outside"]);
    let cleaned = base_profile(&["duplicate-cleaned"]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &old_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_0"])),
        accepted_merged: &[],
        accepted_removed: &[],
        all_merged: &[],
        all_removed_ids: &[],
        explicit_acceptance: false,
    });

    assert_eq!(
        descriptions(&result),
        vec!["duplicate", "outside", "duplicate-cleaned"],
        "the id's index selects the occurrence; duplicates are never cross-deleted"
    );
}

#[test]
fn test_rejected_merge_does_not_duplicate_items_whose_metadata_changed() {
    let mut old_profile = base_profile(&["duplicate", "duplicate", "outside"]);
    old_profile.preferences[1].centroid = Some(vec![0.1, 0.2]);
    let mut current_profile = old_profile.clone();
    current_profile.preferences[1].centroid = None;
    let cleaned = base_profile(&["duplicate"]);
    let all_merged = merged(&[(&["pref_0", "pref_1"], "duplicate")]);

    let result = merge_cleanup_into_profile(&CleanupApplyArgs {
        current_profile: &current_profile,
        old_profile_data: &old_profile,
        cleaned_data: &cleaned,
        include_ids: Some(&ids(&["pref_0", "pref_1"])),
        accepted_merged: &[],
        accepted_removed: &[],
        all_merged: &all_merged,
        all_removed_ids: &[],
        explicit_acceptance: true,
    });

    assert_eq!(result.preferences.len(), 3);
    assert_eq!(
        result
            .preferences
            .iter()
            .filter(|item| item.description == "duplicate")
            .count(),
        2,
        "the rejected merge restores both duplicates exactly once"
    );
    assert!(descriptions(&result).contains(&"outside"));
}

#[test]
fn test_rebuild_profile_using_does_not_preserve_merge_sources_as_unmentioned() {
    let mut target = pref("duplicate");
    target.frequency = Some(2);
    target.last_seen = Some(1);
    let mut source = pref("duplicate");
    source.frequency = Some(3);
    source.last_seen = Some(1);
    let mut cleaned_target = pref("duplicate");
    cleaned_target.frequency = Some(5);
    cleaned_target.last_seen = Some(1);

    let result = rebuild_profile_using(
        &CleanupMapping {
            kept: Vec::new(),
            merged: groups(&[&["pref_0", "pref_1"]]),
            removed: Vec::new(),
        },
        &index(&[("pref_0", cleaned_target)]),
        &index(&[("pref_0", target), ("pref_1", source)]),
    );

    assert_eq!(result.preferences.len(), 1);
    assert_eq!(result.preferences[0].description, "duplicate");
}

#[test]
fn test_merged_pair_confidence_is_the_ema_of_target_and_source_at_alpha_three_tenths() {
    let mut target = pref("target");
    target.confidence = Some(0.9);
    let mut source = pref("source");
    source.confidence = Some(0.5);

    let result = rebuild_profile_using(
        &CleanupMapping {
            kept: Vec::new(),
            merged: groups(&[&["pref_0", "pref_1"]]),
            removed: Vec::new(),
        },
        &BTreeMap::new(),
        &index(&[("pref_0", target), ("pref_1", source)]),
    );

    assert_eq!(result.preferences.len(), 1);
    let confidence = result.preferences[0]
        .confidence
        .expect("the folded confidence lands on the merged entry");
    let expected = 0.3 * 0.5 + 0.7 * 0.9;
    assert!(
        (confidence - expected).abs() < 1e-9,
        "expected the α = 0.3 fold (0.78), got {confidence}"
    );
}
