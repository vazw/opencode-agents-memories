//! W6.3 (issue #25): the session-history endpoints over the **real**
//! `EpisodicStore`.
//!
//! Every case drives the shipped router (`build_router` + `oneshot`) with the
//! `x-opencode-mem-token` header, so the auth gate, the 1 MiB body cap and the
//! D-8 Host check all run. The store is a real `episodic.db` under a unique
//! temp dir with the hermetic [`store_config`], a fixed-vector [`FixedEmbedder`]
//! and an injected clock + id factory (deterministic newest-first ordering).
//!
//! Security surface pinned here:
//! - `project_id` / `session_id` are data, never paths: a percent-encoded
//!   traversal id reaches the store as an id (`check_id` accepts it) and never
//!   touches the filesystem — the store directory is byte-identical afterwards;
//! - a delete is project-scoped: wrong-project, unknown and already-deleted
//!   answer the same fixed `404` with no existence leak;
//! - the delete marks children orphaned in the same atomic write batch, so the
//!   child stays visible in the timeline, flagged.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use axum::Router;
use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_store::config::StoreConfig;
use mem_web::api::ports::ApiServices;
use mem_web::api::store_adapter::episodic_session_port;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::{Value, json};

use common::{RawResponse, TempDir, api_request, app_state_with_api};

/// The project the fixture seeds.
const PROJECT: &str = "demo-project";
/// A second project (the delete-is-project-scoped cases).
const OTHER_PROJECT: &str = "other-project";
/// The API token the fixture's state is built with.
const TOKEN: &str = "w63-test-token";
/// The fixture's base timestamp (2026-09-01T00:00:00Z).
const T0: i64 = 1_788_220_800_000;

// ---------------------------------------------------------------------------
// Hermetic store fixture
// ---------------------------------------------------------------------------

/// The hermetic store config: encryption off, 8 dimensions.
fn store_config(storage_path: PathBuf) -> StoreConfig {
    StoreConfig {
        storage_path,
        embedding_dimensions: 8,
        embedding_model: "test".to_string(),
        max_vectors_per_shard: 100,
        container_tag_prefix: "mem".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// A fixed 8-dimension embedding: the store's dimension check passes and no
/// provider is involved.
struct FixedEmbedder;

impl Embedder for FixedEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]) })
    }
}

/// A real episodic store plus its temp dir.
struct Fixture {
    dir: TempDir,
    store: Arc<EpisodicStore>,
    embedder: FixedEmbedder,
    clock: Arc<AtomicI64>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = TempDir::new("w63-sessions");
        let clock = Arc::new(AtomicI64::new(T0));
        let clock_seam: mem_episodic::store::Clock = {
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        };
        let ids = Arc::new(AtomicU64::new(0));
        let id_factory: mem_episodic::store::IdFactory = {
            let ids = Arc::clone(&ids);
            Arc::new(move || {
                let next = ids.fetch_add(1, Ordering::SeqCst) + 1;
                format!("row-{next:04}")
            })
        };
        let store = Arc::new(
            EpisodicStore::new(dir.path().clone(), store_config(dir.path().clone()))
                .with_clock(clock_seam)
                .with_id_factory(id_factory),
        );
        store.ready().await.expect("the episodic store must open");
        Self {
            dir,
            store,
            embedder: FixedEmbedder,
            clock,
        }
    }

    /// Inserts one row at `created_at` (unix millis) and returns its row id.
    #[allow(clippy::too_many_arguments)]
    async fn seed(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
        agent_name: &str,
        summary: &str,
        parent_session_id: Option<&str>,
        created_at: i64,
    ) -> String {
        self.clock.store(created_at, Ordering::SeqCst);
        let new = NewAgentSession {
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            parent_session_id: parent_session_id.map(str::to_owned),
            summary: summary.to_string(),
            agent_name: agent_name.to_string(),
            ..Default::default()
        };
        self.store
            .insert_session(new, &self.embedder)
            .await
            .expect("the fixture row must insert")
    }

    /// The port bundle the router is built over.
    ///
    /// The real `EpisodicSessionPort` is injected through the W6.3 seam
    /// (`with_sessions`); every other port stays `unavailable()`, so a session
    /// test can only pass through this adapter.
    fn api(&self) -> ApiServices {
        ApiServices::unavailable().with_sessions(episodic_session_port(Arc::clone(&self.store)))
    }

    /// The shipped router over this fixture's store.
    fn router(&self) -> Router {
        let state = app_state_with_api(
            WebAuth::new(None, None, "tester"),
            Some(TOKEN),
            None,
            self.dir.path().clone(),
            common::throwaway_log_dir(),
            self.api(),
            "tester@example.com",
            "127.0.0.1",
        );
        build_router(Arc::new(state))
    }

    /// Every entry under the store's temp dir, as sorted relative paths.
    fn dir_entries(&self) -> Vec<String> {
        fn walk(root: &Path, current: &Path, entries: &mut Vec<String>) {
            let Ok(read_dir) = std::fs::read_dir(current) else {
                return;
            };
            for entry in read_dir.flatten() {
                let path = entry.path();
                if let Ok(relative) = path.strip_prefix(root) {
                    entries.push(relative.to_string_lossy().into_owned());
                }
                if path.is_dir() {
                    walk(root, &path, entries);
                }
            }
        }
        let mut entries = Vec::new();
        walk(self.dir.path(), self.dir.path(), &mut entries);
        entries.sort();
        entries
    }
}

/// The parsed JSON body of a response.
fn body_json(response: &RawResponse) -> Value {
    serde_json::from_str(&response.body_string()).unwrap_or_else(|error| {
        panic!(
            "the response body must be JSON (status {}): {error} / {:?}",
            response.status,
            response.body_string()
        )
    })
}

/// The `data` array of a success envelope.
fn data_array(response: &RawResponse) -> Vec<Value> {
    let body = body_json(response);
    assert_eq!(
        body["success"],
        json!(true),
        "expected a success envelope, got {}",
        response.body_string()
    );
    body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("data must be an array: {}", response.body_string()))
        .clone()
}

// ---------------------------------------------------------------------------
// AC 1 — the project timeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sessions_list_returns_project_timeline() {
    let fixture = Fixture::new().await;
    let parent = fixture
        .seed(
            PROJECT,
            "demo-session",
            "agent-parent",
            "Planner",
            "Parent session: scaffolded the demo",
            None,
            T0,
        )
        .await;
    let child = fixture
        .seed(
            PROJECT,
            "demo-session",
            "agent-child",
            "Implementer",
            "Child session: implemented the endpoint",
            Some(&parent),
            T0 + 1_000,
        )
        .await;
    // A second project's row must never leak into this project's timeline.
    let _other = fixture
        .seed(
            OTHER_PROJECT,
            "demo-session",
            "agent-parent",
            "Planner",
            "Another project's session",
            None,
            T0 + 2_000,
        )
        .await;

    let response = api_request(
        fixture.router(),
        "GET",
        &format!("/api/projects/{PROJECT}/sessions"),
        Some(TOKEN),
        None,
    )
    .await;

    assert_eq!(
        response.status,
        200,
        "GET sessions answered {}: {}",
        response.status,
        response.body_string()
    );
    let data = data_array(&response);
    assert_eq!(
        data.len(),
        2,
        "only the requested project's live rows: {}",
        response.body_string()
    );

    let newest = &data[0];
    assert_eq!(newest["id"], json!(child), "newest-first order");
    assert_eq!(newest["agentName"], json!("Implementer"));
    assert_eq!(newest["agentId"], json!("agent-child"));
    assert_eq!(
        newest["summary"],
        json!("Child session: implemented the endpoint")
    );
    assert_eq!(newest["createdAt"], json!(T0 + 1_000));
    assert_eq!(newest["orphaned"], json!(false));
    assert_eq!(newest["parentSessionId"], json!(parent));

    assert_eq!(data[1]["id"], json!(parent));
    assert_eq!(data[1]["agentName"], json!("Planner"));
}

// ---------------------------------------------------------------------------
// Delete — the happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sessions_delete_returns_ok_and_hides_row() {
    let fixture = Fixture::new().await;
    let older = fixture
        .seed(
            PROJECT,
            "session-one",
            "agent-a",
            "Agent A",
            "Older row",
            None,
            T0,
        )
        .await;
    let target = fixture
        .seed(
            PROJECT,
            "session-two",
            "agent-b",
            "Agent B",
            "Row to delete",
            None,
            T0 + 1_000,
        )
        .await;

    let response = api_request(
        fixture.router(),
        "DELETE",
        &format!("/api/projects/{PROJECT}/sessions/{target}"),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        200,
        "DELETE answered {}: {}",
        response.status,
        response.body_string()
    );
    assert_eq!(
        response.body_string(),
        r#"{"success":true}"#,
        "the data key is omitted, like every data-less success"
    );

    let list = api_request(
        fixture.router(),
        "GET",
        &format!("/api/projects/{PROJECT}/sessions"),
        Some(TOKEN),
        None,
    )
    .await;
    let data = data_array(&list);
    assert_eq!(data.len(), 1, "a soft-deleted row is invisible (D-11)");
    assert_eq!(data[0]["id"], json!(older));
}

// ---------------------------------------------------------------------------
// Delete — the fixed 404 (no existence leak)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sessions_delete_unknown_returns_404() {
    let fixture = Fixture::new().await;

    let response = api_request(
        fixture.router(),
        "DELETE",
        &format!("/api/projects/{PROJECT}/sessions/no-such-row"),
        Some(TOKEN),
        None,
    )
    .await;

    assert_eq!(
        response.status,
        404,
        "unknown id answered {}: {}",
        response.status,
        response.body_string()
    );
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Session not found"}"#
    );
}

#[tokio::test]
async fn test_sessions_delete_wrong_project_returns_404_no_leak() {
    let fixture = Fixture::new().await;
    let row = fixture
        .seed(
            PROJECT,
            "session-one",
            "agent-a",
            "Agent A",
            "Row in demo-project",
            None,
            T0,
        )
        .await;

    let response = api_request(
        fixture.router(),
        "DELETE",
        &format!("/api/projects/{OTHER_PROJECT}/sessions/{row}"),
        Some(TOKEN),
        None,
    )
    .await;

    assert_eq!(
        response.status,
        404,
        "wrong project answered {}: {}",
        response.status,
        response.body_string()
    );
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Session not found"}"#
    );
    assert!(
        !response.body_string().contains(&row),
        "the id is never echoed"
    );

    // The row is untouched in its own project (no cross-project write).
    let list = api_request(
        fixture.router(),
        "GET",
        &format!("/api/projects/{PROJECT}/sessions"),
        Some(TOKEN),
        None,
    )
    .await;
    let data = data_array(&list);
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], json!(row));
}

#[tokio::test]
async fn test_sessions_delete_twice_returns_404() {
    let fixture = Fixture::new().await;
    let row = fixture
        .seed(
            PROJECT,
            "session-one",
            "agent-a",
            "Agent A",
            "Row to delete twice",
            None,
            T0,
        )
        .await;
    let path = format!("/api/projects/{PROJECT}/sessions/{row}");

    let first = api_request(fixture.router(), "DELETE", &path, Some(TOKEN), None).await;
    assert_eq!(first.status, 200, "body: {}", first.body_string());

    let second = api_request(fixture.router(), "DELETE", &path, Some(TOKEN), None).await;
    assert_eq!(
        second.status,
        404,
        "an already-deleted row answered {}: {}",
        second.status,
        second.body_string()
    );
    assert_eq!(
        second.body_string(),
        r#"{"success":false,"error":"Session not found"}"#
    );
}

// ---------------------------------------------------------------------------
// Orphan-safety (AC 3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sessions_delete_is_orphan_safe() {
    let fixture = Fixture::new().await;
    // Distinct agent_id under one session_id: `UNIQUE (project_id,
    // session_id, agent_id)` forbids sharing the triple, and
    // `insert_session` pre-checks that the parent is live and in-project.
    let parent = fixture
        .seed(
            PROJECT,
            "demo-session",
            "agent-parent",
            "Planner",
            "Parent session",
            None,
            T0,
        )
        .await;
    let child = fixture
        .seed(
            PROJECT,
            "demo-session",
            "agent-child",
            "Implementer",
            "Child session",
            Some(&parent),
            T0 + 1_000,
        )
        .await;

    let response = api_request(
        fixture.router(),
        "DELETE",
        &format!("/api/projects/{PROJECT}/sessions/{parent}"),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200, "body: {}", response.body_string());

    let list = api_request(
        fixture.router(),
        "GET",
        &format!("/api/projects/{PROJECT}/sessions"),
        Some(TOKEN),
        None,
    )
    .await;
    let data = data_array(&list);
    assert_eq!(
        data.len(),
        1,
        "the child stays visible: {}",
        list.body_string()
    );
    let only = &data[0];
    assert_eq!(only["id"], json!(child));
    assert_eq!(
        only["orphaned"],
        json!(true),
        "the delete marks children orphaned (D-12)"
    );
    assert_eq!(
        only["parentSessionId"],
        json!(parent),
        "the write-time flag, not a read-time null, marks the severed link"
    );
}

// ---------------------------------------------------------------------------
// Validation and router shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sessions_invalid_project_id_returns_400() {
    let fixture = Fixture::new().await;

    // `%20` decodes to a single-space segment: `check_id` rejects it as outer
    // whitespace (reject, never coerce).
    let response = api_request(
        fixture.router(),
        "GET",
        "/api/projects/%20/sessions",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        400,
        "a whitespace-only project id answered {}: {}",
        response.status,
        response.body_string()
    );
    let body = body_json(&response);
    assert_eq!(body["success"], json!(false));
    let error = body["error"].as_str().expect("an error string");
    assert!(
        error.starts_with("invalid project_id"),
        "unexpected message: {error}"
    );

    // The same rule guards the delete's session id.
    let response = api_request(
        fixture.router(),
        "DELETE",
        &format!("/api/projects/{PROJECT}/sessions/%20"),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        400,
        "a whitespace-only session id answered {}: {}",
        response.status,
        response.body_string()
    );
    let body = body_json(&response);
    let error = body["error"].as_str().expect("an error string");
    assert!(
        error.starts_with("invalid id"),
        "unexpected message: {error}"
    );
}

#[tokio::test]
async fn test_sessions_traversal_project_id_carried_as_data() {
    let fixture = Fixture::new().await;
    let row = fixture
        .seed(
            PROJECT,
            "session-one",
            "agent-a",
            "Agent A",
            "Row that must survive a traversal attempt",
            None,
            T0,
        )
        .await;
    let before = fixture.dir_entries();

    // `..%2F..%2Fetc` percent-decodes to `../../etc`: a valid id for
    // `check_id` (no whitespace, no NUL, in bounds), so it reaches the store
    // as data and matches nothing.
    let response = api_request(
        fixture.router(),
        "GET",
        "/api/projects/..%2F..%2Fetc/sessions",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        200,
        "a traversal-shaped project id answered {}: {}",
        response.status,
        response.body_string()
    );
    let data = data_array(&response);
    assert!(
        data.is_empty(),
        "the traversal id never matches a project: {}",
        response.body_string()
    );

    let delete = api_request(
        fixture.router(),
        "DELETE",
        &format!("/api/projects/..%2F..%2Fetc/sessions/{row}"),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(
        delete.status,
        404,
        "a cross-project delete answered {}: {}",
        delete.status,
        delete.body_string()
    );
    assert_eq!(
        delete.body_string(),
        r#"{"success":false,"error":"Session not found"}"#
    );
    assert!(
        !delete.body_string().contains(".."),
        "the id is never echoed"
    );
    assert!(!delete.body_string().contains("etc"), "no path echo");

    // The traversal id never became a filesystem path.
    assert_eq!(
        fixture.dir_entries(),
        before,
        "the store directory must be unchanged"
    );

    // The row itself is untouched.
    let list = api_request(
        fixture.router(),
        "GET",
        &format!("/api/projects/{PROJECT}/sessions"),
        Some(TOKEN),
        None,
    )
    .await;
    let data = data_array(&list);
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], json!(row));
}

#[tokio::test]
async fn test_sessions_empty_segment_is_router_404() {
    let fixture = Fixture::new().await;

    // An empty segment carries no id at all. `matchit` 0.8.4 still hands it to
    // the handler as an empty `{project_id}` (an empty string is a match for a
    // parameter segment), so the "not a request for this endpoint" answer is
    // produced by the port: an empty id is `NotFound`, never a `400`
    // validation answer (blueprint: "a truly empty segment is a router 404,
    // not 400").
    let response = api_request(
        fixture.router(),
        "GET",
        "/api/projects//sessions",
        Some(TOKEN),
        None,
    )
    .await;

    assert_eq!(
        response.status,
        404,
        "an empty segment answered {}: {}",
        response.status,
        response.body_string()
    );
    assert_ne!(response.status, 400);
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Session not found"}"#
    );
}
