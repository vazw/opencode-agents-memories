//! The production `DedupStore` over real shard files (`api::deduplication`).
//!
//! Five cases run against a real temp store from the shared
//! `common::real_store` fixture (a real `ShardManager`, real shard files, real
//! vectors); `test_order_rows_sorts_by_created_at_desc_then_id_asc` is a pure,
//! DB-free guard on the ordering function itself.

mod common;

use std::sync::Arc;

use common::real_store::{
    create_shard, insert, new_store, row_ids, unit_x, unit_y, vector_count, with_cos, with_cos_y,
};
use mem_core::scope::Scope;
use mem_store::vector;
use mem_web::api::deduplication::store::{ShardDedupStore, order_rows};
use mem_web::api::deduplication::{DedupMemory, DedupService};

const HASH: &str = "a1b2c3d4e5f67890";
const TAG: &str = "opencode_project_a1b2c3d4e5f67890";

fn service(store: &common::real_store::RealStore) -> DedupService {
    DedupService::new(
        Arc::new(ShardDedupStore::new(store.manager.clone())),
        true,
        0.9,
    )
}

#[tokio::test]
async fn test_dedup_store_deletes_the_older_exact_duplicate_and_decrements_the_count() {
    let store = new_store("dedup-store-delete");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    insert(
        &store,
        &shard,
        "mem_new",
        "same content",
        TAG,
        3000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_mid",
        "same content",
        TAG,
        2000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_old",
        "same content",
        TAG,
        1000,
        unit_x(),
    )
    .await;
    assert_eq!(vector_count(&store, &shard).await.vector_count, 3);

    let report = service(&store)
        .run()
        .await
        .expect("the real-store walk succeeds");
    assert_eq!(report.exact_duplicates_deleted, 2);
    assert_eq!(
        vector_count(&store, &shard).await.vector_count,
        1,
        "the registry count moved with the deletions"
    );

    let db = store
        .manager
        .open_shard(&shard.db_path)
        .await
        .expect("the shard opens");
    for deleted in ["mem_mid", "mem_old"] {
        assert!(
            vector::get_memory_by_id(db.as_ref(), deleted)
                .await
                .expect("the row reads")
                .is_none(),
            "{deleted} is gone with its vector (no orphan)"
        );
    }
    let survivor = vector::get_memory_by_id(db.as_ref(), "mem_new")
        .await
        .expect("the row reads")
        .expect("the newest row survives");
    assert_eq!(survivor.i64("created_at"), Some(3000));
    assert_eq!(row_ids(&store, &shard).await, vec!["mem_new".to_string()]);
}

#[tokio::test]
async fn test_dedup_store_reports_a_near_duplicate_pair_without_deleting() {
    let store = new_store("dedup-store-near");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    // The exact pair (both `unit_x`, same tag and content): the exact pass
    // fires, so "the near path deleted nothing" is asserted against a run
    // that did delete.
    insert(
        &store,
        &shard,
        "mem_exact_new",
        "exact content",
        TAG,
        4000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_exact_old",
        "exact content",
        TAG,
        3000,
        unit_x(),
    )
    .await;
    // The near pair: different content, same tag, 0.95 apart.
    insert(
        &store,
        &shard,
        "mem_near_new",
        "near content new",
        TAG,
        2000,
        unit_y(),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_near_old",
        "near content old",
        TAG,
        1000,
        with_cos_y(0.95),
    )
    .await;
    assert_eq!(vector_count(&store, &shard).await.vector_count, 4);

    let report = service(&store)
        .run()
        .await
        .expect("the real-store walk succeeds");
    assert_eq!(report.exact_duplicates_deleted, 1);

    let ids = row_ids(&store, &shard).await;
    assert_eq!(ids.len(), 3, "exactly one row is gone: {ids:?}");
    assert!(
        !ids.contains(&"mem_exact_old".to_string()),
        "the older exact copy is the deleted one: {ids:?}"
    );

    assert_eq!(report.near_duplicate_groups.len(), 1);
    let group = &report.near_duplicate_groups[0];
    assert_eq!(group.representative.id, "mem_near_new");
    assert_eq!(group.duplicates.len(), 1);
    assert_eq!(group.duplicates[0].id, "mem_near_old");
    assert!(
        (0.94..0.96).contains(&group.duplicates[0].similarity),
        "the reported similarity: {}",
        group.duplicates[0].similarity
    );

    assert!(
        ids.contains(&"mem_near_new".to_string()) && ids.contains(&"mem_near_old".to_string()),
        "the near path deletes nothing: {ids:?}"
    );
    assert_eq!(
        vector_count(&store, &shard).await.vector_count,
        3,
        "the count is down by exactly one"
    );
}

#[tokio::test]
async fn test_dedup_store_ties_break_on_the_smaller_id_in_either_insert_order() {
    for (label, first, second) in [("z-first", "mem_z", "mem_a"), ("a-first", "mem_a", "mem_z")] {
        let store = new_store(&format!("dedup-store-ties-{label}"));
        let shard = create_shard(&store, Scope::Project, HASH, 0).await;
        insert(&store, &shard, first, "same content", TAG, 1000, unit_x()).await;
        insert(&store, &shard, second, "same content", TAG, 1000, unit_x()).await;

        let report = service(&store)
            .run()
            .await
            .expect("the real-store walk succeeds");
        assert_eq!(
            report.exact_duplicates_deleted, 1,
            "{label}: one duplicate is deleted"
        );
        assert_eq!(
            row_ids(&store, &shard).await,
            vec!["mem_a".to_string()],
            "{label}: the smaller id survives real SQL"
        );
    }
}

#[test]
fn test_order_rows_sorts_by_created_at_desc_then_id_asc() {
    let row = |id: &str| DedupMemory {
        id: id.to_string(),
        content: String::new(),
        container_tag: String::new(),
        created_at: 1000,
        vector: None,
    };
    let mut rows = vec![row("mem_d"), row("mem_c"), row("mem_b"), row("mem_a")];

    order_rows(&mut rows);

    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, vec!["mem_a", "mem_b", "mem_c", "mem_d"]);
}

#[tokio::test]
async fn test_dedup_store_orders_near_groups_by_created_at_then_id() {
    let store = new_store("dedup-store-order");
    let shard = create_shard(&store, Scope::Project, HASH, 0).await;
    // Every row has the same `created_at`, inserted larger-id-first: only the
    // `id ASC` tie-break can put `mem_a` before `mem_b`.
    insert(
        &store,
        &shard,
        "mem_d",
        "content d",
        TAG,
        1000,
        with_cos(0.95),
    )
    .await;
    insert(
        &store,
        &shard,
        "mem_c",
        "content c",
        TAG,
        1000,
        with_cos_y(0.95),
    )
    .await;
    insert(&store, &shard, "mem_b", "content b", TAG, 1000, unit_y()).await;
    insert(&store, &shard, "mem_a", "content a", TAG, 1000, unit_x()).await;

    let report = service(&store)
        .run()
        .await
        .expect("the real-store walk succeeds");
    assert_eq!(report.exact_duplicates_deleted, 0);

    let pairs: Vec<(String, String)> = report
        .near_duplicate_groups
        .iter()
        .map(|group| {
            (
                group.representative.id.clone(),
                group.duplicates[0].id.clone(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("mem_a".to_string(), "mem_d".to_string()),
            ("mem_b".to_string(), "mem_c".to_string()),
        ],
        "with `order_rows` removed, the two groups come out in another order"
    );
}

#[tokio::test]
async fn test_dedup_store_never_deduplicates_across_shards() {
    let store = new_store("dedup-store-shards");
    let shard0 = create_shard(&store, Scope::Project, HASH, 0).await;
    let shard1 = create_shard(&store, Scope::Project, HASH, 1).await;
    // Same scope hash ⇒ same container tag by construction.
    insert(
        &store,
        &shard0,
        "mem_in_0",
        "same content",
        TAG,
        2000,
        unit_x(),
    )
    .await;
    insert(
        &store,
        &shard1,
        "mem_in_1",
        "same content",
        TAG,
        1000,
        unit_x(),
    )
    .await;

    let report = service(&store)
        .run()
        .await
        .expect("the two-shard walk succeeds");
    assert_eq!(
        report.exact_duplicates_deleted, 0,
        "each shard's own exact group holds exactly one member"
    );
    assert!(report.near_duplicate_groups.is_empty());

    assert_eq!(row_ids(&store, &shard0).await, vec!["mem_in_0".to_string()]);
    assert_eq!(row_ids(&store, &shard1).await, vec!["mem_in_1".to_string()]);
    assert_eq!(vector_count(&store, &shard0).await.vector_count, 1);
    assert_eq!(vector_count(&store, &shard1).await.vector_count, 1);
}
