//! W7.1 — raw context: byte-equal round trip, first-write-wins, validation,
//! scope isolation and — the load-bearing invariant — survival of a full
//! `Consolidator::run` (ADR-0010 D-1, M1).

mod common;

use std::sync::Arc;

use common::{ConsolidationFixture, DAY_MILLIS, FakeEmbedder, TestDir};
use mem_episodic::raw_context::{MAX_RAW_CONTEXT_BYTES, RawContextWrite};
use mem_episodic::record::NewAgentSession;
use mem_episodic::segments::MAX_SEGMENT_BYTES;
use mem_episodic::store::EpisodicStore;

const PROJECT: &str = "project-a";
const SESSION: &str = "session-a";
const AGENT: &str = "agent-a";

async fn store_with_session(dir: &TestDir) -> (Arc<EpisodicStore>, Arc<FakeEmbedder>) {
    let store = Arc::new(EpisodicStore::new(dir.to_path_buf(), dir.store_config()));
    let embedder = Arc::new(FakeEmbedder::with_vector(vec![0.1; 8]));
    store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT.to_string(),
                session_id: SESSION.to_string(),
                agent_id: AGENT.to_string(),
                summary: "the session summary".to_string(),
                ..Default::default()
            },
            embedder.as_ref(),
        )
        .await
        .expect("insert session");
    (store, embedder)
}

fn sample() -> String {
    format!(
        "the first paragraph of the session\nstill the first paragraph\n\nsecond paragraph\n\n{}",
        "é🦀 padding ".repeat(2000)
    )
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_roundtrips_byte_equal() {
    let dir = TestDir::new("raw-roundtrip");
    let (store, _embedder) = store_with_session(&dir).await;
    let raw = sample();

    let outcome = store
        .insert_raw_context(PROJECT, SESSION, AGENT, &raw)
        .await
        .expect("insert raw context");
    assert_eq!(
        outcome,
        RawContextWrite::Inserted {
            segments: store
                .segments_for_session(PROJECT, SESSION, AGENT)
                .await
                .expect("segments")
                .len()
        }
    );

    let stored = store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("read")
        .expect("present");
    assert_eq!(stored.raw_context, raw);
    assert_eq!(stored.byte_len as usize, raw.len());
    assert_eq!(stored.project_id, PROJECT);
    assert_eq!(stored.session_id, SESSION);
    assert_eq!(stored.agent_id, AGENT);
    assert!(stored.captured_at > 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_segments_read_in_index_order() {
    let dir = TestDir::new("raw-segments-order");
    let (store, _embedder) = store_with_session(&dir).await;
    // Long enough to split under MAX_SEGMENT_BYTES.
    let raw = format!(
        "{}\n\n{}",
        "a".repeat(MAX_SEGMENT_BYTES),
        "b".repeat(MAX_SEGMENT_BYTES)
    );
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, &raw)
        .await
        .expect("insert");

    let segments = store
        .segments_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("segments");
    assert!(segments.len() >= 2, "{}", segments.len());
    for (index, segment) in segments.iter().enumerate() {
        assert_eq!(segment.segment_index, index as i64, "ordered by index");
    }
    let rebuilt: String = segments
        .iter()
        .map(|segment| segment.raw_content.as_str())
        .collect();
    assert_eq!(rebuilt, raw, "segments regenerate the raw bytes");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_first_write_wins() {
    let dir = TestDir::new("raw-first-wins");
    let (store, _embedder) = store_with_session(&dir).await;
    let first = "the first bytes\n\nsecond paragraph";
    let second = "different bytes entirely";

    assert!(matches!(
        store
            .insert_raw_context(PROJECT, SESSION, AGENT, first)
            .await
            .expect("first write"),
        RawContextWrite::Inserted { .. }
    ));
    assert_eq!(
        store
            .insert_raw_context(PROJECT, SESSION, AGENT, second)
            .await
            .expect("second write"),
        RawContextWrite::AlreadyPresent,
        "a recapture with different bytes is already-present, never an overwrite"
    );

    let stored = store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("read")
        .expect("present");
    assert_eq!(
        stored.raw_context, first,
        "the FIRST bytes are authoritative"
    );
    let segments = store
        .segments_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("segments");
    let rebuilt: String = segments
        .iter()
        .map(|segment| segment.raw_content.as_str())
        .collect();
    assert_eq!(rebuilt, first, "segments were not rewritten either");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_rejects_empty_oversize_and_nul() {
    let dir = TestDir::new("raw-validation");
    let (store, _embedder) = store_with_session(&dir).await;

    for (label, raw) in [
        ("empty", ""),
        ("blank", "   \n\t "),
        ("nul", "a\u{0}b"),
        ("oversize", "x".repeat(MAX_RAW_CONTEXT_BYTES + 1).as_str()),
    ] {
        let error = store
            .insert_raw_context(PROJECT, SESSION, AGENT, raw)
            .await
            .expect_err(&format!("{label} must be rejected"));
        match error {
            mem_episodic::EpisodicError::Validation { field, .. } => {
                assert_eq!(field, "raw_context", "{label}")
            }
            other => panic!("{label}: expected a validation error, got {other:?}"),
        }
    }
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("read")
            .is_none(),
        "nothing was written"
    );

    // Exactly at the bound is accepted.
    let exact = "y".repeat(MAX_RAW_CONTEXT_BYTES);
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, &exact)
        .await
        .expect("the bound itself is inclusive");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_unknown_scope_reads_none() {
    let dir = TestDir::new("raw-unknown");
    let (store, _embedder) = store_with_session(&dir).await;
    assert!(
        store
            .raw_context_for_session(PROJECT, "no-such-session", AGENT)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        store
            .segments_for_session(PROJECT, "no-such-session", AGENT)
            .await
            .expect("segments")
            .is_empty()
    );
    assert!(
        store
            .agent_role_for(PROJECT, "no-such-session", AGENT)
            .await
            .expect("role")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_survives_a_session_row_delete() {
    let dir = TestDir::new("raw-survives-delete");
    let (store, _embedder) = store_with_session(&dir).await;
    let raw = sample();
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, &raw)
        .await
        .expect("insert");

    let db = store.ready().await.expect("ready");
    let id = mem_store::db::query_one(
        db.as_ref(),
        "SELECT id FROM agent_sessions WHERE project_id = ? AND session_id = ? AND agent_id = ?",
        vec![
            mem_store::db::Value::Text(PROJECT.to_string()),
            mem_store::db::Value::Text(SESSION.to_string()),
            mem_store::db::Value::Text(AGENT.to_string()),
        ],
    )
    .await
    .expect("select id")
    .expect("row")
    .text("id")
    .expect("id");
    assert!(store.delete_session(&id).await.expect("delete session"));

    let stored = store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("read after row delete")
        .expect("raw context survives a ROW delete");
    assert_eq!(stored.raw_context, raw);
    assert!(
        !store
            .segments_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("segments")
            .is_empty()
    );
}

/// The ADR-0010 D-1 invariant, end to end: a real `Consolidator::run` (real
/// store batch writer) hard-deletes the session row and must not touch the
/// raw context or its segments.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_survives_a_full_consolidation_run() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            PROJECT,
            SESSION,
            AGENT,
            "the source summary",
            &["a durable decision"],
        )
        .await;

    let raw = sample();
    fixture
        .store
        .insert_raw_context(PROJECT, SESSION, AGENT, &raw)
        .await
        .expect("insert raw context");
    let before = fixture
        .store
        .segments_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("segments before");

    let report = fixture
        .real_consolidator(30)
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect("consolidation run");
    assert_eq!(
        report.sessions_consolidated, 1,
        "the session row was deleted"
    );
    assert_eq!(
        fixture
            .store
            .count_sessions(Some(PROJECT))
            .await
            .expect("count"),
        0,
        "no session row remains"
    );

    let stored = fixture
        .store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("read after consolidation")
        .expect("raw context SURVIVES the full consolidation run");
    assert_eq!(stored.raw_context, raw, "byte-equal");
    let after = fixture
        .store
        .segments_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("segments after");
    assert_eq!(
        after, before,
        "segments are byte-identical and byte-ordered"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_agent_role_defaults_to_main_and_is_writable_per_scope_triple() {
    let dir = TestDir::new("raw-agent-role");
    let store = Arc::new(EpisodicStore::new(dir.to_path_buf(), dir.store_config()));
    let embedder = Arc::new(FakeEmbedder::with_vector(vec![0.1; 8]));
    for (session, agent) in [(SESSION, AGENT), (SESSION, "agent-b"), ("session-b", AGENT)] {
        store
            .insert_session(
                NewAgentSession {
                    project_id: PROJECT.to_string(),
                    session_id: session.to_string(),
                    agent_id: agent.to_string(),
                    summary: "s".to_string(),
                    ..Default::default()
                },
                embedder.as_ref(),
            )
            .await
            .expect("insert");
    }

    assert_eq!(
        store
            .agent_role_for(PROJECT, SESSION, AGENT)
            .await
            .expect("role")
            .as_deref(),
        Some("main"),
        "the v6 default"
    );

    assert!(
        store
            .set_session_agent_role(PROJECT, SESSION, AGENT, "reviewer")
            .await
            .expect("set role")
    );
    assert_eq!(
        store
            .agent_role_for(PROJECT, SESSION, AGENT)
            .await
            .expect("role")
            .as_deref(),
        Some("reviewer")
    );
    // Only the named triple moved.
    assert_eq!(
        store
            .agent_role_for(PROJECT, SESSION, "agent-b")
            .await
            .expect("role")
            .as_deref(),
        Some("main")
    );
    assert_eq!(
        store
            .agent_role_for(PROJECT, "session-b", AGENT)
            .await
            .expect("role")
            .as_deref(),
        Some("main")
    );

    // An unknown role value is rejected before any write.
    let error = store
        .set_session_agent_role(PROJECT, SESSION, AGENT, "boss")
        .await
        .expect_err("an unknown role is rejected");
    assert!(matches!(
        error,
        mem_episodic::EpisodicError::Validation { .. }
    ));
    assert_eq!(
        store
            .agent_role_for(PROJECT, SESSION, AGENT)
            .await
            .expect("role")
            .as_deref(),
        Some("reviewer"),
        "the rejected write changed nothing"
    );
}
