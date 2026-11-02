//! The project roster: the union of live `agent_sessions` and
//! `episodic_milestones` (issue 116, D-6).
//!
//! Neither source alone is a roster. Consolidation **hard**-deletes its source
//! sessions (`DELETE_SESSION_SCOPED`), so a sessions-only roster loses every
//! well-consolidated project; milestones alone have no history for a project
//! that was never consolidated. The projection therefore unions the two and
//! counts each side separately.
//!
//! Everything runs against a real turso database in a temp dir; the rows are
//! seeded with raw statements so the test owns the exact fixture (no
//! production write path is exercised here).

mod common;

use common::TestDir;
use mem_episodic::store::EpisodicStore;
use mem_store::db::Value;
use mem_store::vector::vector_to_text;

/// Inserts one `agent_sessions` row (8 dims — the hermetic test config's
/// dimension).
async fn raw_insert_session(
    store: &EpisodicStore,
    id: &str,
    project_id: &str,
    created_at: i64,
    deleted_at: Option<i64>,
) {
    let db = store.ready().await.expect("ready");
    db.execute(
        "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, summary, metadata, embedding, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, vector32(?), ?, ?, ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Text(format!("session-{id}")),
            Value::Text("agent-a".to_string()),
            Value::Text("summary".to_string()),
            Value::Text("{}".to_string()),
            Value::Text(vector_to_text(&[0.5_f32; 8]).expect("vector serializes")),
            Value::Integer(created_at),
            Value::Integer(created_at),
            deleted_at.map(Value::Integer).unwrap_or(Value::Null),
        ],
    )
    .await
    .expect("insert session row");
}

/// Inserts one `episodic_milestones` row.
async fn raw_insert_milestone(store: &EpisodicStore, id: &str, project_id: &str, created_at: i64) {
    let db = store.ready().await.expect("ready");
    db.execute(
        "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text(project_id.to_string()),
            Value::Text("agent-a".to_string()),
            Value::Text("merged summary".to_string()),
            Value::Text("[]".to_string()),
            Value::Integer(0),
            Value::Integer(created_at),
            Value::Integer(created_at),
            Value::Text(vector_to_text(&[0.25_f32; 8]).expect("vector serializes")),
            Value::Integer(created_at),
        ],
    )
    .await
    .expect("insert milestone row");
}

/// The `(project_id, session_count, milestone_count, last_activity_at)` tuples,
/// in the store's documented order.
fn tuples(rows: &[mem_episodic::store::ProjectRosterRow]) -> Vec<(String, i64, i64, i64)> {
    rows.iter()
        .map(|row| {
            (
                row.project_id.clone(),
                row.session_count,
                row.milestone_count,
                row.last_activity_at,
            )
        })
        .collect()
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_project_roster_unions_live_sessions_and_milestones() {
    let dir = TestDir::new("project-roster-union");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());

    // Project A: sessions only. Project B: sessions and milestones.
    raw_insert_session(&store, "a1", "/proj/a", 100, None).await;
    raw_insert_session(&store, "b1", "/proj/b", 200, None).await;
    raw_insert_milestone(&store, "mb1", "/proj/b", 250).await;
    // Project C: one milestone, and NOT ONE session row left (consolidation
    // hard-deletes its sources). A sessions-only roster — or an INNER JOIN —
    // drops C entirely.
    raw_insert_milestone(&store, "mc1", "/proj/c", 300).await;

    let rows = store.project_roster(100).await.expect("the roster reads");
    assert_eq!(
        tuples(&rows),
        vec![
            ("/proj/c".to_string(), 0, 1, 300),
            // B still holds a live session, so its stamp is that session's
            // (200); the milestone stamp (250) is the fallback a fully
            // consolidated project uses — which is exactly C.
            ("/proj/b".to_string(), 1, 1, 200),
            ("/proj/a".to_string(), 1, 0, 100),
        ]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_project_roster_counts_live_sessions_only() {
    let dir = TestDir::new("project-roster-soft-delete");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());

    for (id, created_at) in [("s1", 10), ("s2", 20), ("s3", 30)] {
        raw_insert_session(&store, id, "/proj/live", created_at, None).await;
    }
    // Soft-deleted: still a row, not a live session. It must neither be
    // counted nor set the project's stamp.
    raw_insert_session(&store, "s4", "/proj/live", 40, Some(50)).await;

    let rows = store.project_roster(100).await.expect("the roster reads");
    assert_eq!(
        tuples(&rows),
        vec![("/proj/live".to_string(), 3, 0, 30)],
        "only live sessions are counted and only live stamps set last_activity"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_project_roster_orders_by_last_activity_then_id() {
    let dir = TestDir::new("project-roster-order");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());

    // A last activity 100, B 300: newest first.
    raw_insert_session(&store, "a1", "/proj/a", 100, None).await;
    raw_insert_session(&store, "b1", "/proj/b", 300, None).await;
    // Two projects TIE at 250: the secondary key is `project_id` asc, so
    // "/proj/t-b" must follow "/proj/t-a". `ORDER BY last_activity_at` alone
    // cannot produce this order.
    raw_insert_session(&store, "t1", "/proj/t-b", 250, None).await;
    raw_insert_session(&store, "t2", "/proj/t-a", 250, None).await;

    let rows = store.project_roster(100).await.expect("the roster reads");
    let ids: Vec<String> = rows.iter().map(|row| row.project_id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            "/proj/b".to_string(),
            "/proj/t-a".to_string(),
            "/proj/t-b".to_string(),
            "/proj/a".to_string(),
        ],
        "last_activity desc, then project_id asc"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_project_roster_respects_the_limit() {
    let dir = TestDir::new("project-roster-limit");
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());

    raw_insert_session(&store, "p1", "/proj/1", 100, None).await;
    raw_insert_session(&store, "p2", "/proj/2", 200, None).await;
    raw_insert_session(&store, "p3", "/proj/3", 300, None).await;

    let rows = store.project_roster(2).await.expect("the roster reads");
    assert_eq!(
        tuples(&rows),
        vec![
            ("/proj/3".to_string(), 1, 0, 300),
            ("/proj/2".to_string(), 1, 0, 200),
        ],
        "the limit keeps the head of the documented order"
    );

    assert_eq!(
        store.project_roster(100).await.expect("reads").len(),
        3,
        "a generous limit returns every project"
    );
}
