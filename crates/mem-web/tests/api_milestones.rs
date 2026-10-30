//! W6.6 (issue #30): the agent-contribution milestone projection over the
//! **real** `EpisodicStore`.
//!
//! Every case drives the shipped router (`build_router` + `oneshot`) with the
//! `x-opencode-mem-token` header, so the auth gate, the 1 MiB body cap and the
//! D-8 Host check all run. The store is a real `episodic.db` under a unique
//! temp dir with the hermetic [`store_config`]; milestones are inserted raw
//! through `EpisodicStore::write_batch` (the consolidator's own write path is
//! out of scope here).
//!
//! Security surface pinned here:
//! - `project_id` is data, never a path: a percent-encoded traversal id reaches
//!   the store as an id and never touches the filesystem — the storage
//!   directory listing is identical afterwards (declared regression guard);
//! - the list is project-scoped and newest-first, bounded by `?limit`
//!   (default 200, max 500, strict `400` outside — never a clamp);
//! - the promoted-facts join reads `facts.db` through `facts.source_ref` and
//!   fails closed: a broken facts read is a fixed `500`, never an empty
//!   success, and a GET never creates `facts.db`.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use mem_core::tier::MemoryTier;
use mem_episodic::store::EpisodicStore;
use mem_facts::schema::fact_id;
use mem_facts::store::{FactInput, FactSource, FactsStore};
use mem_store::config::StoreConfig;
use mem_store::db::{Statement, Value};
use mem_store::vector::vector_to_text;
use mem_web::api::ports::ApiServices;
use mem_web::api::store_adapter::episodic_milestone_port;
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value as JsonValue;

use common::{RawResponse, TempDir, api_request, app_state_with_api};

/// The project the fixture seeds.
const PROJECT: &str = "demo-project";
/// A second project (the project-scoping case).
const OTHER_PROJECT: &str = "other-project";
/// The API token the fixture's state is built with.
const TOKEN: &str = "w66-test-token";

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

/// A raw `episodic_milestones` insert (the consolidation output shape).
struct MilestoneSeed<'a> {
    id: &'a str,
    project_id: &'a str,
    summary: &'a str,
    contributors: &'a str,
    source_count: i64,
    window_start: i64,
    window_end: i64,
    created_at: i64,
}

/// Builds the raw insert statement (`vector32(?)` carries the 8-dim write).
fn milestone_statement(seed: MilestoneSeed<'_>) -> Statement {
    Statement::new(
        "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors, source_count, window_start, window_end, embedding, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?), ?)",
        vec![
            Value::Text(seed.id.to_string()),
            Value::Text(seed.project_id.to_string()),
            Value::Text("agent-a".to_string()),
            Value::Text(seed.summary.to_string()),
            Value::Text(seed.contributors.to_string()),
            Value::Integer(seed.source_count),
            Value::Integer(seed.window_start),
            Value::Integer(seed.window_end),
            Value::Text(vector_to_text(&[0.5_f32; 8]).expect("vector serializes")),
            Value::Integer(seed.created_at),
        ],
    )
}

/// A milestone with an empty contributor list, one source and a one-instant
/// window (the ordering/cap fixtures).
fn simple_milestone(id: &str, project_id: &str, created_at: i64) -> Statement {
    milestone_statement(MilestoneSeed {
        id,
        project_id,
        summary: "page fixture milestone",
        contributors: "[]",
        source_count: 1,
        window_start: created_at,
        window_end: created_at,
        created_at,
    })
}

/// A real episodic store plus its temp dir and the shipped router over it.
struct Fixture {
    dir: TempDir,
    store: Arc<EpisodicStore>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = TempDir::new("w66-milestones");
        let store = Arc::new(EpisodicStore::new(
            dir.path().clone(),
            store_config(dir.path().clone()),
        ));
        store.ready().await.expect("the episodic store must open");
        Self { dir, store }
    }

    /// The shipped router with the real projection injected through the W6.6
    /// seam; every other port stays `unavailable()`, so a test can only pass
    /// through this wave's adapter.
    fn router(&self) -> Router {
        self.router_with_facts(None)
    }

    /// The shipped router with an explicit `facts.db` seam (the W6.6 join).
    fn router_with_facts(&self, facts: Option<Arc<FactsStore>>) -> Router {
        let api = ApiServices::unavailable()
            .with_milestones(episodic_milestone_port(Arc::clone(&self.store), facts));
        let state = app_state_with_api(
            WebAuth::new(None, None, "tester"),
            Some(TOKEN),
            None,
            self.dir.path().clone(),
            common::throwaway_log_dir(),
            api,
            "tester@example.com",
            "127.0.0.1",
        );
        build_router(Arc::new(state))
    }

    async fn seed(&self, statements: Vec<Statement>) {
        self.store
            .write_batch(statements)
            .await
            .expect("seed milestones");
    }

    async fn get(&self, path: &str) -> RawResponse {
        api_request(self.router(), "GET", path, Some(TOKEN), None).await
    }

    async fn get_with_facts(&self, facts: Option<Arc<FactsStore>>, path: &str) -> RawResponse {
        api_request(
            self.router_with_facts(facts),
            "GET",
            path,
            Some(TOKEN),
            None,
        )
        .await
    }
}

/// The `data` value of a successful envelope.
fn envelope_data(response: &RawResponse) -> JsonValue {
    let body: JsonValue = serde_json::from_str(&response.body_string())
        .unwrap_or_else(|error| panic!("the body is JSON: {error}: {}", response.body_string()));
    assert_eq!(
        body.get("success"),
        Some(&serde_json::json!(true)),
        "the request succeeded: {}",
        response.body_string()
    );
    body.get("data")
        .cloned()
        .unwrap_or_else(|| panic!("the envelope carries data: {}", response.body_string()))
}

/// The storage directory's listing (entry names, sorted) — the traversal
/// guard's observable.
fn dir_listing(path: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .expect("the storage directory is readable")
        .map(|entry| {
            entry
                .expect("a directory entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The projection carries the milestone's VALUES (not just its keys): the
/// contributor fields, the summary, the window, the count, the creation stamp
/// — and never the write-only embedding.
#[tokio::test]
async fn test_milestones_list_returns_the_project_projection() {
    let fixture = Fixture::new().await;
    let contributors = r#"[{"sessionRowId":"row-alpha","sessionId":"session-alpha","parentSessionId":"parent-alpha","agentId":"agent-alpha","agentName":"Agent Alpha","createdAt":111,"summary":"alpha contribution"}]"#;
    fixture
        .seed(vec![milestone_statement(MilestoneSeed {
            id: "m-alpha",
            project_id: PROJECT,
            summary: "merged alpha summary",
            contributors,
            source_count: 2,
            window_start: 100,
            window_end: 900,
            created_at: 950,
        })])
        .await;

    let response = fixture.get("/api/projects/demo-project/milestones").await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = envelope_data(&response);
    let rows = data.as_array().expect("the data is an array");
    assert_eq!(rows.len(), 1, "{}", response.body_string());
    let row = &rows[0];
    assert_eq!(row["id"], "m-alpha");
    assert_eq!(row["projectId"], PROJECT);
    assert_eq!(row["summary"], "merged alpha summary");
    assert_eq!(row["sourceCount"], 2);
    assert_eq!(row["windowStart"], 100);
    assert_eq!(row["windowEnd"], 900);
    assert_eq!(row["createdAt"], 950);
    let contributor = &row["contributors"][0];
    assert_eq!(contributor["sessionRowId"], "row-alpha");
    assert_eq!(contributor["sessionId"], "session-alpha");
    assert_eq!(contributor["parentSessionId"], "parent-alpha");
    assert_eq!(contributor["agentId"], "agent-alpha");
    assert_eq!(contributor["agentName"], "Agent Alpha");
    assert_eq!(contributor["createdAt"], 111);
    assert_eq!(contributor["summary"], "alpha contribution");
    assert!(
        row.get("embedding").is_none(),
        "the write-only embedding never travels the wire: {row}"
    );
    assert_eq!(
        row["promotedFacts"],
        serde_json::json!([]),
        "a milestone with no promoted facts carries an empty array"
    );
}

/// The projection is project-scoped: another project's milestone never leaks
/// (the single row is the positive control).
#[tokio::test]
async fn test_milestones_list_is_project_scoped() {
    let fixture = Fixture::new().await;
    fixture
        .seed(vec![
            simple_milestone("m-demo", PROJECT, 10),
            simple_milestone("m-other", OTHER_PROJECT, 20),
        ])
        .await;

    let response = fixture.get("/api/projects/demo-project/milestones").await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = response.body_string();
    let data = envelope_data(&response);
    let rows = data.as_array().expect("the data is an array");
    assert_eq!(rows.len(), 1, "only the requested project: {body}");
    assert_eq!(rows[0]["id"], "m-demo");
    assert!(
        !body.contains("m-other"),
        "another project's milestone id never appears: {body}"
    );
}

/// Newest first under `?limit`; the `id DESC` tie-break is observable because
/// the ids run opposite to the creation stamps and one pair ties.
#[tokio::test]
async fn test_milestones_list_selects_the_newest_rows_under_the_limit() {
    let fixture = Fixture::new().await;
    fixture
        .seed(vec![
            simple_milestone("m-aa", PROJECT, 300),
            simple_milestone("m-t1", PROJECT, 200),
            simple_milestone("m-t2", PROJECT, 200),
            simple_milestone("m-zz", PROJECT, 100),
        ])
        .await;

    let response = fixture
        .get("/api/projects/demo-project/milestones?limit=2")
        .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = envelope_data(&response);
    let ids: Vec<&str> = data
        .as_array()
        .expect("the data is an array")
        .iter()
        .map(|row| row["id"].as_str().expect("an id string"))
        .collect();
    assert_eq!(ids, vec!["m-aa", "m-t2"]);
}

/// The absent `?limit` is bounded at 200: 201 milestones answer exactly the
/// newest 200 and the oldest id never appears.
#[tokio::test]
async fn test_milestones_list_caps_at_the_default_limit() {
    let fixture = Fixture::new().await;
    let statements = (1..=201)
        .map(|index| {
            let id = format!("m-{index:03}");
            simple_milestone(&id, PROJECT, index)
        })
        .collect();
    fixture.seed(statements).await;

    let response = fixture.get("/api/projects/demo-project/milestones").await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = envelope_data(&response);
    let rows = data.as_array().expect("the data is an array");
    assert_eq!(rows.len(), 200, "the default limit bounds the page");
    assert_eq!(rows[0]["id"], "m-201", "the newest row leads");
    assert_eq!(rows[199]["id"], "m-002", "the page stops at the bound");
    assert!(
        !response.body_string().contains("m-001"),
        "the oldest row is beyond the bound: {}",
        response.body_string()
    );
}

/// A malformed or out-of-range `?limit` is the fixed `400` — never a clamp,
/// never a silent default. `%2B5` (`"+5"`) and `%205` (`" 5"`) are the
/// discriminators against a bare `parse::<usize>()` and a trim-based parse.
#[tokio::test]
async fn test_milestones_list_rejects_a_malformed_limit() {
    let fixture = Fixture::new().await;
    for raw in ["501", "0", "abc", "%2B5", "%205"] {
        let response = fixture
            .get(&format!(
                "/api/projects/demo-project/milestones?limit={raw}"
            ))
            .await;
        assert_eq!(
            response.status,
            400,
            "?limit={raw} is refused: {}",
            response.body_string()
        );
        assert_eq!(
            response.body_string(),
            r#"{"success":false,"error":"Invalid limit parameter"}"#,
            "?limit={raw} answers the fixed body"
        );
    }
}

/// An empty project segment is `404`, not `500`, not `200` and not `400`:
/// `matchit` 0.8.4 matches an empty segment as a `{project_id}` value, so the
/// adapter maps it to "the request addresses no project" before the store
/// sees it.
#[tokio::test]
async fn test_milestones_empty_project_segment_is_404() {
    let fixture = Fixture::new().await;
    let response = fixture.get("/api/projects//milestones").await;
    assert_eq!(response.status, 404, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":false,"error":"Project not found"}"#
    );
    assert!(
        response
            .header("content-type")
            .unwrap_or_default()
            .contains("application/json"),
        "the 404 is JSON: {:?}",
        response.header("content-type")
    );
}

/// A non-empty malformed id goes through the store's reject-never-coerce
/// `check_id`: `400` with the field name and the fixed reason — never the
/// offending value.
#[tokio::test]
async fn test_milestones_malformed_project_id_returns_400() {
    let fixture = Fixture::new().await;
    let response = fixture.get("/api/projects/%20padded%20/milestones").await;
    assert_eq!(response.status, 400, "{}", response.body_string());
    let body = response.body_string();
    assert_eq!(
        body,
        r#"{"success":false,"error":"invalid project_id: must not have leading or trailing whitespace"}"#
    );
    assert!(
        !body.contains("padded"),
        "the offending value never appears: {body}"
    );
}

/// Inserts one fact with the given `source_ref` and rewrites its `created_at`
/// raw (`FactsStore::insert` stamps `now_millis()`).
async fn seed_fact(
    facts: &FactsStore,
    project_id: &str,
    tier: MemoryTier,
    content: &str,
    source_ref: &str,
    created_at: i64,
) -> String {
    facts
        .insert(FactInput {
            project_id: project_id.to_string(),
            tier,
            content: content.to_string(),
            key: None,
            source_ref: Some(source_ref.to_string()),
            confidence: None,
            metadata: None,
            embedding: None,
            model_id: None,
            sources: vec![FactSource {
                project_id: project_id.to_string(),
                session_id: "session-a".to_string(),
                agent_id: "agent-a".to_string(),
                milestone_id: source_ref.to_string(),
            }],
        })
        .await
        .expect("insert fact");
    let id = fact_id(project_id, tier, content);
    let db = facts.ready().await.expect("facts ready");
    db.execute(
        "UPDATE facts SET created_at = ? WHERE id = ?",
        vec![Value::Integer(created_at), Value::Text(id.clone())],
    )
    .await
    .expect("set created_at");
    id
}

/// The join is `facts.source_ref`: only the live facts of THIS project and
/// THIS milestone travel; a soft-deleted fact, a fact of another milestone and
/// a foreign-project fact are all absent. No `memoryType` is invented — the
/// durable tier is a different concept.
#[tokio::test]
async fn test_milestones_list_carries_only_that_milestones_promoted_facts() {
    let fixture = Fixture::new().await;
    fixture
        .seed(vec![simple_milestone("m1", PROJECT, 500)])
        .await;

    let facts = Arc::new(FactsStore::new(store_config(fixture.dir.path().clone())));
    let alpha_id = seed_fact(
        &facts,
        PROJECT,
        MemoryTier::Semantic,
        "alpha decision",
        "m1",
        10,
    )
    .await;
    seed_fact(
        &facts,
        PROJECT,
        MemoryTier::Procedural,
        "beta decision",
        "m2",
        20,
    )
    .await;
    let deleted_id = seed_fact(
        &facts,
        PROJECT,
        MemoryTier::Resource,
        "deleted decision",
        "m1",
        30,
    )
    .await;
    assert!(facts.soft_delete(&deleted_id).await.expect("soft delete"));
    seed_fact(
        &facts,
        OTHER_PROJECT,
        MemoryTier::Semantic,
        "foreign decision",
        "m1",
        40,
    )
    .await;

    let response = fixture
        .get_with_facts(Some(facts), "/api/projects/demo-project/milestones")
        .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = response.body_string();
    let data = envelope_data(&response);
    let promoted = data[0]["promotedFacts"]
        .as_array()
        .expect("promotedFacts is an array");
    assert_eq!(
        promoted.len(),
        1,
        "only m1's live same-project fact: {body}"
    );
    assert_eq!(promoted[0]["id"], alpha_id);
    assert_eq!(promoted[0]["content"], "alpha decision");
    assert_eq!(promoted[0]["createdAt"], 10);
    assert!(
        promoted[0].get("memoryType").is_none(),
        "durable facts carry a tier, never an upstream memory type: {body}"
    );
    assert!(
        !body.contains("deleted decision") && !body.contains("foreign decision"),
        "the soft-deleted and foreign rows never travel: {body}"
    );
}

/// A broken facts read is the D-9 fixed `500` — never an empty success, and
/// the store message / path never reaches the body.
#[tokio::test]
async fn test_milestones_facts_read_failure_answers_500_without_detail() {
    let fixture = Fixture::new().await;
    fixture
        .seed(vec![simple_milestone("m1", PROJECT, 500)])
        .await;

    let facts = Arc::new(FactsStore::new(store_config(fixture.dir.path().clone())));
    seed_fact(
        &facts,
        PROJECT,
        MemoryTier::Semantic,
        "alpha decision",
        "m1",
        10,
    )
    .await;
    let db = facts.ready().await.expect("facts ready");
    db.execute("DROP TABLE facts", Vec::new())
        .await
        .expect("drop the facts table");

    let response = fixture
        .get_with_facts(Some(facts), "/api/projects/demo-project/milestones")
        .await;
    assert_ne!(response.status, 200, "a broken read is not a success");
    assert_eq!(
        response.status,
        500,
        "the D-9 internal error: {}",
        response.body_string()
    );
    let body = response.body_string();
    assert_eq!(
        body, r#"{"success":false,"error":"Internal Server Error"}"#,
        "the fixed body"
    );
    assert!(
        !body.contains('/') && !body.contains("facts"),
        "no store message and no path: {body}"
    );
}

/// A GET must never create `facts.db`; an unopened store is reported as "no
/// promoted facts", not as a reason to migrate the file into existence.
#[tokio::test]
async fn test_milestones_list_never_creates_facts_db_and_reports_no_facts() {
    let fixture = Fixture::new().await;
    fixture
        .seed(vec![simple_milestone("m1", PROJECT, 500)])
        .await;
    let facts_path = fixture.dir.path().join("facts.db");

    // Arm 1 (a Step-4/5 regression guard): `facts: None` is an empty list.
    let response = fixture.get("/api/projects/demo-project/milestones").await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = envelope_data(&response);
    assert_eq!(data[0]["promotedFacts"], serde_json::json!([]));
    assert!(
        !facts_path.exists(),
        "no facts.db exists before any facts read"
    );

    // Arm 2: the store is constructed but never opened; the request must not
    // open it (`ready()` would create the file).
    let unopened = Arc::new(FactsStore::new(store_config(fixture.dir.path().clone())));
    let response = fixture
        .get_with_facts(Some(unopened), "/api/projects/demo-project/milestones")
        .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let data = envelope_data(&response);
    assert_eq!(data[0]["promotedFacts"], serde_json::json!([]));
    assert!(
        !facts_path.exists(),
        "a GET never creates facts.db: {}",
        response.body_string()
    );
}

/// `project_id` is data, never a path. Declared **regression guard** (green on
/// its first run): it kills a future id-becomes-a-path change, not a current
/// mutant.
#[tokio::test]
async fn test_milestones_traversal_project_id_carried_as_data() {
    let fixture = Fixture::new().await;
    fixture
        .seed(vec![simple_milestone("m-one", PROJECT, 10)])
        .await;
    let before = dir_listing(fixture.dir.path());

    let response = fixture.get("/api/projects/..%2F..%2Fetc/milestones").await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert_eq!(
        response.body_string(),
        r#"{"success":true,"data":[]}"#,
        "the traversal-shaped id selects no rows"
    );
    assert_eq!(
        dir_listing(fixture.dir.path()),
        before,
        "the id never becomes a filesystem path"
    );
}
