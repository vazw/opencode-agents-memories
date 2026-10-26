//! The pure deduplication kernel (`api::deduplication::engine`) — upstream
//! `deduplication-service.ts`'s grouping logic, with no DB in sight.
//!
//! Every fixture vector is 768 lanes (the store's configured width):
//! - [`unit_x`] along `x`, [`unit_y`] along `y` (orthogonal);
//! - [`with_cos`] at `cos = c` from `unit_x` in the `x`/`y` plane;
//! - [`rotated`] at an angle from `x` (the greediness test's directions);
//! - [`identical_vector`] whose cosine with itself is **exactly** `1.0`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mem_core::utils::math::cosine_similarity;
use mem_web::api::deduplication::DedupMemory;
use mem_web::api::deduplication::engine::{NearScanLimits, exact_plan, near_groups};

const DIMENSIONS: usize = 768;

/// A budget so generous it cannot bind, plus a fresh never-set cancel flag:
/// these cases pin the *grouping* behaviour, which the bound must not perturb.
fn limits() -> NearScanLimits {
    NearScanLimits {
        max_pairs: u64::MAX,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

fn mem(
    id: &str,
    content: &str,
    tag: &str,
    created_at: i64,
    vector: Option<Vec<f32>>,
) -> DedupMemory {
    DedupMemory {
        id: id.to_string(),
        content: content.to_string(),
        container_tag: tag.to_string(),
        created_at,
        vector,
    }
}

fn unit_x() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[0] = 1.0;
    vector
}

fn unit_y() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[1] = 1.0;
    vector
}

fn with_cos(c: f64) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[0] = c as f32;
    vector[1] = (1.0 - c * c).sqrt() as f32;
    vector
}

fn rotated(degrees: f64) -> Vec<f32> {
    let radians = degrees.to_radians();
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[0] = radians.cos() as f32;
    vector[1] = radians.sin() as f32;
    vector
}

/// A unit vector in the `y`/`z` plane at `cos = c` from [`unit_y`].
fn with_cos_y(c: f64) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[1] = c as f32;
    vector[2] = (1.0 - c * c).sqrt() as f32;
    vector
}

/// Four `3.0` lanes ⇒ `|v|^2 = 36 = 6^2`, so `cos(v, v)` is exactly `1.0`.
fn identical_vector() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    for lane in vector.iter_mut().take(4) {
        *lane = 3.0;
    }
    vector
}

#[test]
fn test_exact_plan_keeps_the_newest_of_three() {
    let rows = vec![
        mem("mem_a", "same content", "tag_a", 3000, Some(unit_x())),
        mem("mem_b", "same content", "tag_a", 2000, Some(unit_x())),
        mem("mem_c", "same content", "tag_a", 1000, Some(unit_x())),
    ];

    let plan = exact_plan(&rows);
    assert_eq!(plan.survivors, vec![0], "the newest row survives");
    assert_eq!(plan.deletions, vec![1, 2], "the older two are deleted");
}

#[test]
fn test_exact_plan_ties_break_on_the_smaller_id() {
    let mem_b = mem("mem_b", "same content", "tag_a", 1000, Some(unit_x()));
    let mem_a = mem("mem_a", "same content", "tag_a", 1000, Some(unit_x()));

    // Fed larger-id-first: an implementation that merely keeps the first row
    // of the input elects `mem_b` here and fails.
    let plan = exact_plan(&[mem_b.clone(), mem_a.clone()]);
    assert_eq!(plan.survivors, vec![1], "`mem_a` survives at index 1");
    assert_eq!(plan.deletions, vec![0]);

    // ... and the other way round, so the election cannot be position luck.
    let plan = exact_plan(&[mem_a, mem_b]);
    assert_eq!(plan.survivors, vec![0], "`mem_a` survives at index 0");
    assert_eq!(plan.deletions, vec![1]);
}

#[test]
fn test_near_groups_report_a_pair_in_the_band() {
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 2000, Some(unit_x())),
        mem("mem_b", "content b", "tag_a", 1000, Some(with_cos(0.95))),
    ];

    let groups = near_groups(&rows, &[0, 1], 0.9, &limits()).groups;
    assert_eq!(groups.len(), 1, "one group: {groups:?}");
    assert_eq!(groups[0].representative, 0);
    assert_eq!(groups[0].duplicates.len(), 1);
    let (index, similarity) = groups[0].duplicates[0];
    assert_eq!(index, 1);
    assert!(
        (0.94..0.96).contains(&similarity),
        "the reported similarity is in the configured band: {similarity}"
    );
}

#[test]
fn test_near_groups_exclude_a_similarity_of_exactly_one() {
    let vector = identical_vector();
    assert_eq!(
        cosine_similarity(&vector, &vector),
        1.0,
        "the fixture's cosine with itself is exactly 1.0 (four 3.0 lanes)"
    );

    let rows = vec![
        mem("mem_a", "content a", "tag_a", 2000, Some(vector.clone())),
        mem("mem_b", "content b", "tag_a", 1000, Some(vector)),
    ];

    assert!(
        near_groups(&rows, &exact_plan(&rows).survivors, 0.9, &limits())
            .groups
            .is_empty(),
        "similarity == 1.0 is excluded: the exact pass owns identical rows"
    );
}

#[test]
fn test_near_groups_skip_a_pair_below_the_threshold() {
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 2000, Some(unit_x())),
        mem("mem_b", "content b", "tag_a", 1000, Some(with_cos(0.5))),
    ];

    assert!(
        near_groups(&rows, &[0, 1], 0.9, &limits())
            .groups
            .is_empty()
    );
}

#[test]
fn test_near_groups_never_pair_different_container_tags() {
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 2000, Some(unit_x())),
        mem("mem_b", "content b", "tag_b", 1000, Some(with_cos(0.99))),
    ];

    assert!(
        near_groups(&rows, &[0, 1], 0.9, &limits())
            .groups
            .is_empty()
    );
}

#[test]
fn test_near_groups_are_greedy_not_transitive() {
    // 0°, 18°, 36° in the x/y plane: A~B and B~C are ~0.951, A~C is ~0.809.
    let a = rotated(0.0);
    let b = rotated(18.0);
    let c = rotated(36.0);
    assert!(
        cosine_similarity(&a, &c) < 0.9,
        "A and C are outside the band"
    );
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 3000, Some(a)),
        mem("mem_b", "content b", "tag_a", 2000, Some(b)),
        mem("mem_c", "content c", "tag_a", 1000, Some(c)),
    ];

    let groups = near_groups(&rows, &[0, 1, 2], 0.9, &limits()).groups;
    assert_eq!(groups.len(), 1, "exactly one group: {groups:?}");
    assert_eq!(groups[0].representative, 0);
    assert_eq!(
        groups[0].duplicates.len(),
        1,
        "a transitive implementation claims C too: {groups:?}"
    );
    assert_eq!(groups[0].duplicates[0].0, 1);
    assert!(
        (0.94..0.96).contains(&groups[0].duplicates[0].1),
        "B's similarity: {}",
        groups[0].duplicates[0].1
    );
}

#[test]
fn test_near_groups_skip_a_row_without_a_parsed_vector() {
    // As a candidate: the vector-less row is invisible, so no group.
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 2000, Some(unit_x())),
        mem("mem_b", "content b", "tag_a", 1000, None),
    ];
    assert!(
        near_groups(&rows, &[0, 1], 0.9, &limits())
            .groups
            .is_empty()
    );

    // As the first survivor: skipped, so the later row groups with nothing.
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 2000, None),
        mem("mem_b", "content b", "tag_a", 1000, Some(unit_x())),
    ];
    assert!(
        near_groups(&rows, &[0, 1], 0.9, &limits())
            .groups
            .is_empty()
    );
}

#[test]
fn test_near_groups_follow_the_scan_order_of_the_input() {
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 4000, Some(unit_x())),
        mem("mem_b", "content b", "tag_a", 3000, Some(unit_y())),
        mem("mem_c", "content c", "tag_a", 2000, Some(with_cos(0.95))),
        mem("mem_d", "content d", "tag_a", 1000, Some(with_cos_y(0.95))),
    ];

    let groups = near_groups(&rows, &[0, 1, 2, 3], 0.9, &limits()).groups;
    assert_eq!(groups.len(), 2, "two groups: {groups:?}");
    assert_eq!(groups[0].representative, 0);
    assert_eq!(groups[0].duplicates.len(), 1);
    assert_eq!(groups[0].duplicates[0].0, 2);
    assert_eq!(groups[1].representative, 1);
    assert_eq!(groups[1].duplicates.len(), 1);
    assert_eq!(
        groups[1].duplicates[0].0, 3,
        "a hash-ordered implementation flips the two groups"
    );
}
