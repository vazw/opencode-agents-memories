//! The near-duplicate kernel's work bound — `engine::near_groups` under
//! `NearScanLimits`.
//!
//! Two properties are pinned here, both at the kernel seam (no store, no
//! service):
//!
//! - **the budget**: candidate comparisons are counted once per inner-loop
//!   candidate visited, `max_pairs` is tested *before* the comparison, so
//!   `pairs_scanned <= max_pairs` always; exhaustion sets `truncated` and keeps
//!   the group built so far;
//! - **cancellation**: a one-way flag ends the scan at its next check with
//!   `cancelled`, before any pair work when it is already set.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mem_web::api::deduplication::DedupMemory;
use mem_web::api::deduplication::engine::{NearScanLimits, near_groups};

const DIMENSIONS: usize = 768;

fn mem(id: &str, content: &str, tag: &str, created_at: i64, vector: Vec<f32>) -> DedupMemory {
    DedupMemory {
        id: id.to_string(),
        content: content.to_string(),
        container_tag: tag.to_string(),
        created_at,
        vector: Some(vector),
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

/// A unit vector in the `y`/`z` plane at `cos = c` from [`unit_y`].
fn with_cos_y(c: f64) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMENSIONS];
    vector[1] = c as f32;
    vector[2] = (1.0 - c * c).sqrt() as f32;
    vector
}

/// A never-set cancel flag.
fn never_cancelled() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// A budget with a fresh never-set flag.
fn limits(max_pairs: u64) -> NearScanLimits {
    NearScanLimits {
        max_pairs,
        cancel: never_cancelled(),
    }
}

/// Two independent in-band pairs: representatives at rows 0 and 2, claimed
/// candidates at rows 1 and 3. Four ordered comparisons in total.
fn two_pairs() -> Vec<DedupMemory> {
    vec![
        mem("mem_a", "content a", "tag_a", 4000, unit_x()),
        mem("mem_b", "content b", "tag_a", 3000, with_cos(0.95)),
        mem("mem_c", "content c", "tag_a", 2000, unit_y()),
        mem("mem_d", "content d", "tag_a", 1000, with_cos_y(0.95)),
    ]
}

#[test]
fn test_near_scan_stops_at_the_pair_budget_and_reports_the_truncation() {
    let rows = two_pairs();
    let survivors = [0, 1, 2, 3];

    let scan = near_groups(&rows, &survivors, 0.9, &limits(2));

    assert_eq!(scan.pairs_scanned, 2, "the budget is spent, not exceeded");
    assert!(scan.truncated, "exhaustion is reported: {scan:?}");
    assert!(!scan.cancelled, "no cancel flag was set");
    assert_eq!(
        scan.groups.len(),
        1,
        "the group built before the cut is kept (it holds a duplicate)"
    );
    assert_eq!(scan.groups[0].representative, 0);
    assert_eq!(scan.groups[0].duplicates.len(), 1);
    assert_eq!(scan.groups[0].duplicates[0].0, 1);

    // The bound is a ceiling, never a target that can be overshot.
    assert!(scan.pairs_scanned <= 2);
}

#[test]
fn test_near_scan_reports_every_group_when_the_pair_budget_is_not_reached() {
    let rows = two_pairs();
    let survivors = [0, 1, 2, 3];

    let scan = near_groups(&rows, &survivors, 0.9, &limits(u64::MAX));

    assert_eq!(scan.pairs_scanned, 4, "all four ordered comparisons ran");
    assert!(!scan.truncated, "the generous budget never truncates");
    assert!(!scan.cancelled);
    assert_eq!(scan.groups.len(), 2, "{:?}", scan.groups);
    assert_eq!(scan.groups[0].representative, 0);
    assert_eq!(scan.groups[0].duplicates.len(), 1);
    assert_eq!(scan.groups[0].duplicates[0].0, 1);
    assert_eq!(scan.groups[1].representative, 2);
    assert_eq!(scan.groups[1].duplicates.len(), 1);
    assert_eq!(scan.groups[1].duplicates[0].0, 3);
}

#[test]
fn test_near_scan_counts_candidate_comparisons_even_when_no_cosine_is_computed() {
    // Every row carries its own container tag, so every comparison is a tag
    // rejection and no cosine runs — yet each visited candidate is still work
    // the budget must account for.
    let rows = vec![
        mem("mem_a", "content a", "tag_a", 4000, unit_x()),
        mem("mem_b", "content b", "tag_b", 3000, with_cos(0.95)),
        mem("mem_c", "content c", "tag_c", 2000, unit_y()),
        mem("mem_d", "content d", "tag_d", 1000, with_cos_y(0.95)),
    ];
    let survivors = [0, 1, 2, 3];

    let scan = near_groups(&rows, &survivors, 0.9, &limits(u64::MAX));

    assert!(
        scan.groups.is_empty(),
        "no pair shares a container tag: {:?}",
        scan.groups
    );
    assert_eq!(
        scan.pairs_scanned, 6,
        "the whole ordered grid is counted, cosine or not"
    );
    assert!(!scan.truncated);

    // ... and the same fixture under a budget cuts at exactly that budget.
    let cut = near_groups(&rows, &survivors, 0.9, &limits(3));
    assert_eq!(cut.pairs_scanned, 3);
    assert!(cut.truncated);
    assert!(cut.groups.is_empty());
}

#[test]
fn test_near_scan_reports_no_groups_for_a_zero_budget() {
    let rows = two_pairs();
    let survivors = [0, 1, 2, 3];

    let scan = near_groups(&rows, &survivors, 0.9, &limits(0));

    assert_eq!(scan.pairs_scanned, 0, "a zero budget does no pair work");
    assert!(scan.truncated, "there was scan work left undone");
    assert!(!scan.cancelled);
    assert!(scan.groups.is_empty(), "{:?}", scan.groups);
}

#[test]
fn test_near_scan_scans_no_pairs_when_the_cancel_flag_is_already_set() {
    let rows = two_pairs();
    let survivors = [0, 1, 2, 3];
    let cancel = Arc::new(AtomicBool::new(true));

    let scan = near_groups(
        &rows,
        &survivors,
        0.9,
        &NearScanLimits {
            max_pairs: u64::MAX,
            cancel: Arc::clone(&cancel),
        },
    );

    assert!(
        scan.cancelled,
        "the flag ends the scan before any pair work"
    );
    assert_eq!(scan.pairs_scanned, 0);
    assert!(!scan.truncated, "a cancelled scan is not a truncated one");
    assert!(scan.groups.is_empty());
    assert!(
        cancel.load(std::sync::atomic::Ordering::Relaxed),
        "the scan never clears the one-way flag"
    );
}
