//! W5.3 Step 7 / AC3 — durability, retention and bounded-shutdown oracles:
//! superseded captures keep DISTINCT tier-1 rows once each; a crash
//! before any drain replays durably; the shutdown flush completes quickly
//! handling only summary-supplied entries and never invokes an LLM.

mod common;

use std::sync::Arc;

use common::{FixedScopeResolver, TestDir};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_bridge::project_scope::ResolvedProjectScope;
use mem_store::db::Value;

fn params(capture_id: &str, summary: &str) -> CaptureParams {
    CaptureParams {
        session_id: "sess-1".to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: Some("build".to_string()),
        parent_session_id: None,
        transcript: Some("t".to_string()),
        summary: Some(summary.to_string()),
        metadata: Some(json!({ "promptId": "p-1" })),
        ..Default::default()
    }
}

struct FixedScopeRealWriter {
    deps: mem_bridge::capture::CaptureWriterDeps,
}

impl mem_bridge::capture::Tier1Writer for FixedScopeRealWriter {
    fn write<'a>(
        &'a self,
        scope: &'a ResolvedProjectScope,
        input: &'a mem_bridge::capture::CaptureSummaryInput,
    ) -> mem_embed::embedder::BoxFuture<
        'a,
        Result<mem_bridge::capture::Tier1WriteOutcome, mem_bridge::capture::CaptureWriterError>,
    > {
        Box::pin(mem_bridge::capture::write_capture_summary(
            &self.deps, scope, input,
        ))
    }
}

async fn fixture_writer(dir: &TestDir) -> FixedScopeRealWriter {
    let shards = std::sync::Arc::new(common::shard_manager(dir.store_config()));
    FixedScopeRealWriter {
        deps: mem_bridge::capture::CaptureWriterDeps {
            shards,
            embedder: Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
            prompt_stamps: Arc::new(mem_bridge::capture::AbsentPromptStampSource),
        },
    }
}

async fn service(dir: &TestDir) -> CaptureService {
    let writer = Arc::new(fixture_writer(dir).await);
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        writer,
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        None,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
}

/// A live session's superseded capture: the staging row keeps the newer
/// content, yet each unique capture_id has its own deterministic tier-1
/// row EXACTLY once (matching upstream's per-prompt memory write).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_superseded_capture_keeps_distinct_tier1_summary_once() {
    let dir = TestDir::new("w53-retention");
    let service = service(&dir).await;

    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000001",
            "older summary",
        ))
        .await
        .expect("enqueue");
    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000002",
            "newer summary",
        ))
        .await
        .expect("enqueue");

    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.done, 2, "both entries complete");

    // The staging row keeps the NEWER content only.
    let store = open_store(&dir).await;
    let sessions = store.list_sessions("/srv/proj", 10).await.expect("list");
    assert_eq!(sessions.len(), 1, "the staging tier is the upsert ladder");
    assert_eq!(sessions[0].summary, "newer summary");

    // The tier-1 tier keeps BOTH, once each.
    let mut rows = tier1_rows(&dir).await;
    rows.sort();
    assert_eq!(
        rows,
        {
            let mut expected = vec![
                mem_bridge::capture::capture_summary_row_id(
                    "opencode_project_0123456789abcdef",
                    "019a0000-0000-7f00-8000-000000000001",
                ),
                mem_bridge::capture::capture_summary_row_id(
                    "opencode_project_0123456789abcdef",
                    "019a0000-0000-7f00-8000-000000000002",
                ),
            ];
            expected.sort();
            expected
        },
        "each unique capture_id has its own deterministic tier-1 row exactly once"
    );

    // Re-draining is a safe no-op: everything is already done.
    let second = service.drain_once().await.expect("second drain");
    assert_eq!(
        second.done + second.retryable + second.dead_lettered + second.deferred,
        0
    );
    let rows = tier1_rows(&dir).await;
    assert_eq!(rows.len(), 2, "no tier-1 duplication on replay");
}

/// A crash after enqueue but before any drain: a rebuilt runtime replays
/// the durable entry and writes BOTH tiers exactly once (row exists).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_crash_replay_row_exists() {
    let dir = TestDir::new("w53-crash");
    {
        let service = service(&dir).await;
        service
            .enqueue(&params(
                "019a0000-0000-7f00-8000-000000000005",
                "crashed work",
            ))
            .await
            .expect("enqueue");
        // "crash": no drain ran — the ROW persists on disk.
        drop(service);
    }
    {
        // Restart: the rebuilt service drains the same durable entry.
        let service = service(&dir).await;
        let report = service.drain_once().await.expect("replay drain");
        assert_eq!(report.done, 1);
        let store = open_store(&dir).await;
        let sessions = store.list_sessions("/srv/proj", 10).await.expect("list");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].capture_id.as_deref(),
            Some("019a0000-0000-7f00-8000-000000000005")
        );
        assert_eq!(
            tier1_rows(&dir).await.len(),
            1,
            "both tiers exist after replay"
        );
    }
}

/// The shutdown flush: bounded, no LLM, only already-supplied summaries.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_shutdown_bounded_with_pending_entry() {
    let dir = TestDir::new("w53-shutdown-flush");
    let service = service(&dir).await;

    // (1) An entry WITHOUT a summary stays queued on the shutdown path.
    service
        .enqueue(&CaptureParams {
            session_id: "sess-1".to_string(),
            capture_id: "019a0000-0000-7f00-8000-000000000006".to_string(),
            project_id: "/srv/proj".to_string(),
            agent_id: "agent-1".to_string(),
            agent_name: None,
            parent_session_id: None,
            transcript: Some("needs an LLM".to_string()),
            summary: None,
            metadata: None,
            ..Default::default()
        })
        .await
        .expect("enqueue");

    let started = std::time::Instant::now();
    let report = service
        .flush_supplied_summary_queued()
        .await
        .expect("flush");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(450),
        "the flush stays inside the 500 ms DB_FLUSH budget"
    );
    assert_eq!(report.done, 0, "an LLM-less entry is not flushed");
    assert_eq!(
        service.inbox().queued_count().await.expect("count"),
        1,
        "the entry stays queued for the next start's replay"
    );

    // (2) An entry WITH a summary IS flushed.
    service
        .enqueue(&params(
            "019a0000-0000-7f00-8000-000000000007",
            "flushed summary",
        ))
        .await
        .expect("enqueue");
    let report = service
        .flush_supplied_summary_queued()
        .await
        .expect("flush 2");
    assert_eq!(report.done, 1);
    assert_eq!(service.inbox().queued_count().await.expect("count"), 1);
    assert_eq!(tier1_rows(&dir).await.len(), 1);
    assert_eq!(
        open_store(&dir)
            .await
            .list_sessions("/srv/proj", 10)
            .await
            .expect("list")
            .len(),
        1
    );

    // Step 5's flush path also exists (no purge reaper was added).
    assert_eq!(mem_episodic::inbox::INBOX_CAPACITY, 1024);
}

async fn open_store(dir: &TestDir) -> mem_episodic::store::EpisodicStore {
    let store = EpisodicStore::new(dir.path().to_path_buf(), dir.store_config());
    store.ready().await.expect("ready");
    store
}

async fn tier1_rows(dir: &TestDir) -> Vec<String> {
    let shards = common_shards(dir);
    let infos = shards
        .get_all_shards(mem_core::scope::Scope::Project, Some("0123456789abcdef"))
        .await
        .expect("shards");
    let mut ids = Vec::new();
    for info in infos {
        let db = shards.open_shard(&info.db_path).await.expect("open");
        let rows = db
            .query(
                "SELECT id FROM memories WHERE container_tag = ?",
                vec![Value::Text("opencode_project_0123456789abcdef".to_string())],
            )
            .await
            .expect("query");
        for row in rows {
            if let Some(id) = row.text("id") {
                ids.push(id);
            }
        }
    }
    ids
}

fn common_shards(dir: &TestDir) -> mem_store::shard_manager::ShardManager {
    common::shard_manager(dir.store_config())
}
