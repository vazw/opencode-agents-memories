//! W7.1 — the facts erasure path (D6, M17, M18, M21).
//!
//! `purge_sources` is the ONE hard delete in the facts tier. Its counts are
//! distinct rows measured from the union of its predicates BEFORE any
//! mutation, and its orphan sweep is project-scoped.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mem_core::tier::MemoryTier;
use mem_facts::FactsError;
use mem_facts::purge::FactsPurgeReport;
use mem_facts::store::{FactInput, FactSource, FactsStore};
use mem_store::config::StoreConfig;
use mem_store::db::query_one;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn store_config(label: &str) -> StoreConfig {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "mem-facts-w71-purge-{label}-{}-{counter}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    StoreConfig {
        storage_path: dir,
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

fn source(project: &str, session: &str, agent: &str, milestone: &str) -> FactSource {
    FactSource {
        project_id: project.to_string(),
        session_id: session.to_string(),
        agent_id: agent.to_string(),
        milestone_id: milestone.to_string(),
    }
}

fn input(project: &str, tier: MemoryTier, content: &str, sources: Vec<FactSource>) -> FactInput {
    FactInput {
        project_id: project.to_string(),
        tier,
        content: content.to_string(),
        key: None,
        source_ref: sources.first().map(|source| source.milestone_id.clone()),
        confidence: None,
        metadata: None,
        embedding: None,
        model_id: None,
        sources,
    }
}

async fn provenance_count(store: &FactsStore) -> i64 {
    let db = store.ready().await.expect("ready");
    query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM fact_provenance",
        Vec::new(),
    )
    .await
    .expect("count")
    .expect("row")
    .i64("count")
    .unwrap_or(0)
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_sources_deletes_facts_and_provenance_for_the_triple() {
    let store = FactsStore::new(store_config("triple"));
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "the first fact",
            vec![source("project-a", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("insert 1");
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "the second fact",
            vec![source("project-a", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("insert 2");

    let report = store
        .purge_sources("project-a", "session-a", "agent-a", &[])
        .await
        .expect("purge");
    assert_eq!(
        report,
        FactsPurgeReport {
            facts: 2,
            fact_sources: 2,
        }
    );
    assert_eq!(store.count("project-a").await.expect("count"), 0);
    assert_eq!(provenance_count(&store).await, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_sources_counts_distinct_rows_when_both_predicates_match() {
    let store = FactsStore::new(store_config("overlap"));
    // Two facts of session A, both provenance-tagged with ms-1.
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "fact a one",
            vec![source("project-a", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("a1");
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "fact a two",
            vec![source("project-a", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("a2");
    // One fact of session B that the SAME milestone produced.
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "fact b one",
            vec![source("project-a", "session-b", "agent-a", "ms-1")],
        ))
        .await
        .expect("b1");
    assert_eq!(store.count("project-a").await.expect("count"), 3);
    assert_eq!(provenance_count(&store).await, 3);

    // Purging A with the milestone id: both predicates match A's two facts,
    // and the milestone predicate matches B's fact. The union is 3 facts and
    // 3 provenance rows — never 2 + 3 = 5.
    let report = store
        .purge_sources("project-a", "session-a", "agent-a", &["ms-1".to_string()])
        .await
        .expect("purge");
    assert_eq!(
        report,
        FactsPurgeReport {
            facts: 3,
            fact_sources: 3,
        },
        "distinct rows from the union, not a sum of changes"
    );
    assert_eq!(store.count("project-a").await.expect("count"), 0);
    assert_eq!(provenance_count(&store).await, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_sources_deletes_facts_whose_milestone_is_purged_even_when_their_session_survives()
 {
    let store = FactsStore::new(store_config("milestone"));
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "fact b one",
            vec![source("project-a", "session-b", "agent-a", "ms-1")],
        ))
        .await
        .expect("b1");

    // No triple row matches; the milestone predicate alone deletes the fact,
    // so no `source_ref` can dangle at a destroyed milestone.
    let report = store
        .purge_sources("project-a", "session-a", "agent-a", &["ms-1".to_string()])
        .await
        .expect("purge");
    assert_eq!(
        report,
        FactsPurgeReport {
            facts: 1,
            fact_sources: 1
        }
    );
    assert_eq!(store.count("project-a").await.expect("count"), 0);
    assert_eq!(provenance_count(&store).await, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_sources_leaves_other_projects_tiers_and_sessions() {
    let store = FactsStore::new(store_config("isolation"));
    // The target triple.
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "the target fact",
            vec![source("project-a", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("target");
    // Same session id, other project.
    store
        .insert(input(
            "project-b",
            MemoryTier::Semantic,
            "another project fact",
            vec![source("project-b", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("other project");
    // Same project, another tier.
    store
        .insert(input(
            "project-a",
            MemoryTier::Procedural,
            "a procedural fact",
            vec![source("project-a", "session-c", "agent-a", "ms-2")],
        ))
        .await
        .expect("other tier");
    // Same project, another session.
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "another session fact",
            vec![source("project-a", "session-d", "agent-a", "ms-3")],
        ))
        .await
        .expect("other session");

    let report = store
        .purge_sources("project-a", "session-a", "agent-a", &[])
        .await
        .expect("purge");
    assert_eq!(
        report,
        FactsPurgeReport {
            facts: 1,
            fact_sources: 1
        }
    );
    assert_eq!(store.count("project-a").await.expect("count a"), 2);
    assert_eq!(store.count("project-b").await.expect("count b"), 1);
    assert_eq!(provenance_count(&store).await, 3, "three untouched rows");
    assert!(
        store
            .get_by_key("project-b", MemoryTier::Semantic, "another project fact")
            .await
            .expect("get")
            .is_some(),
        "the scoped orphan sweep never writes across projects"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_sources_is_idempotent_and_unknown_scope_returns_zeros() {
    let store = FactsStore::new(store_config("idempotent"));
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "the fact",
            vec![source("project-a", "session-a", "agent-a", "ms-1")],
        ))
        .await
        .expect("insert");

    let first = store
        .purge_sources("project-a", "session-a", "agent-a", &[])
        .await
        .expect("first");
    assert_eq!(
        first,
        FactsPurgeReport {
            facts: 1,
            fact_sources: 1
        }
    );
    let second = store
        .purge_sources("project-a", "session-a", "agent-a", &[])
        .await
        .expect("second");
    assert_eq!(second, FactsPurgeReport::default());
    assert_eq!(
        store
            .purge_sources(
                "project-a",
                "no-such-session",
                "agent-a",
                &["no-such-ms".to_string()]
            )
            .await
            .expect("unknown"),
        FactsPurgeReport::default()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_sources_deletes_the_multi_source_fact_and_its_provenance() {
    let store = FactsStore::new(store_config("multi-source"));
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "a fact with two sources",
            vec![
                source("project-a", "session-a", "agent-a", "ms-1"),
                source("project-a", "session-b", "agent-a", "ms-1"),
            ],
        ))
        .await
        .expect("multi");
    store
        .insert(input(
            "project-a",
            MemoryTier::Semantic,
            "b's own fact",
            vec![source("project-a", "session-b", "agent-a", "ms-2")],
        ))
        .await
        .expect("b own");
    assert_eq!(provenance_count(&store).await, 3);

    // M18: purging ONE source deletes the fact AND every provenance row of
    // that fact, including the surviving session's — erasure wins over
    // durability. B's other facts are untouched.
    let report = store
        .purge_sources("project-a", "session-a", "agent-a", &[])
        .await
        .expect("purge");
    assert_eq!(
        report,
        FactsPurgeReport {
            facts: 1,
            fact_sources: 2
        }
    );
    assert_eq!(provenance_count(&store).await, 1, "only B's own fact's row");
    assert!(
        store
            .get_by_key("project-a", MemoryTier::Semantic, "a fact with two sources")
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        store
            .get_by_key("project-a", MemoryTier::Semantic, "b's own fact")
            .await
            .expect("get")
            .is_some()
    );
}

/// The validation surface is shared with `insert`; a purge never opens a
/// non-existent database implicitly beyond `ready`'s schema apply.
#[allow(dead_code)]
fn _error_witness(_: FactsError) {}
