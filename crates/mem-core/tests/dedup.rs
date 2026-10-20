//! New coverage — no upstream test file exists for the dedup grouping service.
//!
//! Behavioural origin: upstream `user-profile-manager.ts::detectConflicts`
//! (the `cos >= 0.65 && cos < 0.9` conflict band, same-category restricted) and
//! the exact `(category, description)` duplicate rule. W3.3 lands it as a pure,
//! deterministic, DB-free function so the cleanup scope (W3.3) and episodic
//! consolidation (W4.3) share one grouping kernel.
//!
//! Tests:
//! - `test_exact_groups_collapse_same_key_into_one_group`
//! - `test_exact_singletons_are_not_grouped`
//! - `test_conflict_group_forms_from_pairs_in_the_band`
//! - `test_pair_at_or_above_the_near_max_is_not_a_conflict`
//! - `test_candidate_without_embedding_never_joins_a_conflict_group`
//! - `test_conflict_groups_are_ordered_by_score_descending_and_are_deterministic`

use mem_core::dedup::{DedupCandidate, DedupGroup, DedupKind, DedupOptions, dedup_groups};

fn candidate<'a>(id: &'a str, key: &'a str, embedding: Option<&'a [f32]>) -> DedupCandidate<'a> {
    DedupCandidate { id, key, embedding }
}

/// Unit vector at `degrees` (counter-clockwise from the +x axis). Two such
/// vectors have a cosine similarity of exactly `cos(Δ)` up to f32 rounding, so
/// the band boundaries can be driven from `acos` without magic literals.
fn unit(degrees: f64) -> Vec<f32> {
    let radians = degrees.to_radians();
    vec![radians.cos() as f32, radians.sin() as f32]
}

fn units(degrees: &[f64]) -> Vec<Vec<f32>> {
    degrees.iter().map(|&degree| unit(degree)).collect()
}

fn group_ids(group: &DedupGroup) -> Vec<&str> {
    group.ids.iter().map(String::as_str).collect()
}

#[test]
fn test_exact_groups_collapse_same_key_into_one_group() {
    let candidates = [
        candidate("a", "shared", None),
        candidate("b", "other", None),
        candidate("c", "shared", None),
        candidate("d", "shared", None),
    ];

    let groups = dedup_groups(&candidates, &DedupOptions::default());

    assert_eq!(groups.len(), 1, "only the repeated key forms a group");
    assert_eq!(groups[0].kind, DedupKind::Exact);
    assert_eq!(
        group_ids(&groups[0]),
        vec!["a", "c", "d"],
        "exact group members keep first-appearance order"
    );
    assert_eq!(groups[0].score, 1.0);
}

#[test]
fn test_exact_singletons_are_not_grouped() {
    let candidates = [
        candidate("a", "k0", None),
        candidate("b", "k1", None),
        candidate("c", "k2", None),
    ];

    let groups = dedup_groups(&candidates, &DedupOptions::default());

    assert!(
        groups.is_empty(),
        "a key seen once is not a duplicate: {groups:?}"
    );
}

#[test]
fn test_conflict_group_forms_from_pairs_in_the_band() {
    // 0°, 36.87° (cos 0.8), 73.74° (cos 0.8 with its predecessor, 0.28 with the
    // first item): only the consecutive pairs are inside [0.65, 0.9), yet the
    // component is transitive.
    let vectors = units(&[
        0.0,
        0.8f64.acos().to_degrees(),
        2.0 * 0.8f64.acos().to_degrees(),
    ]);
    let candidates = [
        candidate("a", "k0", Some(&vectors[0])),
        candidate("b", "k1", Some(&vectors[1])),
        candidate("c", "k2", Some(&vectors[2])),
    ];

    let groups = dedup_groups(&candidates, &DedupOptions::default());

    assert_eq!(groups.len(), 1, "one transitive component: {groups:?}");
    assert_eq!(groups[0].kind, DedupKind::Conflict);
    assert_eq!(group_ids(&groups[0]), vec!["a", "b", "c"]);
    assert!(
        (groups[0].score - 0.8).abs() < 1e-6,
        "the group score is the maximum pairwise cosine, got {}",
        groups[0].score
    );
}

#[test]
fn test_pair_at_or_above_the_near_max_is_not_a_conflict() {
    // Exactly-at-the-ceiling probe: two identical unit vectors have a cosine
    // of exactly 1.0 in f32 arithmetic, so `near_max = 1.0` is hit exactly and
    // the half-open band `[min, max)` must still exclude the pair.
    let identical = [[1.0f32, 0.0], [1.0f32, 0.0]];
    let identical_candidates = [
        candidate("a", "k0", Some(&identical[0])),
        candidate("b", "k1", Some(&identical[1])),
    ];
    assert!(
        dedup_groups(
            &identical_candidates,
            &DedupOptions {
                conflict_min: 0.65,
                near_max: 1.0,
            },
        )
        .is_empty(),
        "cos == near_max is excluded (the band is half-open)"
    );
    assert!(
        dedup_groups(&identical_candidates, &DedupOptions::default()).is_empty(),
        "cos == 1.0 is far above the near_max ceiling and never a conflict"
    );

    // A pair at ≈ 0.8: excluded as soon as the ceiling sits at or below it,
    // included while the ceiling is above it.
    let a = [1.0f32, 0.0];
    let b = [0.8f32, 0.6];
    let candidates = [
        candidate("a", "k0", Some(&a)),
        candidate("b", "k1", Some(&b)),
    ];

    let above_the_ceiling = dedup_groups(
        &candidates,
        &DedupOptions {
            conflict_min: 0.65,
            near_max: 0.75,
        },
    );
    assert!(
        above_the_ceiling.is_empty(),
        "a pair at or above near_max is not a conflict: {above_the_ceiling:?}"
    );

    let below_the_ceiling = dedup_groups(
        &candidates,
        &DedupOptions {
            conflict_min: 0.65,
            near_max: 0.9,
        },
    );
    assert_eq!(
        below_the_ceiling.len(),
        1,
        "the same pair is a conflict once near_max is above it"
    );
}

#[test]
fn test_candidate_without_embedding_never_joins_a_conflict_group() {
    let vectors = units(&[0.0, 0.8f64.acos().to_degrees()]);
    let candidates = [
        candidate("a", "k0", Some(&vectors[0])),
        candidate("b", "k1", Some(&vectors[1])),
        candidate("c", "k2", None),
        candidate("d", "k3", None),
    ];

    let groups = dedup_groups(&candidates, &DedupOptions::default());

    assert_eq!(
        groups.len(),
        1,
        "only the embedded pair conflicts: {groups:?}"
    );
    assert_eq!(
        group_ids(&groups[0]),
        vec!["a", "b"],
        "embedding-less candidates can never join a conflict group"
    );
}

#[test]
fn test_conflict_groups_are_ordered_by_score_descending_and_are_deterministic() {
    // Three disjoint components with distinct maximum pairwise cosines
    // (0.8, 0.75, 0.7); the angular spread keeps every cross-component pair
    // outside the [0.65, 0.9) band.
    let vectors = units(&[
        0.0,
        0.8f64.acos().to_degrees(),
        90.0,
        90.0 + 0.75f64.acos().to_degrees(),
        200.0,
        200.0 + 0.7f64.acos().to_degrees(),
    ]);
    let candidates = [
        candidate("a0", "k0", Some(&vectors[0])),
        candidate("a1", "k1", Some(&vectors[1])),
        candidate("b0", "k2", Some(&vectors[2])),
        candidate("b1", "k3", Some(&vectors[3])),
        candidate("c0", "k4", Some(&vectors[4])),
        candidate("c1", "k5", Some(&vectors[5])),
    ];

    let first = dedup_groups(&candidates, &DedupOptions::default());
    let second = dedup_groups(&candidates, &DedupOptions::default());

    assert_eq!(first.len(), 3, "three disjoint components: {first:?}");
    assert_eq!(group_ids(&first[0]), vec!["a0", "a1"]);
    assert_eq!(group_ids(&first[1]), vec!["b0", "b1"]);
    assert_eq!(group_ids(&first[2]), vec!["c0", "c1"]);
    assert!(
        first[0].score >= first[1].score && first[1].score >= first[2].score,
        "groups are ordered by score descending: {:?}",
        first.iter().map(|group| group.score).collect::<Vec<_>>()
    );

    let fingerprint = |groups: &[DedupGroup]| {
        groups
            .iter()
            .map(|group| (group.ids.clone(), group.kind, group.score))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fingerprint(&first),
        fingerprint(&second),
        "grouping is deterministic across runs (no HashMap iteration order)"
    );
}
