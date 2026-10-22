//! Shared dedup grouping service (W3.3, reused by W4.3).
//!
//! Behavioural origin: upstream `user-profile-manager.ts::detectConflicts`
//! (the same-category `cos >= 0.65 && cos < 0.9` conflict band) plus the exact
//! `(category, description)` duplicate rule already used by the merge path.
//!
//! Pure and deterministic — no DB, no provider, no clock, no `HashMap`
//! iteration order anywhere in the output. Two consumers:
//!
//! - W3.3's incremental cleanup scope (`mem_profile::incremental_cleanup`) —
//!   the landed trait plus its conflict-band neighbours;
//! - (planned) W4.3's episodic consolidation, which supplies `id`/`key`/
//!   `embedding` the same way.

use crate::utils::math::cosine_similarity;

/// The ONE dedup-key normalization in the workspace (W7.1): trim, collapse
/// whitespace runs to a single space, then lowercase.
///
/// `mem-episodic`'s promotion selection, `mem-facts`' key derivation and the
/// bridge's cross-tier task all call this function — a second copy would let
/// the three tiers disagree about whether two facts are the same one.
///
/// The unit separator `\u{1f}` is NOT whitespace (`char::is_whitespace`), so
/// the exact-match key separator survives untouched.
pub fn normalize_key(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// One grouping candidate.
///
/// `key` is the exact-match identity (upstream's
/// `category + "\u{1f}" + description`); `embedding` is the similarity
/// vector used for the conflict band (absent ⇒ the candidate can only ever be
/// part of an [`DedupKind::Exact`] group).
#[derive(Debug, Clone, Copy)]
pub struct DedupCandidate<'a> {
    pub id: &'a str,
    pub key: &'a str,
    pub embedding: Option<&'a [f32]>,
}

/// Band configuration for [`dedup_groups`].
///
/// The defaults are upstream `detectConflicts`' band: a pair is a *conflict*
/// when its cosine similarity is inside the half-open `[0.65, 0.9)` interval —
/// at or above `0.9` the pair is already an exact/strong match, below `0.65`
/// they are unrelated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DedupOptions {
    pub conflict_min: f64,
    pub near_max: f64,
}

impl Default for DedupOptions {
    fn default() -> Self {
        Self {
            conflict_min: 0.65,
            near_max: 0.9,
        }
    }
}

/// How a group was formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupKind {
    /// Every member shares one `key` (upstream's exact duplicate rule).
    Exact,
    /// Every member is transitively linked by band-conflicting pairs.
    Conflict,
}

/// One group of candidates that must be treated together.
#[derive(Debug, Clone, PartialEq)]
pub struct DedupGroup {
    /// Member ids in first-appearance (candidate index) order.
    pub ids: Vec<String>,
    pub kind: DedupKind,
    /// `1.0` for [`DedupKind::Exact`]; the maximum pairwise cosine similarity
    /// for [`DedupKind::Conflict`].
    pub score: f64,
}

/// Groups `candidates` deterministically.
///
/// - **Exact** — all candidates sharing a `key` form one group in
///   first-appearance order (emitted only when it has ≥ 2 members).
/// - **Conflict** — every unordered pair with two embeddings whose cosine
///   similarity lies in `[conflict_min, near_max)` joins a union-find
///   component. Candidates already inside an exact group of size ≥ 2 are
///   skipped, and a candidate without an embedding never joins one.
/// - **Order** — exact groups first (first-appearance key order), then
///   conflict groups by descending score, ties broken by the group's smallest
///   candidate index.
///
/// The inputs are never mutated.
pub fn dedup_groups(candidates: &[DedupCandidate<'_>], options: &DedupOptions) -> Vec<DedupGroup> {
    let count = candidates.len();

    // --- Exact groups, in first-appearance key order (never a map walk). ---
    let mut key_groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        match key_groups.iter_mut().find(|(key, _)| *key == candidate.key) {
            Some((_, members)) => members.push(index),
            None => key_groups.push((candidate.key, vec![index])),
        }
    }

    let mut exact_groups: Vec<DedupGroup> = Vec::new();
    let mut in_large_exact_group = vec![false; count];
    for (_, members) in &key_groups {
        if members.len() < 2 {
            continue;
        }
        for &index in members {
            in_large_exact_group[index] = true;
        }
        exact_groups.push(DedupGroup {
            ids: members
                .iter()
                .map(|&index| candidates[index].id.to_string())
                .collect(),
            kind: DedupKind::Exact,
            score: 1.0,
        });
    }

    // --- Conflict components over the eligible candidates. ---
    let eligible =
        |index: usize| candidates[index].embedding.is_some() && !in_large_exact_group[index];

    let mut parent: Vec<usize> = (0..count).collect();
    for left in 0..count {
        if !eligible(left) {
            continue;
        }
        for right in (left + 1)..count {
            if !eligible(right) {
                continue;
            }
            let Some(left_embedding) = candidates[left].embedding else {
                continue;
            };
            let Some(right_embedding) = candidates[right].embedding else {
                continue;
            };
            let cosine = cosine_similarity(left_embedding, right_embedding);
            if cosine >= options.conflict_min && cosine < options.near_max {
                union(&mut parent, left, right);
            }
        }
    }

    // Components in first-appearance order, members ascending.
    let mut components: Vec<(usize, Vec<usize>)> = Vec::new();
    for index in 0..count {
        if !eligible(index) {
            continue;
        }
        let root = find_root(&mut parent, index);
        match components
            .iter_mut()
            .find(|(candidate_root, _)| *candidate_root == root)
        {
            Some((_, members)) => members.push(index),
            None => components.push((root, vec![index])),
        }
    }

    let mut conflict_groups: Vec<(usize, DedupGroup)> = Vec::new();
    for (_, members) in &components {
        if members.len() < 2 {
            continue;
        }
        let mut score = f64::NEG_INFINITY;
        for (position, &left) in members.iter().enumerate() {
            for &right in &members[position + 1..] {
                let Some(left_embedding) = candidates[left].embedding else {
                    continue;
                };
                let Some(right_embedding) = candidates[right].embedding else {
                    continue;
                };
                let cosine = cosine_similarity(left_embedding, right_embedding);
                if cosine > score {
                    score = cosine;
                }
            }
        }
        conflict_groups.push((
            members[0],
            DedupGroup {
                ids: members
                    .iter()
                    .map(|&index| candidates[index].id.to_string())
                    .collect(),
                kind: DedupKind::Conflict,
                score,
            },
        ));
    }

    // Descending score, ties broken by the smallest candidate index.
    conflict_groups.sort_by(|(left_index, left), (right_index, right)| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left_index.cmp(right_index))
    });

    exact_groups.extend(conflict_groups.into_iter().map(|(_, group)| group));
    exact_groups
}

/// Union-find root, with path halving (no recursion, bounded work).
fn find_root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

/// Joins two components, always linking onto the smaller index so the
/// component identity never depends on pair enumeration quirks.
fn union(parent: &mut [usize], left: usize, right: usize) {
    let left_root = find_root(parent, left);
    let right_root = find_root(parent, right);
    if left_root == right_root {
        return;
    }
    let (low, high) = if left_root < right_root {
        (left_root, right_root)
    } else {
        (right_root, left_root)
    };
    parent[high] = low;
}
