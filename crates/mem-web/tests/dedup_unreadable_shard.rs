//! An unreadable shard must never be reported as a fully-read, empty one
//! (issue 55): the pass counts it in `nearDuplicateScan.unreadableShards`,
//! forces `truncated`, and keeps walking the remaining shards. The
//! mirror-image lie — a genuinely empty shard claiming truncation — is
//! forbidden too (AC5).
//!
//! The unreadable shard is built deterministically: the file opens (the
//! connection-level pragmas run), then `DROP TABLE memories` makes the pass's
//! `SELECT *` fail at prepare time. `ShardManager: Clone` shares the
//! connection cache, so the pass reads through the very handle the fixture
//! broke.

mod common;

use std::sync::Arc;

use common::real_store::{
    RealStore, create_shard, insert, new_store, row_ids, unit_x, unit_y, with_cos,
};
use mem_core::scope::Scope;
use mem_web::api::deduplication::DedupService;
use mem_web::api::deduplication::store::ShardDedupStore;

const HASH: &str = "a1b2c3d4e5f67890";
const TAG: &str = "opencode_project_a1b2c3d4e5f67890";

fn service(store: &RealStore) -> DedupService {
    DedupService::new(
        Arc::new(ShardDedupStore::new(store.manager.clone())),
        true,
        0.9,
    )
}

/// Drops the `memories` table from a shard whose file still opens.
async fn make_unreadable(store: &RealStore, shard: &mem_store::shard_manager::ShardInfo) {
    let db = store
        .manager
        .open_shard(&shard.db_path)
        .await
        .expect("the shard opens");
    db.execute("DROP TABLE memories", Vec::new())
        .await
        .expect("the table drops");
}

#[tokio::test]
async fn test_unreadable_shard_is_counted_and_never_reported_as_empty() {
    let store = new_store("dedup-unreadable-counted");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    // Two distinct rows: no exact duplicate exists and no near pair is
    // claimed, so the report's only interesting value is the coverage claim.
    insert(&store, &shard, "mem_a", "content a", TAG, 2000, unit_x()).await;
    insert(&store, &shard, "mem_b", "content b", TAG, 1000, unit_y()).await;
    make_unreadable(&store, &shard).await;

    let report = service(&store)
        .run()
        .await
        .expect("an unreadable shard does not abort the pass");

    assert!(
        report.near_duplicate_scan.truncated,
        "a shard that was never read must withdraw the full-coverage claim: {report:?}"
    );
    assert_eq!(
        report.near_duplicate_scan.unreadable_shards, 1,
        "the shard is counted, never dropped: {report:?}"
    );
    assert_eq!(report.near_duplicate_scan.pairs_scanned, 0);
    assert!(report.near_duplicate_groups.is_empty());
    assert_eq!(report.exact_duplicates_deleted, 0);
}

#[tokio::test]
async fn test_unreadable_shard_does_not_stop_the_rest_of_the_pass() {
    let store = new_store("dedup-unreadable-degrades");
    // Project shards index 0 then 1, i.e. the walk order the registry SQL
    // defines (`ORDER BY shard_index ASC`). Shard 0 is broken before the run.
    let broken = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(&store, &broken, "lost_a", "content a", TAG, 2000, unit_x()).await;
    insert(&store, &broken, "lost_b", "content b", TAG, 1000, unit_y()).await;
    make_unreadable(&store, &broken).await;

    // Shard 1 is healthy: one exact duplicate pair (newest wins) plus one
    // distinct row at cosine 0.95 from the newest survivor, so the pass must
    // delete once and report exactly one near group.
    let healthy = create_shard(&store, Scope::Project, HASH, 1).await;
    insert(
        &store,
        &healthy,
        "mem_new",
        "same content",
        TAG,
        3000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &healthy,
        "mem_old",
        "same content",
        TAG,
        2000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &healthy,
        "mem_near",
        "near content",
        TAG,
        1000,
        with_cos(0.95),
    )
    .await;

    let report = service(&store)
        .run()
        .await
        .expect("one unreadable shard does not abort the pass");

    assert_eq!(
        report.near_duplicate_scan.unreadable_shards, 1,
        "only the broken shard is unreadable: {report:?}"
    );
    assert!(
        report.near_duplicate_scan.truncated,
        "the broken shard's contents are unknown: {report:?}"
    );
    assert_eq!(
        report.exact_duplicates_deleted, 1,
        "the healthy shard's older exact duplicate is still deleted"
    );
    let mut remaining = row_ids(&store, &healthy).await;
    remaining.sort();
    assert_eq!(
        remaining,
        vec!["mem_near".to_string(), "mem_new".to_string()],
        "the healthy shard keeps its newest survivor and the distinct row"
    );
    assert_eq!(
        report.near_duplicate_groups.len(),
        1,
        "the healthy shard's near pair is reported: {report:?}"
    );
    assert_eq!(
        report.near_duplicate_groups[0].representative.id, "mem_new",
        "the newest row represents the pair"
    );
}

#[tokio::test]
async fn test_genuinely_empty_shard_reports_no_truncation() {
    // The mirror-image lie: a shard that really holds no rows must not look
    // unreadable. This is a regression guard, not a pre-change failure — the
    // counter-less base answered `truncated: false` here correctly, and it
    // must keep doing so once the read fault has its own arm.
    let store = new_store("dedup-unreadable-empty");
    create_shard(&store, Scope::Project, HASH, 0).await;

    let report = service(&store)
        .run()
        .await
        .expect("an empty shard is readable");

    assert_eq!(report.near_duplicate_scan.pairs_scanned, 0);
    assert!(
        !report.near_duplicate_scan.truncated,
        "an empty shard is fully covered: {report:?}"
    );
    assert_eq!(report.near_duplicate_scan.unreadable_shards, 0);
    assert_eq!(report.exact_duplicates_deleted, 0);
    assert!(report.near_duplicate_groups.is_empty());
}
