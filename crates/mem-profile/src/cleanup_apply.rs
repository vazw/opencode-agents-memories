//! `cleanup_apply` — port of upstream `src/services/api-handlers.ts`
//! L1188-1269 (`mergeCleanupIntoProfile`) and its four helpers (`L1271+`).
//!
//! The apply loop is where "no data loss outside the merged pair" (R7) is
//! actually guaranteed: only the ids in scope can change disposition, every
//! item outside `include_ids` is carried over from `current_profile` verbatim,
//! and a rejected merge/removal is restored from `old_profile_data`.
//!
//! Everything here is pure and total: unknown ids, duplicate descriptions, ids
//! without `_`, and ids whose prefix has no section are all no-ops — never a
//! panic.

use serde_json::json;

use crate::ai_cleanup::MergedGroup;
use crate::profile_store::{ProfileData, ProfileItem};
use crate::validation::TraitType;

/// The arguments upstream's `mergeCleanupIntoProfile` destructures.
pub struct CleanupApplyArgs<'a> {
    /// The profile as it is now (may contain items added after the analysis).
    pub current_profile: &'a ProfileData,
    /// The snapshot the analysis ran against — the restoration source.
    pub old_profile_data: &'a ProfileData,
    /// The scoped, cleaned result.
    pub cleaned_data: &'a ProfileData,
    /// `None`/empty means "the cleaned data replaces the whole profile".
    pub include_ids: Option<&'a [String]>,
    pub accepted_merged: &'a [Vec<String>],
    pub accepted_removed: &'a [String],
    pub all_merged: &'a [MergedGroup],
    pub all_removed_ids: &'a [String],
    /// `true` when the client sent acceptance arrays (even empty = reject all).
    pub explicit_acceptance: bool,
}

/// Upstream `mergeCleanupIntoProfile`.
pub fn merge_cleanup_into_profile(args: &CleanupApplyArgs<'_>) -> ProfileData {
    // Start from the cleaned data, then restore any rejected removal/merge.
    let mut scoped = ProfileData {
        preferences: args.cleaned_data.preferences.clone(),
        patterns: args.cleaned_data.patterns.clone(),
        workflows: args.cleaned_data.workflows.clone(),
        learning_paths: args.cleaned_data.learning_paths.clone(),
    };

    if args.explicit_acceptance {
        for id in args.accepted_removed {
            remove_item_from_profile(&mut scoped, args.old_profile_data, id);
        }

        for ids in args.accepted_merged {
            for id in ids.iter().skip(1) {
                remove_item_from_profile(&mut scoped, args.old_profile_data, id);
            }
        }

        let accepted_targets: Vec<&str> = args
            .accepted_merged
            .iter()
            .filter_map(|group| group.first().map(String::as_str))
            .collect();
        for merge in args.all_merged {
            let group_ids = &merge.ids;
            if group_ids.len() <= 1 {
                continue;
            }
            if accepted_targets
                .iter()
                .any(|target| *target == group_ids[0])
            {
                continue;
            }
            // A rejected merge: undo the cleaned leader by description, then
            // re-push every group member from the snapshot.
            remove_one_by_description(&mut scoped, &merge.result, item_type_from_id(&group_ids[0]));
            for id in group_ids {
                if !id.is_empty() {
                    push_item_from_profile(&mut scoped, args.old_profile_data, id);
                }
            }
        }

        for removed_id in args.all_removed_ids {
            if args
                .accepted_removed
                .iter()
                .any(|accepted| accepted == removed_id)
            {
                continue;
            }
            push_item_from_profile(&mut scoped, args.old_profile_data, removed_id);
        }
    }

    // Full-profile cleanup: the cleaned data (plus acceptance) is the result.
    let Some(include_ids) = args.include_ids else {
        return scoped;
    };
    if include_ids.is_empty() {
        return scoped;
    }

    // Partial selection: mutate only the analyzed scope inside the current
    // profile, so anything added (or changed) after the analysis survives.
    let mut result = ProfileData {
        preferences: args.current_profile.preferences.clone(),
        patterns: args.current_profile.patterns.clone(),
        workflows: args.current_profile.workflows.clone(),
        learning_paths: args.current_profile.learning_paths.clone(),
    };

    for id in include_ids {
        remove_item_from_profile(&mut result, args.old_profile_data, id);
    }

    result.preferences.extend(scoped.preferences);
    result.patterns.extend(scoped.patterns);
    result.workflows.extend(scoped.workflows);

    result
}

/// Upstream `itemTypeFromId`: the section key the id's prefix names.
fn item_type_from_id(id: &str) -> &'static str {
    if id.starts_with("pref_") {
        "preferences"
    } else if id.starts_with("pat_") {
        "patterns"
    } else {
        "workflows"
    }
}

/// Upstream `findItemById`: an id only counts when it contains `_`, and the
/// number after the prefix must parse.
fn find_item_by_id<'a>(profile: &'a ProfileData, id: &str) -> Option<&'a ProfileItem> {
    if !id.contains('_') {
        return None;
    }
    let mut parts = id.split('_');
    let prefix = parts.next().unwrap_or_default();
    let index: usize = parts.next().and_then(|part| part.parse().ok())?;
    match prefix {
        "pref" => profile.preferences.get(index),
        "pat" => profile.patterns.get(index),
        "wf" => profile.workflows.get(index),
        _ => None,
    }
}

/// Upstream `profileItemIdentityKey` — what makes one occurrence of an item
/// distinguishable from a same-description sibling.
fn identity_key(item: &ProfileItem) -> String {
    serde_json::to_string(&json!({
        "category": &item.category,
        "description": item.description,
        "steps": &item.steps,
    }))
    .unwrap_or_default()
}

/// Upstream `removeItemFromProfile`: remove the target item whose identity key
/// matches the snapshot item the id points at.
fn remove_item_from_profile(target: &mut ProfileData, source: &ProfileData, id: &str) -> bool {
    let Some(source_item) = find_item_by_id(source, id) else {
        return false;
    };
    let source_key = identity_key(source_item);
    let items = match item_type_from_id(id) {
        "preferences" => &mut target.preferences,
        "patterns" => &mut target.patterns,
        _ => &mut target.workflows,
    };
    let Some(index) = items
        .iter()
        .position(|item| identity_key(item) == source_key)
    else {
        return false;
    };
    items.remove(index);
    true
}

/// Upstream `removeOneByDescription`: removes the FIRST matching description in
/// the section.
fn remove_one_by_description(
    profile: &mut ProfileData,
    description: &str,
    item_type: &str,
) -> bool {
    let items = match item_type {
        "preferences" => &mut profile.preferences,
        "patterns" => &mut profile.patterns,
        _ => &mut profile.workflows,
    };
    let Some(index) = items
        .iter()
        .position(|item| item.description == description)
    else {
        return false;
    };
    items.remove(index);
    true
}

/// Upstream `pushItemFromProfile`: re-pushes the snapshot item the id points
/// at, without its synthetic `id`.
fn push_item_from_profile(target: &mut ProfileData, source: &ProfileData, id: &str) {
    let Some(source_item) = find_item_by_id(source, id) else {
        return;
    };
    let mut item = source_item.clone();
    // The synthetic id lives in `ProfileItem::extra` when it came from a model
    // response; it is never part of a stored profile item.
    item.extra.remove("id");
    match TraitType::from_id_prefix(id) {
        Some(TraitType::Preference) => target.preferences.push(item),
        Some(TraitType::Pattern) => target.patterns.push(item),
        Some(TraitType::Workflow) => target.workflows.push(item),
        // Upstream has no `else`: an id with no known prefix is dropped.
        None => {}
    }
}
