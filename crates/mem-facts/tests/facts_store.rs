//! W7.1 — the facts tier store: round trip, deterministic ids, per
//! project+tier dedup, upsert semantics, soft delete, and provenance.
//!
//! `mem_facts::FactsStore` is the durable five-tier facts DB (`facts.db`). It
//! is NOT `mem_portability::facts::FactStore` (the tier-1 memory seam
//! portability already owns): the names differ by design (M30).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mem_core::tier::MemoryTier;
use mem_facts::FactsError;
use mem_facts::schema::fact_id;
use mem_facts::store::{FactInput, FactSource, FactWrite, FactsStore};
use mem_store::config::StoreConfig;
use mem_store::db::query_one;

static COUNTER: AtomicU64 = AtomicU64::new(0);

const DIMS: u32 = 8;

fn store_config() -> StoreConfig {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("mem-facts-w71-{}-{counter}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    StoreConfig {
        storage_path: dir,
        embedding_dimensions: DIMS,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

fn store() -> FactsStore {
    FactsStore::new(store_config())
}

fn source(project: &str, session: &str, agent: &str, milestone: &str) -> FactSource {
    FactSource {
        project_id: project.to_string(),
        session_id: session.to_string(),
        agent_id: agent.to_string(),
        milestone_id: milestone.to_string(),
    }
}

fn input(project: &str, tier: MemoryTier, content: &str) -> FactInput {
    FactInput {
        project_id: project.to_string(),
        tier,
        content: content.to_string(),
        key: None,
        source_ref: Some("milestone-1".to_string()),
        confidence: None,
        metadata: None,
        embedding: None,
        model_id: None,
        sources: vec![source(project, "session-a", "agent-a", "milestone-1")],
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_roundtrip_preserves_tier_key_and_source_ref() {
    let store = store();
    let write = store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "Turso keeps the invariant",
        ))
        .await
        .expect("insert");
    assert_eq!(write, FactWrite::Created);

    let fact = store
        .get_by_key(
            "project-a",
            MemoryTier::Semantic,
            "Turso keeps   the invariant",
        )
        .await
        .expect("get_by_key")
        .expect("present");
    assert_eq!(fact.project_id, "project-a");
    assert_eq!(fact.tier, MemoryTier::Semantic);
    assert_eq!(fact.content, "Turso keeps the invariant");
    assert_eq!(fact.key_normalized, "turso keeps the invariant");
    assert_eq!(fact.source_ref.as_deref(), Some("milestone-1"));
    assert_eq!(fact.version, 1);
    assert!(!fact.is_deleted());

    let by_id = store.get(&fact.id).await.expect("get").expect("present");
    assert_eq!(by_id, fact);
    assert_eq!(store.count("project-a").await.expect("count"), 1);
    assert_eq!(store.schema_version().await.expect("version"), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_ids_are_deterministic_per_project_tier_and_key() {
    let a = fact_id("project-a", MemoryTier::Semantic, "the same key");
    let b = fact_id("project-a", MemoryTier::Semantic, "the same key");
    assert_eq!(a, b, "deterministic");
    assert!(a.starts_with("fact-"), "{a}");
    assert_eq!(a.len(), "fact-".len() + 32, "{a}");
    assert_ne!(
        a,
        fact_id("project-b", MemoryTier::Semantic, "the same key")
    );
    assert_ne!(
        a,
        fact_id("project-a", MemoryTier::Procedural, "the same key")
    );

    // The stored row agrees with the helper.
    let store = store();
    store
        .insert(input("project-a", MemoryTier::Semantic, "the same key"))
        .await
        .expect("insert");
    let fact = store
        .get_by_key("project-a", MemoryTier::Semantic, "the same key")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(fact.id, a);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_upsert_bumps_version_and_clears_stale() {
    let store = store();
    store
        .insert(input("project-a", MemoryTier::Semantic, "the same key"))
        .await
        .expect("insert");
    let id = store
        .get_by_key("project-a", MemoryTier::Semantic, "the same key")
        .await
        .expect("get")
        .expect("present")
        .id;

    // Mark it stale and soft-deleted, then re-insert: the upsert bumps the
    // version and clears both flags.
    let db = store.ready().await.expect("ready");
    db.execute(
        "UPDATE facts SET stale = 1 WHERE id = ?",
        vec![mem_store::db::Value::Text(id.clone())],
    )
    .await
    .expect("mark stale");
    assert!(store.soft_delete(&id).await.expect("soft delete"));

    let write = store
        .insert(input("project-a", MemoryTier::Semantic, "the same key"))
        .await
        .expect("re-insert");
    assert_eq!(write, FactWrite::Updated);

    let fact = store.get(&id).await.expect("get").expect("present");
    assert_eq!(fact.version, 2, "the upsert bumps the version");
    assert!(!fact.is_deleted(), "deleted_at is cleared");
    assert_eq!(fact.stale, 0, "stale is cleared");
    assert_eq!(store.count("project-a").await.expect("count"), 1, "one row");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_dedup_is_per_project_and_per_tier() {
    let store = store();
    store
        .insert(input("project-a", MemoryTier::Semantic, "shared text"))
        .await
        .expect("insert a semantic");
    store
        .insert(input("project-a", MemoryTier::Semantic, "shared  text"))
        .await
        .expect("same project+tier+key upserts");
    store
        .insert(input("project-a", MemoryTier::Procedural, "shared text"))
        .await
        .expect("a different tier is a different fact");
    store
        .insert(input("project-b", MemoryTier::Semantic, "shared text"))
        .await
        .expect("a different project is a different fact");

    assert_eq!(store.count("project-a").await.expect("count a"), 2);
    assert_eq!(store.count("project-b").await.expect("count b"), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_rejects_non_fact_tiers() {
    let store = store();
    for tier in [MemoryTier::Profile, MemoryTier::Episodic, MemoryTier::Vault] {
        let error = store
            .insert(input("project-a", tier, "not a fact"))
            .await
            .expect_err("only the three fact tiers are accepted");
        match error {
            FactsError::Validation { field, .. } => assert_eq!(field, "tier", "{tier:?}"),
            other => panic!("{tier:?}: {other:?}"),
        }
    }
    assert_eq!(store.count("project-a").await.expect("count"), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_facts_store_reopens_without_reapplying() {
    let config = store_config();
    let store = FactsStore::new(config.clone());
    store
        .insert(input("project-a", MemoryTier::Semantic, "durable text"))
        .await
        .expect("insert");
    assert_eq!(store.schema_version().await.expect("version"), 1);
    drop(store);

    let reopened = FactsStore::new(config);
    assert_eq!(reopened.schema_version().await.expect("version"), 1);
    assert_eq!(reopened.count("project-a").await.expect("count"), 1);
    let fact = reopened
        .get_by_key("project-a", MemoryTier::Semantic, "durable text")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(fact.content, "durable text");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_soft_delete_hides_from_list_but_keeps_the_row() {
    let store = store();
    store
        .insert(input("project-a", MemoryTier::Semantic, "first"))
        .await
        .expect("insert first");
    let id = store
        .get_by_key("project-a", MemoryTier::Semantic, "first")
        .await
        .expect("get")
        .expect("present")
        .id;

    assert!(store.soft_delete(&id).await.expect("soft delete"));
    assert!(
        !store.soft_delete(&id).await.expect("idempotent"),
        "second call changes nothing"
    );

    assert!(store.list("project-a", 10).await.expect("list").is_empty());
    assert_eq!(store.count("project-a").await.expect("count"), 0);
    let kept = store.get(&id).await.expect("get").expect("the row is kept");
    assert!(kept.is_deleted(), "deleted_at is set");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_insert_rejects_empty_content() {
    let store = store();
    for content in ["", "   ", "\n\t"] {
        let error = store
            .insert(input("project-a", MemoryTier::Semantic, content))
            .await
            .expect_err("empty content is rejected");
        match error {
            FactsError::Validation { field, .. } => assert_eq!(field, "content"),
            other => panic!("{other:?}"),
        }
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_fact_insert_requires_provenance() {
    let store = store();
    let mut bad = input("project-a", MemoryTier::Semantic, "a fact with no source");
    bad.sources.clear();
    let error = store
        .insert(bad)
        .await
        .expect_err("empty provenance is refused");
    match error {
        FactsError::Validation { field, .. } => assert_eq!(field, "sources"),
        other => panic!("{other:?}"),
    }
    assert_eq!(store.count("project-a").await.expect("count"), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_provenance_records_one_row_per_contributing_session() {
    let store = store();
    let mut multi = input("project-a", MemoryTier::Semantic, "a two-session fact");
    multi.sources = vec![
        source("project-a", "session-a", "agent-a", "ms-1"),
        source("project-a", "session-b", "agent-a", "ms-1"),
    ];
    store.insert(multi.clone()).await.expect("insert");

    let provenance = |db: mem_store::db::DbHandle| async move {
        db.query(
            "SELECT session_id, milestone_id FROM fact_provenance ORDER BY session_id",
            Vec::new(),
        )
        .await
        .expect("provenance")
        .iter()
        .map(|row| {
            (
                row.text("session_id").unwrap_or_default(),
                row.text("milestone_id").unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
    };
    let db = store.ready().await.expect("ready");
    let rows = provenance(Arc::clone(&db)).await;
    assert_eq!(
        rows,
        vec![
            ("session-a".to_string(), "ms-1".to_string()),
            ("session-b".to_string(), "ms-1".to_string()),
        ]
    );

    // Re-promotion with the same sources does not duplicate rows.
    store.insert(multi).await.expect("re-insert");
    assert_eq!(provenance(db).await.len(), 2);
    let fact_count = query_one(
        store.ready().await.expect("ready").as_ref(),
        "SELECT COUNT(*) AS count FROM facts",
        Vec::new(),
    )
    .await
    .expect("count")
    .expect("row");
    assert_eq!(fact_count.i64("count"), Some(1));
}
