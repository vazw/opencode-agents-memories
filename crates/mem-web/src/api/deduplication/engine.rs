//! The pure deduplication passes — upstream `detectAndRemoveDuplicates`'
//! grouping logic, lifted out of the store so it is testable without a DB.
//!
//! Both functions are pure, synchronous and deterministic: the output order
//! follows the input order (never hash order), so the report a caller sees is
//! the same on every run over the same rows.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mem_core::utils::math::cosine_similarity;

use super::DedupMemory;

/// What the exact pass decided, as indices into the input slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactPlan {
    /// One survivor per distinct `(container_tag, content)` group, in group
    /// first-appearance order.
    pub survivors: Vec<usize>,
    /// Rows to delete, group first-appearance order then position 1.. in
    /// elected order.
    pub deletions: Vec<usize>,
}

/// The exact pass: group by the byte-for-byte pair `(container_tag, content)`,
/// keep the newest member of each group, and plan the rest for deletion.
///
/// Ties on `created_at` break on the **smaller id**, so the election never
/// depends on the caller's row order. No normalization is applied — upstream
/// compares the pair verbatim, and any trimming/case folding here would turn
/// exact grouping into semantic merging.
///
/// Loop invariant: after processing index `k`, every group holds the indices
/// `0..=k` that share its key, in first-appearance order, and
/// `survivors`/`deletions` contain exactly those groups' elected
/// representatives and non-representatives. Termination: the single pass over
/// `memories` is finite. The group keys are replayed from `group_order` (a
/// `Vec`, not a `HashMap` iteration) precisely so the report order cannot
/// depend on hash order.
pub fn exact_plan(memories: &[DedupMemory]) -> ExactPlan {
    let mut group_order: Vec<(String, String)> = Vec::new();
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();

    for (index, memory) in memories.iter().enumerate() {
        let key = (memory.container_tag.clone(), memory.content.clone());
        if !groups.contains_key(&key) {
            group_order.push(key.clone());
        }
        groups.entry(key).or_default().push(index);
    }

    let mut survivors = Vec::new();
    let mut deletions = Vec::new();
    for key in group_order {
        let Some(mut members) = groups.remove(&key) else {
            continue;
        };
        members.sort_by(|left, right| {
            memories[*right]
                .created_at
                .cmp(&memories[*left].created_at)
                .then_with(|| memories[*left].id.cmp(&memories[*right].id))
        });
        let Some((survivor, duplicates)) = members.split_first() else {
            continue;
        };
        survivors.push(*survivor);
        deletions.extend(duplicates.iter().copied());
    }

    ExactPlan {
        survivors,
        deletions,
    }
}

/// One reported near-duplicate group, as indices into the input slice.
#[derive(Debug, Clone, PartialEq)]
pub struct NearGroup {
    pub representative: usize,
    pub duplicates: Vec<(usize, f64)>,
}

/// How often a running scan re-reads the cancellation flag, in candidate
/// comparisons. A stride keeps the atomic load off the per-pair path; at the
/// measured cost of one comparison the scan notices a shutdown within a few
/// milliseconds.
pub const NEAR_SCAN_CHECK_STRIDE: u64 = 1024;

/// The work bound and the shutdown signal one scan runs under.
pub struct NearScanLimits {
    /// Candidate comparisons this call may perform. `u64::MAX` disables the
    /// bound (tests and the measurement harness only).
    pub max_pairs: u64,
    /// One-way shutdown flag; when set, the scan returns at its next check.
    pub cancel: Arc<AtomicBool>,
}

/// What one bounded scan did.
#[derive(Debug)]
pub struct NearScan {
    pub groups: Vec<NearGroup>,
    /// Comparisons actually performed: one per inner-loop candidate visited,
    /// whether or not a cosine was computed.
    pub pairs_scanned: u64,
    pub truncated: bool,
    pub cancelled: bool,
}

/// Upstream's second pass, verbatim: greedy, non-transitive, per shard — plus
/// a work bound and a cooperative cancel.
///
/// - The representative is **never** inserted into `processed` — only the
///   candidates it claims are, which is what makes the scan greedy rather
///   than transitive.
/// - A memory already claimed as a candidate is skipped both as a later
///   representative and as a later candidate.
/// - `similarity < 1.0` is load-bearing: identical vectors should already
///   have been collapsed by the exact pass.
/// - A row with no parsed vector is invisible to this pass (never treated as
///   similarity `0.0` or `1.0`).
/// - Nothing is deleted, merged, or re-embedded here; the scan only reports.
///
/// **The work bound counts candidate comparisons, not cosines.** One inner-loop
/// iteration is one unit of work whether it ends in a cosine, a tag rejection,
/// a `processed` rejection or a missing vector, because every one of those paths
/// costs time and the last of them grows with the data. `max_pairs` is tested
/// *before* the comparison, so `pairs_scanned <= max_pairs` holds exactly; on
/// exhaustion the scan sets `truncated` and keeps the group it has built so far
/// if that group already holds a duplicate (the greedy order of the scanned
/// prefix is unchanged).
///
/// `processed` is a `HashSet` because it is membership-only — never iterated,
/// so output order still comes from the input order — and a `Vec` membership
/// scan under the pair loop is exactly the super-quadratic term that made the
/// un-bounded kernel's per-pair cost grow with the data.
pub fn near_groups(
    memories: &[DedupMemory],
    survivors: &[usize],
    threshold: f64,
    limits: &NearScanLimits,
) -> NearScan {
    let mut processed: HashSet<&str> = HashSet::new();
    let mut out: Vec<NearGroup> = Vec::new();
    let mut pairs_scanned: u64 = 0;
    let mut truncated = false;
    let mut cancelled = false;

    'outer: for (position, survivor) in survivors.iter().copied().enumerate() {
        if limits.cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let Some(mem1) = memories.get(survivor) else {
            continue;
        };
        let Some(vector1) = mem1.vector.as_deref() else {
            continue;
        };
        if processed.contains(mem1.id.as_str()) {
            continue;
        }

        let mut group = NearGroup {
            representative: survivor,
            duplicates: Vec::new(),
        };
        let mut stop = false;

        for candidate in survivors.iter().copied().skip(position + 1) {
            // The budget is checked BEFORE the comparison, so the bound is a
            // ceiling and never an overshoot.
            if pairs_scanned >= limits.max_pairs {
                truncated = true;
                stop = true;
                break;
            }
            // The flag is re-read once per stride (and at every outer
            // iteration above), which is what keeps shutdown latency bounded
            // without an atomic load per pair.
            if pairs_scanned.is_multiple_of(NEAR_SCAN_CHECK_STRIDE)
                && limits.cancel.load(Ordering::Relaxed)
            {
                cancelled = true;
                stop = true;
                break;
            }
            pairs_scanned = pairs_scanned.saturating_add(1);

            let Some(mem2) = memories.get(candidate) else {
                continue;
            };
            let Some(vector2) = mem2.vector.as_deref() else {
                continue;
            };
            if processed.contains(mem2.id.as_str()) {
                continue;
            }
            if mem1.container_tag != mem2.container_tag {
                continue;
            }

            let similarity = cosine_similarity(vector1, vector2);
            if similarity >= threshold && similarity < 1.0 {
                group.duplicates.push((candidate, similarity));
                processed.insert(mem2.id.as_str());
            }
        }

        if !group.duplicates.is_empty() {
            out.push(group);
        }
        if stop {
            break 'outer;
        }
    }

    NearScan {
        groups: out,
        pairs_scanned,
        truncated,
        cancelled,
    }
}
