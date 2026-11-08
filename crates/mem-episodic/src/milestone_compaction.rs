//! Issue 43 W6c — the milestone dead-image counter (Step 16).
//!
//! The audit's `rewrite-needs-sweep`: an `UPDATE` leaves the old page images in
//! the file exactly as a `DELETE` does. The sweep's answer is `VACUUM` itself
//! (D-19-5: it never selects a row by deletion-eligibility, and no new `UPDATE`
//! path exists). This module is therefore ONLY the counter producer: it counts
//! milestones whose `contributors` JSON already carries a scrubbed
//! (empty-summary) contributor, so the sweep report can name
//! `milestone_compacted` truthfully.
//!
//! Invariants (invariant-guard): rows are read in rowid order; the loop
//! invariant is "counted so far = scrubbed-contributor rows among rows[0..i)";
//! the scan advances by `PAGE` and terminates when a page returns fewer than
//! `PAGE` rows (bounded by the table's row count).
//!
//! A corrupt `contributors` row is SKIPPED (fail-soft, the shipped pre-scan rule
//! in `purge.rs`): a count must never refuse the sweep.

use mem_store::db::{Db, Value, query_one};

use crate::error::EpisodicError;
use crate::milestone::decode_contributors;

/// The offset-scan page size. Bounded so a huge milestone table never loads in
/// one query; the loop invariant covers a partial final page.
pub const PAGE: i64 = 256;

/// Counts the milestones that carry at least one scrubbed (empty-summary)
/// contributor.
///
/// `VACUUM` performs the physical rewrite; this function never mutates a row.
pub async fn count_scrubbed_milestones(db: &dyn Db) -> Result<u64, EpisodicError> {
    let mut counted: u64 = 0;
    let mut offset: i64 = 0;
    loop {
        let rows = db
            .query(
                "SELECT contributors FROM episodic_milestones ORDER BY rowid LIMIT ? OFFSET ?",
                vec![Value::Integer(PAGE), Value::Integer(offset)],
            )
            .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let json = row.text("contributors").unwrap_or_default();
            // Fail-soft: an undecodable row is skipped, never a refusal.
            let Ok(contributors) = decode_contributors(&json) else {
                continue;
            };
            if contributors
                .iter()
                .any(|contributor| contributor.summary.is_empty())
            {
                counted += 1;
            }
        }
        let fetched = i64::try_from(rows.len()).unwrap_or(PAGE);
        if fetched < PAGE {
            break;
        }
        offset += fetched;
    }
    Ok(counted)
}

/// Whether a milestone table exists at all. The sweep's episodic leg opens a
/// database an engine created; a fresh file has no table, and the counter must
/// answer 0 rather than fail the sweep.
pub async fn milestone_table_exists(db: &dyn Db) -> Result<bool, EpisodicError> {
    let row = query_one(
        db,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'episodic_milestones'",
        Vec::new(),
    )
    .await?;
    Ok(row.is_some())
}
