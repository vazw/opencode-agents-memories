//! W7.1 — the episodic erasure path (D6): `SessionScope`, the pre-scan and
//! `EpisodicStore::purge_session`.
//!
//! The module header of `mem_episodic::purge` carries the authoritative
//! sentence: consolidation and every lifecycle stage never destroy raw
//! context; only an explicit, owner-initiated purge does.

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, TestDir};
use mem_episodic::EpisodicError;
use mem_episodic::inbox::{CaptureInbox, InboxNewEntry};
use mem_episodic::milestone::{Contributor, encode_contributors};
use mem_episodic::purge::{EpisodicPurgeReport, SessionScope};
use mem_episodic::record::NewAgentSession;
use mem_episodic::segments::MAX_SEGMENT_BYTES;
use mem_episodic::store::EpisodicStore;
use mem_store::db::{BatchMode, Statement, Value, query_one};
use mem_store::episodic_sql as sql;

const PROJECT: &str = "project-a";
const SESSION: &str = "session-a";
const AGENT: &str = "agent-a";

fn scope() -> SessionScope {
    SessionScope::new(PROJECT, SESSION, AGENT).expect("valid scope")
}

async fn store(dir: &TestDir) -> Arc<EpisodicStore> {
    let store = Arc::new(EpisodicStore::new(dir.to_path_buf(), dir.store_config()));
    store.ready().await.expect("ready");
    store
}

async fn insert_session(
    store: &Arc<EpisodicStore>,
    project: &str,
    session: &str,
    agent: &str,
) -> String {
    let embedder = FakeEmbedder::with_vector(vec![0.1; 8]);
    store
        .insert_session(
            NewAgentSession {
                project_id: project.to_string(),
                session_id: session.to_string(),
                agent_id: agent.to_string(),
                summary: "the summary".to_string(),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert session")
}

async fn insert_child(store: &Arc<EpisodicStore>, parent_row_id: &str) -> String {
    let embedder = FakeEmbedder::with_vector(vec![0.1; 8]);
    store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT.to_string(),
                session_id: "child-session".to_string(),
                agent_id: AGENT.to_string(),
                summary: "the child".to_string(),
                parent_session_id: Some(parent_row_id.to_string()),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert child")
}

#[derive(Clone)]
struct MilestoneSeed {
    id: &'static str,
    project: &'static str,
    agent: &'static str,
    contributors: Vec<(&'static str, &'static str, &'static str)>,
}

async fn insert_milestone(store: &Arc<EpisodicStore>, seed: MilestoneSeed) {
    let contributors: Vec<Contributor> = seed
        .contributors
        .iter()
        .map(|(row_id, session_id, agent_id)| Contributor {
            session_row_id: (*row_id).to_string(),
            session_id: (*session_id).to_string(),
            parent_session_id: None,
            agent_id: (*agent_id).to_string(),
            agent_name: (*agent_id).to_string(),
            created_at: 1,
            summary: "retained source text".to_string(),
        })
        .collect();
    let json = encode_contributors(&contributors).expect("contributors");
    let db = store.ready().await.expect("ready");
    db.batch(
        vec![Statement::new(
            sql::INSERT_MILESTONE,
            vec![
                Value::Text(seed.id.to_string()),
                Value::Text(seed.project.to_string()),
                Value::Text(seed.agent.to_string()),
                Value::Text("a merged milestone summary".to_string()),
                Value::Text(json),
                Value::Integer(contributors.len() as i64),
                Value::Integer(1),
                Value::Integer(1),
                Value::Text("[0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1]".to_string()),
                Value::Integer(1),
            ],
        )],
        BatchMode::Write,
    )
    .await
    .expect("insert milestone");
}

async fn enqueue_inbox(
    store: &Arc<EpisodicStore>,
    capture_id: &str,
    project: &str,
    session: &str,
    agent: &str,
) {
    CaptureInbox::new(store.clone())
        .enqueue(InboxNewEntry {
            capture_id: capture_id.to_string(),
            project_id: project.to_string(),
            session_id: session.to_string(),
            agent_id: agent.to_string(),
            payload: mem_store::serde_json::json!({ "summary": "queued" }),
        })
        .await
        .expect("enqueue");
}

async fn row_snapshot(store: &Arc<EpisodicStore>) -> Vec<i64> {
    let db = store.ready().await.expect("ready");
    let mut counts = Vec::new();
    for table in [
        "agent_sessions",
        "episodic_raw_context",
        "episodic_segments",
        "capture_inbox",
        "episodic_milestones",
    ] {
        let row = query_one(
            db.as_ref(),
            &format!("SELECT COUNT(*) AS count FROM {table}"),
            Vec::new(),
        )
        .await
        .expect("count")
        .expect("row");
        counts.push(row.i64("count").unwrap_or(0));
    }
    counts
}

#[test]
fn test_session_scope_rejects_empty_or_blank_parts() {
    for (field, project, session, agent) in [
        ("project_id", "", "s", "a"),
        ("project_id", "   ", "s", "a"),
        ("project_id", "p\u{0}x", "s", "a"),
        ("session_id", "p", "", "a"),
        ("session_id", "p", " \t ", "a"),
        ("session_id", "p", "s\u{0}", "a"),
        ("agent_id", "p", "s", ""),
        ("agent_id", "p", "s", "  "),
        ("agent_id", "p", "s", "a\u{0}b"),
    ] {
        let error =
            SessionScope::new(project, session, agent).expect_err("an invalid scope is rejected");
        match error {
            EpisodicError::Validation {
                field: rejected, ..
            } => {
                assert_eq!(rejected, field, "{project:?}/{session:?}/{agent:?}")
            }
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    // Trimming is applied to valid values.
    let scope = SessionScope::new("  project-a ", " session-a ", " agent-a ").expect("valid");
    assert_eq!(scope.project_id, "project-a");
    assert_eq!(scope.session_id, "session-a");
    assert_eq!(scope.agent_id, "agent-a");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_removes_raw_context_segments_session_and_inbox_for_the_scope() {
    let dir = TestDir::new("w71-purge-full");
    let store = store(&dir).await;
    insert_session(&store, PROJECT, SESSION, AGENT).await;
    let raw = format!(
        "{}\n\n{}",
        "a".repeat(MAX_SEGMENT_BYTES),
        "b".repeat(MAX_SEGMENT_BYTES)
    );
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, &raw)
        .await
        .expect("raw");
    let segment_count = store
        .segments_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("segments")
        .len() as u64;
    enqueue_inbox(&store, "cap-1", PROJECT, SESSION, AGENT).await;

    let report = store.purge_session(&scope()).await.expect("purge");
    assert_eq!(
        report,
        EpisodicPurgeReport {
            session_rows: 1,
            raw_context: 1,
            segments: segment_count,
            inbox_rows: 1,
            milestones_scrubbed: 0,
        }
    );
    assert!(!report.is_empty());

    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_none()
    );
    assert!(
        store
            .segments_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("segments")
            .is_empty()
    );
    assert_eq!(store.count_sessions(Some(PROJECT)).await.expect("count"), 0);
    assert_eq!(
        CaptureInbox::new(store.clone())
            .queued_count()
            .await
            .expect("queued"),
        0
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_removes_only_the_named_scope() {
    let dir = TestDir::new("w71-purge-scope");
    let store = store(&dir).await;
    insert_session(&store, PROJECT, SESSION, AGENT).await;
    // Same session id under a second project and a second agent.
    insert_session(&store, "project-b", SESSION, AGENT).await;
    insert_session(&store, PROJECT, SESSION, "agent-b").await;
    // A sibling session in the same project.
    insert_session(&store, PROJECT, "session-sibling", AGENT).await;
    for (project, session, agent) in [
        (PROJECT, SESSION, AGENT),
        ("project-b", SESSION, AGENT),
        (PROJECT, SESSION, "agent-b"),
        (PROJECT, "session-sibling", AGENT),
    ] {
        store
            .insert_raw_context(
                project,
                session,
                agent,
                &format!("raw {project}/{session}/{agent}"),
            )
            .await
            .expect("raw");
    }

    let report = store.purge_session(&scope()).await.expect("purge");
    assert_eq!(report.session_rows, 1);
    assert_eq!(report.raw_context, 1);

    for (project, session, agent) in [
        ("project-b", SESSION, AGENT),
        (PROJECT, SESSION, "agent-b"),
        (PROJECT, "session-sibling", AGENT),
    ] {
        assert!(
            store
                .raw_context_for_session(project, session, agent)
                .await
                .expect("raw")
                .is_some(),
            "{project}/{session}/{agent} is untouched"
        );
        assert!(
            store
                .agent_role_for(project, session, agent)
                .await
                .expect("role")
                .is_some(),
            "{project}/{session}/{agent} row is untouched"
        );
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_is_idempotent_and_writes_nothing_the_second_time() {
    let dir = TestDir::new("w71-purge-idempotent");
    let store = store(&dir).await;
    insert_session(&store, PROJECT, SESSION, AGENT).await;
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, "raw bytes")
        .await
        .expect("raw");
    enqueue_inbox(&store, "cap-1", PROJECT, SESSION, AGENT).await;

    store.purge_session(&scope()).await.expect("first purge");
    let snapshot = row_snapshot(&store).await;
    let second = store.purge_session(&scope()).await.expect("second purge");
    assert_eq!(second, EpisodicPurgeReport::default());
    assert!(second.is_empty());
    assert_eq!(row_snapshot(&store).await, snapshot, "nothing was written");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_of_an_unknown_scope_returns_zeros() {
    let dir = TestDir::new("w71-purge-unknown");
    let store = store(&dir).await;
    let report = store
        .purge_session(&SessionScope::new(PROJECT, "no-such-session", AGENT).expect("scope"))
        .await
        .expect("purge");
    assert_eq!(report, EpisodicPurgeReport::default());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_scrubs_the_triple_contributor_and_keeps_the_co_contributor_summary() {
    let dir = TestDir::new("w71-purge-milestones");
    let store = store(&dir).await;
    insert_session(&store, PROJECT, SESSION, AGENT).await;
    insert_session(&store, PROJECT, "session-other", AGENT).await;
    // The other contributor's session row stays alive in this fixture.
    insert_session(&store, PROJECT, SESSION, "agent-b").await;

    // One single-contributor milestone of the triple, one mixed milestone,
    // one milestone of another agent.
    insert_milestone(
        &store,
        MilestoneSeed {
            id: "ms-single",
            project: PROJECT,
            agent: AGENT,
            contributors: vec![("row-a", SESSION, AGENT)],
        },
    )
    .await;
    insert_milestone(
        &store,
        MilestoneSeed {
            id: "ms-mixed",
            project: PROJECT,
            agent: AGENT,
            contributors: vec![("row-a", SESSION, AGENT), ("row-b", "session-other", AGENT)],
        },
    )
    .await;
    insert_milestone(
        &store,
        MilestoneSeed {
            id: "ms-other-agent",
            project: PROJECT,
            agent: "agent-b",
            contributors: vec![("row-c", SESSION, "agent-b")],
        },
    )
    .await;

    let ids = store
        .milestone_ids_for_session(&scope())
        .await
        .expect("pre-scan");
    assert_eq!(ids, vec!["ms-single".to_string(), "ms-mixed".to_string()]);

    // The derived recap columns are captured before the purge (D6: the scrub
    // is text-only — the merged summary, its embedding, source_count and the
    // window survive untouched).
    let db = store.ready().await.expect("ready");
    let before = query_one(
        db.as_ref(),
        "SELECT summary, length(embedding) AS embedding_len, source_count, \
         window_start, window_end FROM episodic_milestones WHERE id = ?",
        vec![Value::Text("ms-mixed".to_string())],
    )
    .await
    .expect("select mixed")
    .expect("ms-mixed exists");

    let report = store.purge_session(&scope()).await.expect("purge");
    assert_eq!(report.milestones_scrubbed, 2);

    // The rows SURVIVE: the scrub empties the triple's own summary text in
    // place and keeps every attribution column.
    let milestones = store.list_milestones(PROJECT, 10).await.expect("list");
    assert_eq!(milestones.len(), 3, "no milestone row is deleted");
    let mixed = milestones
        .iter()
        .find(|milestone| milestone.id == "ms-mixed")
        .expect("ms-mixed survives");
    let triple = mixed
        .contributors
        .iter()
        .find(|contributor| contributor.session_id == SESSION)
        .expect("the triple's contributor entry survives");
    assert_eq!(triple.summary, "", "the purged contributor's text is gone");
    assert_eq!(triple.agent_id, AGENT, "the attribution survives");
    let co_contributor = mixed
        .contributors
        .iter()
        .find(|contributor| contributor.session_id == "session-other")
        .expect("the co-contributor survives");
    assert_eq!(
        co_contributor.summary, "retained source text",
        "a co-contributor's text is never rewritten"
    );
    let single = milestones
        .iter()
        .find(|milestone| milestone.id == "ms-single")
        .expect("ms-single survives");
    assert_eq!(single.contributors[0].summary, "");
    let other_agent = milestones
        .iter()
        .find(|milestone| milestone.id == "ms-other-agent")
        .expect("ms-other-agent survives");
    assert_eq!(
        other_agent.contributors[0].summary, "retained source text",
        "another agent's milestone is untouched"
    );

    let after = query_one(
        db.as_ref(),
        "SELECT summary, length(embedding) AS embedding_len, source_count, \
         window_start, window_end FROM episodic_milestones WHERE id = ?",
        vec![Value::Text("ms-mixed".to_string())],
    )
    .await
    .expect("select mixed")
    .expect("ms-mixed exists");
    for column in [
        "summary",
        "embedding_len",
        "source_count",
        "window_start",
        "window_end",
    ] {
        assert_eq!(
            before.i64(column),
            after.i64(column),
            "{column} is not recomputed"
        );
        assert_eq!(before.text(column), after.text(column), "{column} survives");
    }

    // The other contributor's session row is untouched.
    assert_eq!(
        store
            .agent_role_for(PROJECT, "session-other", AGENT)
            .await
            .expect("role")
            .as_deref(),
        Some("main")
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_scrub_converges_on_the_second_call() {
    let dir = TestDir::new("w71-purge-scrub-converge");
    let store = store(&dir).await;
    insert_session(&store, PROJECT, SESSION, AGENT).await;
    insert_milestone(
        &store,
        MilestoneSeed {
            id: "ms-mixed",
            project: PROJECT,
            agent: AGENT,
            contributors: vec![("row-a", SESSION, AGENT), ("row-b", "session-other", AGENT)],
        },
    )
    .await;

    let first = store.purge_session(&scope()).await.expect("first purge");
    assert_eq!(first.milestones_scrubbed, 1);
    let db = store.ready().await.expect("ready");
    let scrubbed = query_one(
        db.as_ref(),
        "SELECT contributors FROM episodic_milestones WHERE id = ?",
        vec![Value::Text("ms-mixed".to_string())],
    )
    .await
    .expect("select")
    .expect("row survives")
    .text("contributors")
    .expect("contributors text");

    let second = store.purge_session(&scope()).await.expect("second purge");
    assert!(second.is_empty(), "a second purge writes nothing");
    assert_eq!(second.milestones_scrubbed, 0);
    let again = query_one(
        db.as_ref(),
        "SELECT contributors FROM episodic_milestones WHERE id = ?",
        vec![Value::Text("ms-mixed".to_string())],
    )
    .await
    .expect("select")
    .expect("row survives")
    .text("contributors")
    .expect("contributors text");
    assert_eq!(again, scrubbed, "the contributors text is byte-identical");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_refuses_a_milestone_with_unreadable_contributors() {
    let dir = TestDir::new("w71-purge-unreadable");
    let store = store(&dir).await;
    insert_session(&store, PROJECT, SESSION, AGENT).await;
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, "raw bytes")
        .await
        .expect("raw");
    insert_milestone(
        &store,
        MilestoneSeed {
            id: "ms-corrupt",
            project: PROJECT,
            agent: AGENT,
            // A valid contributor set on insert; the UPDATE below corrupts it.
            contributors: vec![("row-b", "session-other", AGENT)],
        },
    )
    .await;
    let db = store.ready().await.expect("ready");
    db.execute(
        "UPDATE episodic_milestones SET contributors = ? WHERE id = ?",
        vec![
            Value::Text("not json".to_string()),
            Value::Text("ms-corrupt".to_string()),
        ],
    )
    .await
    .expect("corrupt contributors");

    let error = store
        .purge_session(&scope())
        .await
        .expect_err("an undecodable contributors row refuses the purge");
    assert!(
        matches!(error, EpisodicError::Json { .. }),
        "expected a JSON error, got {error:?}"
    );
    // Fail-closed: nothing of the triple was erased.
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "the raw context survives a refused purge"
    );
    assert_eq!(
        store.count_sessions(Some(PROJECT)).await.expect("count"),
        1,
        "the session row survives a refused purge"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_marks_children_orphaned_before_deleting_the_parent() {
    let dir = TestDir::new("w71-purge-children");
    let store = store(&dir).await;
    let parent_id = insert_session(&store, PROJECT, SESSION, AGENT).await;
    let child_id = insert_child(&store, &parent_id).await;

    store.purge_session(&scope()).await.expect("purge");

    let db = store.ready().await.expect("ready");
    let child = query_one(
        db.as_ref(),
        "SELECT orphaned, parent_session_id FROM agent_sessions WHERE id = ?",
        vec![Value::Text(child_id)],
    )
    .await
    .expect("select child")
    .expect("the child survives");
    assert_eq!(child.i64("orphaned"), Some(1), "the child is orphaned");
    assert!(child.is_null("parent_session_id"), "the link is cleared");
}
