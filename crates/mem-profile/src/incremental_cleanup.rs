//! `incremental_cleanup` — the scoped cleanup trigger (W3.3, R6).
//!
//! Behavioural origin: upstream `user-memory-learning.ts` L15-60
//! (`shouldRunAutomaticProfileCleanup` + `runAutomaticProfileCleanup`), with
//! the issue's "incremental, never a full scan" requirement applied to the
//! *scope*: the traits that landed in this round plus their conflict-band
//! neighbours from [`mem_core::dedup`]. The application loop itself is
//! upstream's scoped one ([`crate::cleanup_apply`]), so only *which ids* reach
//! it changes — never how they are applied.
//!
//! Nothing in this wave constructs this hook in production: per D-W3 there is
//! no timer, thread, queue or composition-root wiring here (R13). The host
//! lifecycle in `mem-bridge`/the composition root owns that.

use std::collections::BTreeSet;
use std::sync::Arc;

use mem_core::dedup::{DedupCandidate, DedupOptions, dedup_groups};
use mem_store::db::BoxFuture;

use crate::ai_cleanup::{ProfileCleanupGateway, ai_cleanup_profile_from_indexed, index_profile};
use crate::cleanup_apply::{CleanupApplyArgs, merge_cleanup_into_profile};
use crate::error::ProfileError;
use crate::profile_store::{ProfileConfig, ProfileData, ProfileStore, ProfileUpdates};
use crate::validation::should_run_automatic_profile_cleanup;

/// What one incremental cleanup did (upstream logs the same three numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupOutcome {
    pub merged: usize,
    pub removed: usize,
    /// `false` when nothing was written (no diff, or a lost optimistic-version
    /// race — the race is never retried inline).
    pub updated: bool,
}

impl CleanupOutcome {
    fn zero() -> Self {
        Self {
            merged: 0,
            removed: 0,
            updated: false,
        }
    }
}

/// The incremental cleanup seam the learning round drives.
pub trait IncrementalCleanup: Send + Sync {
    fn run_incremental<'a>(
        &'a self,
        user_id: &'a str,
        landed: &'a ProfileUpdates,
        previous_prompt_count: i64,
        added_prompt_count: i64,
        now: i64,
    ) -> BoxFuture<'a, Result<CleanupOutcome, ProfileError>>;
}

/// The cleanup scope: the landed traits plus their dedup neighbours.
///
/// Deterministic and category-scoped (upstream `detectConflicts`' same-category
/// guard, generalized to all three sections): only the sections+categories the
/// round actually touched are scanned, so this is never a full-profile scan.
pub fn cleanup_scope(data: &ProfileData, landed: &ProfileUpdates) -> Vec<String> {
    let indexed = index_profile(data);

    // (section, category-or-empty, description) of everything that landed.
    let mut landed_keys: Vec<(&'static str, String, String)> = Vec::new();
    let sections: [(
        &'static str,
        Option<&Vec<crate::profile_store::ProfileItem>>,
    ); 3] = [
        ("preferences", landed.preferences.as_ref()),
        ("patterns", landed.patterns.as_ref()),
        ("workflows", landed.workflows.as_ref()),
    ];
    for (section, items) in sections {
        let Some(items) = items else {
            continue;
        };
        for item in items {
            landed_keys.push((
                section,
                item.category.clone().unwrap_or_default(),
                item.description.clone(),
            ));
        }
    }

    // The indexed ids whose (section, category, description) landed.
    let mut landed_ids: Vec<String> = Vec::new();
    for (section, entries) in [
        ("preferences", &indexed.preferences),
        ("patterns", &indexed.patterns),
        ("workflows", &indexed.workflows),
    ] {
        for entry in entries {
            let category = entry.item.category.clone().unwrap_or_default();
            if landed_keys
                .iter()
                .any(|(landed_section, landed_category, description)| {
                    *landed_section == section
                        && *landed_category == category
                        && *description == entry.item.description
                })
            {
                landed_ids.push(entry.id.clone());
            }
        }
    }
    if landed_ids.is_empty() {
        return Vec::new();
    }

    // Candidates: every item whose (section, category) pair the round touched —
    // never another category, never another section, never the full profile.
    let mut scoped_pairs: Vec<(&str, String)> = Vec::new();
    for (section, category, _) in &landed_keys {
        if !scoped_pairs
            .iter()
            .any(|(scoped_section, scoped_category)| {
                scoped_section == section && scoped_category == category
            })
        {
            scoped_pairs.push((section, category.clone()));
        }
    }

    struct OwnedCandidate {
        id: String,
        key: String,
        embedding: Option<Vec<f32>>,
    }

    let mut candidates: Vec<OwnedCandidate> = Vec::new();
    for (section, entries) in [
        ("preferences", &indexed.preferences),
        ("patterns", &indexed.patterns),
        ("workflows", &indexed.workflows),
    ] {
        for entry in entries {
            let category = entry.item.category.clone().unwrap_or_default();
            if !scoped_pairs
                .iter()
                .any(|(scoped_section, scoped_category)| {
                    *scoped_section == section && *scoped_category == category
                })
            {
                continue;
            }
            candidates.push(OwnedCandidate {
                id: entry.id.clone(),
                key: format!("{section}\u{1f}{category}\u{1f}{}", entry.item.description),
                embedding: entry.item.centroid.clone(),
            });
        }
    }

    let borrowed: Vec<DedupCandidate<'_>> = candidates
        .iter()
        .map(|candidate| DedupCandidate {
            id: &candidate.id,
            key: &candidate.key,
            embedding: candidate.embedding.as_deref(),
        })
        .collect();
    let groups = dedup_groups(&borrowed, &DedupOptions::default());

    let landed_set: BTreeSet<&str> = landed_ids.iter().map(String::as_str).collect();
    let mut scope: BTreeSet<String> = landed_ids.iter().cloned().collect();
    for group in &groups {
        if group.ids.iter().any(|id| landed_set.contains(id.as_str())) {
            scope.extend(group.ids.iter().cloned());
        }
    }

    // First-appearance order over the indexed profile (deterministic).
    let mut ordered: Vec<String> = Vec::new();
    for (_, entries) in [
        ("preferences", &indexed.preferences),
        ("patterns", &indexed.patterns),
        ("workflows", &indexed.workflows),
    ] {
        for entry in entries {
            if scope.contains(&entry.id) && !ordered.contains(&entry.id) {
                ordered.push(entry.id.clone());
            }
        }
    }
    ordered
}

/// The configured cleanup hook: the round calls [`IncrementalCleanup::run_incremental`]
/// only when [`ProfileCleanupHook::should_run`] agrees.
pub struct ProfileCleanupHook {
    store: Arc<ProfileStore>,
    gateway: Arc<ProfileCleanupGateway>,
    config: ProfileConfig,
}

impl ProfileCleanupHook {
    pub fn new(
        store: Arc<ProfileStore>,
        gateway: Arc<ProfileCleanupGateway>,
        config: ProfileConfig,
    ) -> Self {
        Self {
            store,
            gateway,
            config,
        }
    }

    /// Upstream `shouldRunAutomaticProfileCleanup` — one definition of the gate
    /// (delegated, never copied).
    pub fn should_run(&self, previous_prompt_count: i64, added_prompt_count: i64) -> bool {
        should_run_automatic_profile_cleanup(
            previous_prompt_count,
            added_prompt_count,
            self.config.auto_cleanup_interval,
            self.config.auto_cleanup_enabled,
        )
    }
}

impl IncrementalCleanup for ProfileCleanupHook {
    fn run_incremental<'a>(
        &'a self,
        user_id: &'a str,
        landed: &'a ProfileUpdates,
        _previous_prompt_count: i64,
        _added_prompt_count: i64,
        _now: i64,
    ) -> BoxFuture<'a, Result<CleanupOutcome, ProfileError>> {
        Box::pin(async move {
            let Some(profile) = self.store.get_active_profile(user_id).await? else {
                return Ok(CleanupOutcome::zero());
            };
            let data: ProfileData = serde_json::from_str(&profile.profile_data)?;

            let item_count = data.preferences.len() + data.patterns.len() + data.workflows.len();
            if item_count < 2 {
                return Ok(CleanupOutcome::zero());
            }

            let include_ids = cleanup_scope(&data, landed);
            if include_ids.len() < 2 {
                return Ok(CleanupOutcome::zero());
            }

            let indexed = crate::ai_cleanup::filter_profile_for_cleanup(&data, &include_ids);
            let cleanup = ai_cleanup_profile_from_indexed(&indexed, &self.gateway).await?;
            if cleanup.diff.merged.is_empty() && cleanup.diff.removed.is_empty() {
                return Ok(CleanupOutcome::zero());
            }

            // The automatic path auto-accepts everything inside the scope.
            let accepted_merged: Vec<Vec<String>> = cleanup
                .diff
                .merged
                .iter()
                .map(|group| group.ids.clone())
                .collect();
            let accepted_removed: Vec<String> = cleanup
                .diff
                .removed
                .iter()
                .map(|removed| removed.id.clone())
                .collect();

            let result = merge_cleanup_into_profile(&CleanupApplyArgs {
                current_profile: &data,
                old_profile_data: &data,
                cleaned_data: &cleanup.cleaned,
                include_ids: Some(&include_ids),
                accepted_merged: &accepted_merged,
                accepted_removed: &accepted_removed,
                all_merged: &cleanup.diff.merged,
                all_removed_ids: &accepted_removed,
                explicit_acceptance: true,
            });

            let summary = format!(
                "Automatic AI cleanup: {} merged, {} removed",
                cleanup.diff.merged.len(),
                cleanup.diff.removed.len()
            );
            let updated = self
                .store
                .update_profile(&profile.id, &result, 0, &summary)
                .await?;
            if !updated {
                // A lost optimistic-version race: report the counts, but never
                // retry inline and never force a write.
                tracing::debug!(
                    profile_id = %profile.id,
                    "incremental cleanup: lost the optimistic-version race"
                );
            }

            Ok(CleanupOutcome {
                merged: cleanup.diff.merged.len(),
                removed: cleanup.diff.removed.len(),
                updated,
            })
        })
    }
}
