//! W4.1 lanes A + C — `EpisodicStore` insert/get/list/delete/search over a
//! real turso database (net-new, R10: no upstream oracle).
//!
//! Step 4 covers insert/get, id minting, validation-before-write, embedder
//! failure containment and the D-4 exact-embedded-string pin. Later tracer
//! steps add list/delete/search and the scope-lock-in cases.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{FakeEmbedder, TestDir};
use mem_embed::embedder::EmbedTask;
use mem_embed::prefix::{PrefixOptions, apply_task_prefix};
use mem_episodic::error::EpisodicError;
use mem_episodic::record::{Decision, NewAgentSession, WorkItem, WorkKind};
use mem_episodic::store::{EPISODIC_DB_NAME, EpisodicStore, SessionSearchHit};
use mem_store::db::{
    BoxFuture, ConnectFactory, ConnectOptions, DbHandle, Value, query_one, turso_connect_factory,
};
use mem_store::error::StoreError;

const DIMS: usize = 8;

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; DIMS];
    vector[index] = 1.0;
    vector
}

fn new_session(project: &str, session: &str, agent: &str, summary: &str) -> NewAgentSession {
    NewAgentSession {
        project_id: project.to_string(),
        session_id: session.to_string(),
        agent_id: agent.to_string(),
        summary: summary.to_string(),
        ..Default::default()
    }
}

fn fixture(prefix: &str) -> (TestDir, EpisodicStore) {
    let dir = TestDir::new(prefix);
    let store = EpisodicStore::new(dir.to_path_buf(), dir.store_config());
    (dir, store)
}

fn assert_validation_field(error: EpisodicError, expected: &str) {
    match error {
        EpisodicError::Validation { field, .. } => assert_eq!(field, expected),
        other => panic!("expected Validation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Step 4 — insert/get tracer
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_and_get_round_trip() {
    let (_dir, store) = fixture("roundtrip");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let mut new = new_session("project-a", "session-1", "agent-1", "did the thing");
    new.files_touched = vec!["src/lib.rs".to_string()];
    new.completed_work = vec![WorkItem {
        kind: WorkKind::Feature,
        description: "wrote the store".to_string(),
    }];
    new.decisions = vec![Decision {
        description: "dedicated db".to_string(),
    }];
    new.open_threads = vec!["wire capture in W4.2".to_string()];
    new.git_branch = Some("main".to_string());
    new.git_commit = Some("abc1234".to_string());

    let id = store
        .insert_session(new, &embedder)
        .await
        .expect("insert session");
    assert!(!id.is_empty());

    let session = store
        .get_session(&id)
        .await
        .expect("get session")
        .expect("row present");
    assert_eq!(session.id, id);
    assert_eq!(session.project_id, "project-a");
    assert_eq!(session.session_id, "session-1");
    assert_eq!(session.agent_id, "agent-1");
    assert_eq!(session.parent_session_id, None);
    assert_eq!(session.summary, "did the thing");
    assert_eq!(session.metadata.agent_name, "agent-1");
    assert_eq!(session.metadata.git_branch.as_deref(), Some("main"));
    assert_eq!(session.metadata.git_commit.as_deref(), Some("abc1234"));
    assert_eq!(
        session.metadata.files_touched,
        vec!["src/lib.rs".to_string()]
    );
    assert_eq!(session.metadata.completed_work.len(), 1);
    assert_eq!(session.metadata.decisions.len(), 1);
    assert_eq!(session.metadata.open_threads.len(), 1);
    assert_eq!(session.embedding, unit_vector(0));
    assert!(session.created_at > 0);
    assert_eq!(session.created_at, session.updated_at);
    assert_eq!(session.metadata.timestamp, session.created_at);

    // Unknown row ids are a clean `None`, never an error.
    assert!(
        store
            .get_session("missing-id")
            .await
            .expect("get")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_fails_on_invalid_record_without_writing() {
    let (_dir, store) = fixture("insert-invalid");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let mut invalid = new_session("", "session-1", "agent-1", "s");
    invalid.project_id = String::new();
    let error = store
        .insert_session(invalid, &embedder)
        .await
        .expect_err("invalid record rejected");
    assert_validation_field(error, "project_id");
    assert_eq!(store.count_sessions(None).await.expect("count"), 0);
    assert!(
        embedder.calls().is_empty(),
        "validation must run before embedding"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_fails_on_embedder_error_without_writing() {
    let (_dir, store) = fixture("insert-embed-error");
    let embedder = FakeEmbedder::failing();

    let error = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "s"),
            &embedder,
        )
        .await
        .expect_err("embedder failure surfaces");
    assert!(matches!(error, EpisodicError::Embed(_)), "got {error:?}");
    assert_eq!(store.count_sessions(None).await.expect("count"), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_fails_on_dimension_mismatch_without_writing() {
    let (_dir, store) = fixture("insert-dim-mismatch");
    let embedder = FakeEmbedder::with_vector(vec![1.0_f32; DIMS - 2]);

    let error = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "s"),
            &embedder,
        )
        .await
        .expect_err("dimension mismatch rejected");
    assert_validation_field(error, "embedding");
    assert_eq!(store.count_sessions(None).await.expect("count"), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_mints_uuid7_id() {
    let (_dir, store) = fixture("insert-uuid7");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let id = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "s"),
            &embedder,
        )
        .await
        .expect("insert");

    assert_eq!(id.len(), 36, "id: {id}");
    let bytes = id.as_bytes();
    assert_eq!(bytes[8], b'-');
    assert_eq!(bytes[13], b'-');
    assert_eq!(bytes[18], b'-');
    assert_eq!(bytes[23], b'-');
    assert_eq!(bytes[14], b'7', "version nibble in {id}");
    assert!(matches!(bytes[19], b'8'..=b'b'), "variant nibble in {id}");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_uuid7_ids_are_time_ordered() {
    let (_dir, store) = fixture("insert-uuid7-order");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let first = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "first"),
            &embedder,
        )
        .await
        .expect("first insert");
    // UUIDv7 time ordering only becomes observable across a millisecond
    // boundary — deliberately crossed, never flake-tolerated.
    std::thread::sleep(Duration::from_millis(5));
    let second = store
        .insert_session(
            new_session("project-a", "session-2", "agent-1", "second"),
            &embedder,
        )
        .await
        .expect("second insert");

    assert!(
        second > first,
        "UUIDv7 ids must sort by creation time: {first} vs {second}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_with_deterministic_id_injection() {
    let (_dir, store) = fixture("insert-id-factory");
    let store = store.with_id_factory(Arc::new(|| "session-fixed-id".to_string()));
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let id = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "s"),
            &embedder,
        )
        .await
        .expect("insert");
    assert_eq!(id, "session-fixed-id");
    let session = store
        .get_session("session-fixed-id")
        .await
        .expect("get")
        .expect("row");
    assert_eq!(session.id, "session-fixed-id");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_embeds_summary() {
    let (_dir, store) = fixture("insert-embeds-summary");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let id = store
        .insert_session(
            new_session("project-a", "session-7", "agent-7", "a searchable summary"),
            &embedder,
        )
        .await
        .expect("insert");
    assert!(id.len() == 36);

    let calls = embedder.calls();
    assert_eq!(calls.len(), 1, "one embed per insert");
    assert!(
        calls[0].contains("a searchable summary"),
        "embedded text: {}",
        calls[0]
    );
    assert!(
        calls[0].contains("Session session-7 by agent-7"),
        "embedded text: {}",
        calls[0]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_embeds_exact_embedded_string() {
    let (_dir, store) = fixture("insert-exact-string");
    let embedder = FakeEmbedder::with_vectors(vec![unit_vector(0), unit_vector(1)]);

    let mut complete = new_session("project-a", "session-1", "agent-1", "summary text");
    complete.git_branch = Some("main".to_string());
    complete.completed_work = vec![
        WorkItem {
            kind: WorkKind::Feature,
            description: "added store".to_string(),
        },
        WorkItem {
            kind: WorkKind::BugFix,
            description: "fixed bug".to_string(),
        },
    ];
    complete.decisions = vec![Decision {
        description: "use dedicated db".to_string(),
    }];

    store
        .insert_session(complete, &embedder)
        .await
        .expect("insert complete session");
    let expected_branch = "Session session-1 by agent-1 on main: summary text. Completed: feature: added store; bugFix: fixed bug. Decisions: use dedicated db.";
    assert_eq!(
        embedder.last_call().as_deref(),
        Some(
            apply_task_prefix(
                expected_branch,
                EmbedTask::Document,
                &PrefixOptions::default()
            )
            .as_str()
        )
    );

    // Branch `None` pins the "unknown branch" literal.
    store
        .insert_session(
            new_session("project-a", "session-2", "agent-1", "no branch"),
            &embedder,
        )
        .await
        .expect("insert branch-less session");
    let expected_unknown =
        "Session session-2 by agent-1 on unknown branch: no branch. Completed: . Decisions: .";
    assert_eq!(
        embedder.last_call().as_deref(),
        Some(
            apply_task_prefix(
                expected_unknown,
                EmbedTask::Document,
                &PrefixOptions::default()
            )
            .as_str()
        )
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_dimension_mismatch_is_validation_error() {
    let (_dir, store) = fixture("dimension-mismatch-variant");
    let embedder = FakeEmbedder::with_vector(vec![0.5_f32; DIMS + 3]);

    let error = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "s"),
            &embedder,
        )
        .await
        .expect_err("rejected");
    match error {
        EpisodicError::Validation { field, message } => {
            assert_eq!(field, "embedding");
            assert!(message.contains("8"), "message: {message}");
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_duplicate_scope_triple_rejected_without_overwrite() {
    let (_dir, store) = fixture("duplicate-triple");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let first_id = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "original"),
            &embedder,
        )
        .await
        .expect("first insert");

    let error = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "clobber attempt"),
            &embedder,
        )
        .await
        .expect_err("duplicate triple rejected");
    assert!(matches!(error, EpisodicError::Conflict), "got {error:?}");

    assert_eq!(store.count_sessions(None).await.expect("count"), 1);
    let stored = store
        .get_session(&first_id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(stored.summary, "original");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_insert_and_get_round_trip_preserves_v2_defaults() {
    let (_dir, store) = fixture("v2-defaults-roundtrip");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let id = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "s"),
            &embedder,
        )
        .await
        .expect("insert");

    let session = store.get_session(&id).await.expect("get").expect("row");
    assert!(session.deleted_at.is_none(), "inserted rows are live");
    assert!(!session.orphaned, "inserted rows are not orphaned");

    // The raw columns carry the same defaults (v2 is additive).
    let db = store.ready().await.expect("ready");
    let row = query_one(
        db.as_ref(),
        "SELECT deleted_at, orphaned FROM agent_sessions WHERE id = ?",
        vec![Value::Text(id.clone())],
    )
    .await
    .expect("select")
    .expect("row");
    assert!(row.is_null("deleted_at"));
    assert_eq!(row.i64("orphaned"), Some(0));
}

// ---------------------------------------------------------------------------
// Step 5 — list/delete/search/embedding tracer
// ---------------------------------------------------------------------------

/// Inserts one session with an explicit embedding and returns its id.
async fn insert_with_vector(
    store: &EpisodicStore,
    project: &str,
    session: &str,
    summary: &str,
    vector: Vec<f32>,
) -> String {
    let embedder = FakeEmbedder::with_vector(vector);
    store
        .insert_session(new_session(project, session, "agent-1", summary), &embedder)
        .await
        .expect("insert session")
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_sessions_newest_first() {
    let (_dir, store) = fixture("list-newest-first");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let oldest = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "oldest"),
            &embedder,
        )
        .await
        .expect("insert oldest");
    // Distinct millisecond timestamps: ordering must be observable, never
    // flake-tolerated (the ordering is what is under test).
    std::thread::sleep(Duration::from_millis(3));
    let middle = store
        .insert_session(
            new_session("project-a", "session-2", "agent-1", "middle"),
            &embedder,
        )
        .await
        .expect("insert middle");
    std::thread::sleep(Duration::from_millis(3));
    let newest = store
        .insert_session(
            new_session("project-a", "session-3", "agent-1", "newest"),
            &embedder,
        )
        .await
        .expect("insert newest");

    let listed = store
        .list_sessions("project-a", 10)
        .await
        .expect("list sessions");
    let ids: Vec<String> = listed.iter().map(|session| session.id.clone()).collect();
    assert_eq!(ids, vec![newest, middle, oldest]);
    let summaries: Vec<String> = listed
        .iter()
        .map(|session| session.summary.clone())
        .collect();
    assert_eq!(summaries, vec!["newest", "middle", "oldest"]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_delete_session_returns_true_then_false() {
    let (_dir, store) = fixture("delete-session");
    let id = insert_with_vector(&store, "project-a", "session-1", "doomed", unit_vector(0)).await;

    assert!(store.delete_session(&id).await.expect("delete"));
    assert!(!store.delete_session(&id).await.expect("delete again"));
    assert!(store.get_session(&id).await.expect("get").is_none());
    assert_eq!(store.count_sessions(None).await.expect("count"), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_limit_zero_returns_empty() {
    let (_dir, store) = fixture("list-limit-zero");
    insert_with_vector(&store, "project-a", "session-1", "present", unit_vector(0)).await;

    let listed = store
        .list_sessions("project-a", 0)
        .await
        .expect("list with zero limit");
    assert!(listed.is_empty(), "got {listed:?}");
    // The row is still there — zero limit is not a delete.
    assert_eq!(store.count_sessions(None).await.expect("count"), 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_search_sessions_ranks_by_similarity() {
    let (_dir, store) = fixture("search-ranking");
    let embedder = FakeEmbedder::with_vectors(vec![
        unit_vector(0),
        unit_vector(1),
        vec![0.707_106_77, 0.707_106_77, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ]);

    let exact = store
        .insert_session(
            new_session("project-a", "session-exact", "agent-1", "exact"),
            &embedder,
        )
        .await
        .expect("insert exact");
    let orthogonal = store
        .insert_session(
            new_session("project-a", "session-orthogonal", "agent-1", "orthogonal"),
            &embedder,
        )
        .await
        .expect("insert orthogonal");
    let diagonal = store
        .insert_session(
            new_session("project-a", "session-diagonal", "agent-1", "diagonal"),
            &embedder,
        )
        .await
        .expect("insert diagonal");

    let query = unit_vector(0);
    let hits: Vec<SessionSearchHit> = store
        .search_sessions("project-a", &query, 10)
        .await
        .expect("search");

    let ids: Vec<String> = hits.iter().map(|hit| hit.session.id.clone()).collect();
    assert_eq!(ids, vec![exact, diagonal, orthogonal], "hits: {hits:?}");
    assert!(hits[0].similarity > hits[1].similarity);
    assert!(hits[1].similarity > hits[2].similarity);
    assert!((hits[0].similarity - 1.0).abs() < 1e-6);

    // Zero limit returns nothing.
    assert!(
        store
            .search_sessions("project-a", &query, 0)
            .await
            .expect("search zero")
            .is_empty()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_embedding_for_round_trips() {
    let (_dir, store) = fixture("embedding-readback");
    let vector = vec![0.25_f32, 0.5, 0.75, 1.0, 0.0, 0.0, 0.0, 0.0];
    let id = insert_with_vector(
        &store,
        "project-a",
        "session-1",
        "vector row",
        vector.clone(),
    )
    .await;

    let stored = store
        .embedding_for(&id)
        .await
        .expect("embedding_for")
        .expect("embedding present");
    assert_eq!(stored.len(), DIMS);
    for (stored, expected) in stored.iter().zip(vector.iter()) {
        assert!((stored - expected).abs() < 1e-6, "{stored} vs {expected}");
    }

    assert!(
        store
            .embedding_for("missing-id")
            .await
            .expect("embedding_for missing")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_delete_parent_nulls_child_parent_session_id() {
    let (_dir, store) = fixture("delete-parent-set-null");
    let parent_id = insert_with_vector(
        &store,
        "project-a",
        "session-parent",
        "parent",
        unit_vector(0),
    )
    .await;

    let mut child = new_session("project-a", "session-child", "agent-1", "child");
    child.parent_session_id = Some(parent_id.clone());
    let child_id = store
        .insert_session(child, &FakeEmbedder::with_vector(unit_vector(1)))
        .await
        .expect("insert child");

    let stored_child = store
        .get_session(&child_id)
        .await
        .expect("get child")
        .expect("child row");
    assert_eq!(
        stored_child.parent_session_id.as_deref(),
        Some(parent_id.as_str())
    );

    assert!(
        store
            .delete_session(&parent_id)
            .await
            .expect("delete parent")
    );

    let orphaned = store
        .get_session(&child_id)
        .await
        .expect("get child after parent delete")
        .expect("child row remains");
    assert_eq!(orphaned.parent_session_id, None);
    assert_eq!(store.count_sessions(None).await.expect("count"), 1);
}

// ---------------------------------------------------------------------------
// Step 5b — F4 store seam: rebind keeps the injected ConnectFactory
// ---------------------------------------------------------------------------

/// Deliberate scope-binding lock-in for the rebind seam: rebind must reuse the
/// injected factory instead of hardcoding the production one.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_rebind_keeps_injected_connect_factory() {
    let dir_a = TestDir::new("rebind-source");
    let dir_b = TestDir::new("rebind-target");
    let connects = Arc::new(AtomicUsize::new(0));
    let factory: Arc<dyn ConnectFactory> = Arc::new(CountingFactory {
        connects: connects.clone(),
    });
    let mut store = EpisodicStore::with_factory(dir_a.to_path_buf(), dir_a.store_config(), factory);

    store
        .ready()
        .await
        .expect("ready over the injected factory");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "the injected factory opened the first database"
    );

    store.rebind(dir_b.to_path_buf(), dir_b.store_config());
    store.ready().await.expect("ready after rebind");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        2,
        "rebind must reuse the injected factory, never the production one"
    );
    assert_eq!(store.db_path(), dir_b.path().join(EPISODIC_DB_NAME));
}

/// A [`ConnectFactory`] that counts how often it is asked to open a database
/// and otherwise delegates to the production factory.
struct CountingFactory {
    connects: Arc<AtomicUsize>,
}

impl ConnectFactory for CountingFactory {
    fn connect(
        &self,
        path: PathBuf,
        opts: ConnectOptions,
    ) -> BoxFuture<'static, Result<DbHandle, StoreError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        turso_connect_factory().connect(path, opts)
    }
}

// ---------------------------------------------------------------------------
// Step 6 — scope-binding lock-in
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_list_sessions_is_project_scoped() {
    let (_dir, store) = fixture("scope-list");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    let a_one = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "a one"),
            &embedder,
        )
        .await
        .expect("insert a one");
    let a_two = store
        .insert_session(
            new_session("project-a", "session-2", "agent-1", "a two"),
            &embedder,
        )
        .await
        .expect("insert a two");
    let b_one = store
        .insert_session(
            new_session("project-b", "session-1", "agent-1", "b one"),
            &embedder,
        )
        .await
        .expect("insert b one");

    let listed_a: Vec<String> = store
        .list_sessions("project-a", 50)
        .await
        .expect("list a")
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(listed_a.len(), 2, "no cross-project bleed: {listed_a:?}");
    assert!(listed_a.contains(&a_one) && listed_a.contains(&a_two));
    assert!(!listed_a.contains(&b_one));

    let listed_b: Vec<String> = store
        .list_sessions("project-b", 50)
        .await
        .expect("list b")
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(listed_b, vec![b_one]);

    assert!(
        store
            .list_sessions("project-missing", 50)
            .await
            .expect("list missing project")
            .is_empty()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_search_sessions_is_project_scoped() {
    let (_dir, store) = fixture("scope-search");
    let embedder = FakeEmbedder::with_vectors(vec![unit_vector(0), unit_vector(1)]);

    let a_row = store
        .insert_session(
            new_session("project-a", "session-1", "agent-1", "a row"),
            &embedder,
        )
        .await
        .expect("insert a row");
    let b_row = store
        .insert_session(
            new_session("project-b", "session-1", "agent-1", "b row"),
            &embedder,
        )
        .await
        .expect("insert b row");

    let hits: Vec<String> = store
        .search_sessions("project-a", &unit_vector(0), 50)
        .await
        .expect("search a")
        .into_iter()
        .map(|hit| hit.session.id)
        .collect();
    assert_eq!(hits, vec![a_row], "search must not see other projects");
    assert!(!hits.contains(&b_row));

    let hits_b: Vec<String> = store
        .search_sessions("project-b", &unit_vector(0), 50)
        .await
        .expect("search b")
        .into_iter()
        .map(|hit| hit.session.id)
        .collect();
    assert_eq!(hits_b, vec![b_row]);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_count_sessions_per_project() {
    let (_dir, store) = fixture("scope-count");
    let embedder = FakeEmbedder::with_vector(unit_vector(0));

    for (project, session) in [
        ("project-a", "session-1"),
        ("project-a", "session-2"),
        ("project-b", "session-1"),
    ] {
        store
            .insert_session(new_session(project, session, "agent-1", "row"), &embedder)
            .await
            .expect("insert");
    }

    assert_eq!(store.count_sessions(Some("project-a")).await.expect("a"), 2);
    assert_eq!(store.count_sessions(Some("project-b")).await.expect("b"), 1);
    assert_eq!(
        store
            .count_sessions(Some("project-missing"))
            .await
            .expect("missing"),
        0
    );
    assert_eq!(store.count_sessions(None).await.expect("all"), 3);
}
