//! `POST /api/cleanup` retention suite: report widening, fused port, run guard,
//! grace windows, and endpoint contract (issue 39).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::real_store::{RealStore, create_shard, new_store, row_ids};
use common::{TempDir, api_request, app_state_with_api};
use mem_core::scope::Scope;
use mem_embed::embedder::{BoxFuture, Embedder};
use mem_embed::error::EmbedError;
use mem_episodic::inbox::{CaptureInbox, InboxNewEntry};
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_profile::prompt::UserPromptManager;
use mem_store::db::Value;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::{self, MemoryInsert};
use mem_web::api::deduplication::{
    CleanupSweepConfig, production_deduplication_port, production_deduplication_port_with_cleanup,
};
use mem_web::api::ports::{ApiServices, MaintenancePort, PortError};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

const TOKEN: &str = "test-token";
/// The retention window the issue-32 cases sweep with.
const RETENTION_DAYS: u32 = 30;
const USER_HASH: &str = "0123456789abcdef";
const USER_TAG: &str = "opencode_user_0123456789abcdef";
const PROJ_HASH: &str = "fedcba9876543210";
const PROJ_TAG: &str = "opencode_project_fedcba9876543210";

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_memory(
    manager: &ShardManager,
    shard: &ShardInfo,
    id: &str,
    content: &str,
    container_tag: &str,
    updated_at: i64,
    is_pinned: bool,
    source_prompt_created_at: Option<i64>,
) {
    let db = manager
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    vector::insert_vector(
        db.as_ref(),
        &MemoryInsert {
            id: id.to_string(),
            content: content.to_string(),
            vector: vec![0.0; 768],
            tags_vector: None,
            container_tag: container_tag.to_string(),
            tags: None,
            memory_type: None,
            created_at: updated_at,
            updated_at,
            metadata: None,
            session_id: None,
            display_name: None,
            user_name: None,
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
            source_prompt_created_at,
        },
    )
    .await
    .expect("insert vector");

    if is_pinned {
        db.execute(
            "UPDATE memories SET is_pinned = 1 WHERE id = ?",
            vec![Value::Text(id.to_string())],
        )
        .await
        .expect("update is_pinned");
    }

    let count = mem_store::shard_facts::sync_count(db.as_ref())
        .await
        .expect("sync count");
    manager
        .set_vector_count(shard.id, count)
        .await
        .expect("set vector count");
}

async fn insert_prompt(
    prompts: &UserPromptManager,
    id: &str,
    created_at: i64,
    linked_memory_id: Option<&str>,
) {
    let db = prompts.ready().await.expect("prompts ready");
    db.execute(
        "INSERT INTO user_prompts (id, session_id, message_id, project_path, content, created_at, linked_memory_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::Text(id.to_string()),
            Value::Text("sess-1".to_string()),
            Value::Text("msg-1".to_string()),
            Value::Text("/project".to_string()),
            Value::Text("prompt content".to_string()),
            Value::Integer(created_at),
            linked_memory_id
                .map(|s| Value::Text(s.to_string()))
                .unwrap_or(Value::Null),
        ],
    )
    .await
    .expect("insert prompt");
}

fn fixture_port(
    store: &RealStore,
    prompts: Arc<UserPromptManager>,
    retention_days: u32,
) -> Arc<dyn MaintenancePort> {
    production_deduplication_port(store.manager.clone(), prompts, retention_days, true, 0.9)
}

fn router_with(services: ApiServices) -> axum::Router {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        "unknown",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

async fn post_cleanup(router: axum::Router) -> common::RawResponse {
    api_request(router, "POST", "/api/cleanup", Some(TOKEN), None).await
}

// ---------------------------------------------------------------------------
// Step 6 tests: report widening, fused port, run guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn report_carries_five_truthful_fields() {
    let store = new_store("cleanup_report_five_fields");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));

    let user_shard = create_shard(&store, Scope::User, USER_HASH, 0).await;
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;

    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    // Seed expired unpinned user memory (deleted -> user_count +1, deleted_count +1)
    insert_memory(
        &store.manager,
        &user_shard,
        "mem-user-1",
        "user memory",
        USER_TAG,
        cutoff - 10_000,
        false,
        None,
    )
    .await;

    // Seed expired unpinned project memory (deleted -> project_count +1, deleted_count +1)
    insert_memory(
        &store.manager,
        &proj_shard,
        "mem-proj-1",
        "proj memory",
        PROJ_TAG,
        cutoff - 10_000,
        false,
        None,
    )
    .await;

    // Seed expired pinned project memory (skipped -> protected_memories_skipped +1)
    insert_memory(
        &store.manager,
        &proj_shard,
        "mem-proj-pinned",
        "pinned memory",
        PROJ_TAG,
        cutoff - 10_000,
        true,
        None,
    )
    .await;

    // Seed expired prompt in prompts DB (deleted -> prompts_deleted +1)
    insert_prompt(&prompts, "prompt-expired-1", cutoff - 10_000, None).await;

    let port = fixture_port(&store, prompts, retention_days);
    let report = port.cleanup().await.expect("cleanup succeeds");

    assert_eq!(report.deleted_count, 2, "deleted_count must equal 2");
    assert_eq!(report.user_count, 1, "user_count must equal 1");
    assert_eq!(report.project_count, 1, "project_count must equal 1");
    assert_eq!(report.prompts_deleted, 1, "prompts_deleted must equal 1");
    assert_eq!(
        report.protected_memories_skipped, 1,
        "protected_memories_skipped must equal 1"
    );
}

#[tokio::test]
async fn already_running_guard_rejects_second_run() {
    let store = new_store("cleanup_guard_rejects");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));

    let _shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let port = fixture_port(&store, prompts, 30);

    // Call cleanup twice concurrently
    let (res1, res2) = tokio::join!(port.cleanup(), port.cleanup());

    let mut ok_count = 0;
    let mut invalid_count = 0;

    for res in [res1, res2] {
        match res {
            Ok(_) => ok_count += 1,
            Err(PortError::Invalid(msg)) if msg == "cleanup is already running" => {
                invalid_count += 1;
            }
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    assert_eq!(ok_count, 1, "exactly one concurrent run must succeed");
    assert_eq!(
        invalid_count, 1,
        "exactly one concurrent run must be rejected with 'cleanup is already running'"
    );

    // Third call after completion must run clean
    let res3 = port.cleanup().await;
    assert!(
        res3.is_ok(),
        "third run after completion must succeed, got {res3:?}"
    );
}

#[tokio::test]
async fn grace_null_stamp_linked_row_survives_via_prompt_facts() {
    let store = new_store("cleanup_grace_null_stamp");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));

    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;

    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    // Memory: NULL stamp, updated_at < cutoff (base rule would delete)
    let memory_id = "mem-grace-null-stamp";
    insert_memory(
        &store.manager,
        &proj_shard,
        memory_id,
        "grace protected memory",
        PROJ_TAG,
        cutoff - 1000,
        false,
        None,
    )
    .await;

    // Prompt: created_at < cutoff (so it is expired and returned in phase 1 facts),
    // but p + 2r > cutoff (grace condition holds: cutoff - r/2 + 2r = cutoff + 1.5r > cutoff)
    let prompt_created_at = cutoff - (retention_ms / 2);
    insert_prompt(
        &prompts,
        "prompt-linked-grace",
        prompt_created_at,
        Some(memory_id),
    )
    .await;

    let port = fixture_port(&store, Arc::clone(&prompts), retention_days);
    let report = port.cleanup().await.expect("cleanup succeeds");

    assert_eq!(
        report.protected_memories_skipped, 1,
        "NULL-stamp memory must be protected by prompt facts grace"
    );
    assert_eq!(report.deleted_count, 0, "no memories should be deleted");

    let ids = row_ids(&store, &proj_shard).await;
    assert!(
        ids.contains(&memory_id.to_string()),
        "memory must survive in shard"
    );
}

// ---------------------------------------------------------------------------
// Step 7 tests: endpoint matrix, dangling link clear, 501 fix, 200 response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retention_matrix_expired_unlinked_deleted() {
    let store = new_store("matrix_expired_unlinked_deleted");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    let mem_id = "mem-matrix-row1";
    insert_memory(
        &store.manager,
        &proj_shard,
        mem_id,
        "content row1",
        PROJ_TAG,
        cutoff - 10_000,
        false,
        None,
    )
    .await;

    let services =
        ApiServices::unavailable().with_maintenance(fixture_port(&store, prompts, retention_days));
    let router = router_with(services);
    let res = post_cleanup(router).await;
    assert_eq!(res.status, 200);
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["data"]["deletedCount"], 1);

    let ids = row_ids(&store, &proj_shard).await;
    assert!(
        !ids.contains(&mem_id.to_string()),
        "expired unlinked memory must be deleted"
    );
}

#[tokio::test]
async fn retention_matrix_expired_pinned_survives() {
    let store = new_store("matrix_expired_pinned_survives");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    let mem_id = "mem-matrix-pinned";
    insert_memory(
        &store.manager,
        &proj_shard,
        mem_id,
        "content pinned",
        PROJ_TAG,
        cutoff - 10_000,
        true,
        None,
    )
    .await;

    let services =
        ApiServices::unavailable().with_maintenance(fixture_port(&store, prompts, retention_days));
    let router = router_with(services);
    let res = post_cleanup(router).await;
    assert_eq!(res.status, 200);
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["data"]["deletedCount"], 0);
    assert_eq!(val["data"]["protectedMemoriesSkipped"], 1);

    let ids = row_ids(&store, &proj_shard).await;
    assert!(
        ids.contains(&mem_id.to_string()),
        "pinned memory must survive"
    );
}

#[tokio::test]
async fn retention_matrix_expired_live_stamp_survives() {
    let store = new_store("matrix_expired_live_stamp");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    let mem_id = "mem-matrix-live-stamp";
    // Recent stamp s = cutoff - retention_ms + 10_000 (stamp term is false -> survives)
    let s = cutoff - retention_ms + 10_000;
    insert_memory(
        &store.manager,
        &proj_shard,
        mem_id,
        "content live stamp",
        PROJ_TAG,
        cutoff - 10 * retention_ms,
        false,
        Some(s),
    )
    .await;

    let services =
        ApiServices::unavailable().with_maintenance(fixture_port(&store, prompts, retention_days));
    let router = router_with(services);
    let res = post_cleanup(router).await;
    assert_eq!(res.status, 200);
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["data"]["deletedCount"], 0);

    let ids = row_ids(&store, &proj_shard).await;
    assert!(
        ids.contains(&mem_id.to_string()),
        "recent stamp memory must survive"
    );
}

#[tokio::test]
async fn retention_matrix_expired_stale_stamp_deleted() {
    let store = new_store("matrix_expired_stale_stamp");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    let mem_id = "mem-matrix-stale-stamp";
    // Stale stamp: s = cutoff - 2r - 10_000 (grace lapsed)
    let s = cutoff - 2 * retention_ms - 10_000;
    insert_memory(
        &store.manager,
        &proj_shard,
        mem_id,
        "content stale stamp",
        PROJ_TAG,
        cutoff - 10_000,
        false,
        Some(s),
    )
    .await;

    let services =
        ApiServices::unavailable().with_maintenance(fixture_port(&store, prompts, retention_days));
    let router = router_with(services);
    let res = post_cleanup(router).await;
    assert_eq!(res.status, 200);
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["data"]["deletedCount"], 1);

    let ids = row_ids(&store, &proj_shard).await;
    assert!(
        !ids.contains(&mem_id.to_string()),
        "expired stale stamp memory must be deleted"
    );
}

#[tokio::test]
async fn retention_matrix_young_live_stamp_survives() {
    let store = new_store("matrix_young_live_stamp");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    let mem_id = "mem-matrix-young";
    // Ancient stamp but young updated_at
    let s = cutoff - 3 * retention_ms;
    insert_memory(
        &store.manager,
        &proj_shard,
        mem_id,
        "content young",
        PROJ_TAG,
        now,
        false,
        Some(s),
    )
    .await;

    let services =
        ApiServices::unavailable().with_maintenance(fixture_port(&store, prompts, retention_days));
    let router = router_with(services);
    let res = post_cleanup(router).await;
    assert_eq!(res.status, 200);
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["data"]["deletedCount"], 0);

    let ids = row_ids(&store, &proj_shard).await;
    assert!(
        ids.contains(&mem_id.to_string()),
        "young memory must survive"
    );
}

#[tokio::test]
async fn dangling_link_cleared() {
    let store = new_store("dangling_link_cleared");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    let mem_id = "mem-deleted-for-link";
    insert_memory(
        &store.manager,
        &proj_shard,
        mem_id,
        "content to be deleted",
        PROJ_TAG,
        cutoff - 10_000,
        false,
        None,
    )
    .await;

    // Young surviving prompt linked to this memory
    let prompt_id = "prompt-young-surviving";
    insert_prompt(&prompts, prompt_id, now, Some(mem_id)).await;

    let services = ApiServices::unavailable().with_maintenance(fixture_port(
        &store,
        Arc::clone(&prompts),
        retention_days,
    ));
    let router = router_with(services);
    let res = post_cleanup(router).await;
    assert_eq!(res.status, 200);

    // Memory was deleted
    let ids = row_ids(&store, &proj_shard).await;
    assert!(!ids.contains(&mem_id.to_string()), "memory must be deleted");

    // Surviving prompt's linked_memory_id cleared to NULL
    let prompt = prompts
        .get_prompt_by_id(prompt_id)
        .await
        .expect("query prompt")
        .expect("prompt still exists");
    assert_eq!(
        prompt.linked_memory_id, None,
        "surviving prompt's linked_memory_id must be cleared to None"
    );
}

#[tokio::test]
async fn migration_endpoints_answer_501_not_500() {
    let router = router_with(ApiServices::unavailable());

    let res_detect = api_request(
        router.clone(),
        "GET",
        "/api/migration/detect",
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(res_detect.status, 501, "detect_migration must answer 501");
    let val_detect: serde_json::Value = serde_json::from_str(&res_detect.body_string()).unwrap();
    assert_eq!(val_detect["success"], false);
    assert_eq!(
        val_detect["error"],
        "the endpoint is not available in this build"
    );

    let res_run = api_request(router, "POST", "/api/migration/run", Some(TOKEN), None).await;
    assert_eq!(res_run.status, 501, "run_migration must answer 501");
    let val_run: serde_json::Value = serde_json::from_str(&res_run.body_string()).unwrap();
    assert_eq!(val_run["success"], false);
    assert_eq!(
        val_run["error"],
        "the endpoint is not available in this build"
    );
}

#[tokio::test]
async fn cleanup_answers_501_when_the_port_is_unavailable() {
    let router = router_with(ApiServices::unavailable());
    let res = post_cleanup(router).await;
    assert_eq!(
        res.status, 501,
        "cleanup on unavailable port must answer 501"
    );
    assert_eq!(
        res.body_string(),
        r#"{"success":false,"error":"cleanup is not available in this build"}"#
    );
}

#[tokio::test]
async fn cleanup_answers_200_with_report_on_seeded_store() {
    let store = new_store("cleanup_answers_200");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let proj_shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let now = now_millis();
    let retention_days = 30u32;
    let retention_ms = i64::from(retention_days) * 86_400_000;
    let cutoff = now - retention_ms;

    insert_memory(
        &store.manager,
        &proj_shard,
        "mem-seeded-200",
        "content",
        PROJ_TAG,
        cutoff - 5000,
        false,
        None,
    )
    .await;

    let services =
        ApiServices::unavailable().with_maintenance(fixture_port(&store, prompts, retention_days));
    let router = router_with(services);
    let res = post_cleanup(router).await;

    assert_eq!(res.status, 200, "cleanup must answer 200");
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(val["success"], true);
    assert_eq!(val["data"]["deletedCount"], 1);
    assert_eq!(val["data"]["projectCount"], 1);
    assert_eq!(val["data"]["userCount"], 0);
    assert_eq!(val["data"]["promptsDeleted"], 0);
    assert_eq!(val["data"]["protectedMemoriesSkipped"], 0);
}

// ---------------------------------------------------------------------------
// Issue 32 tests: the dead-inbox count (reported, never purged)
// ---------------------------------------------------------------------------

/// The hermetic 8-dim episodic store config for the issue-32 cases.
fn episodic_config(root: PathBuf) -> mem_store::config::StoreConfig {
    mem_store::config::StoreConfig {
        storage_path: root,
        embedding_dimensions: 8,
        embedding_model: "test".to_string(),
        max_vectors_per_shard: 100,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

/// A fixed 8-dimension embedding (the store's dimension check passes, no
/// provider is involved).
struct FixedEmbedder;

impl Embedder for FixedEmbedder {
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(&self, _text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        Box::pin(async { Ok(vec![0.1; 8]) })
    }
}

/// A real `episodic.db` under its own temp dir, plus its inbox handle.
async fn episodic_fixture(label: &str) -> (TempDir, Arc<EpisodicStore>, CaptureInbox) {
    let dir = TempDir::new(label);
    let root = dir.path().clone();
    let store = Arc::new(EpisodicStore::new(root.clone(), episodic_config(root)));
    store.ready().await.expect("the episodic store must open");
    let inbox = CaptureInbox::new(Arc::clone(&store));
    (dir, store, inbox)
}

/// Enqueues one dead-lettered row for `(project, session, agent)`.
async fn dead_row(
    inbox: &CaptureInbox,
    capture_id: &str,
    project: &str,
    session: &str,
    agent: &str,
) {
    inbox
        .enqueue(InboxNewEntry {
            capture_id: capture_id.to_string(),
            project_id: project.to_string(),
            session_id: session.to_string(),
            agent_id: agent.to_string(),
            payload: serde_json::json!({ "transcript": "dead" }),
        })
        .await
        .expect("enqueue");
    inbox.dead_letter(capture_id).await.expect("dead-letter");
}

/// The cleanup-armed port with an injected episodic store (`None` = no store).
fn port_with_episodic(
    store: &RealStore,
    prompts: Arc<UserPromptManager>,
    retention_days: u32,
    episodic: Option<Arc<EpisodicStore>>,
) -> Arc<dyn MaintenancePort> {
    production_deduplication_port_with_cleanup(
        store.manager.clone(),
        prompts,
        retention_days,
        true,
        0.9,
        CleanupSweepConfig {
            enabled: false,
            interval_seconds: 3600,
            sweep_path: store.config.storage_path.join("cleanup-sweep.json"),
        },
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        episodic,
    )
}

/// The owner's ruling, pinned: `cleanup()` never deletes a dead staging row.
///
/// Fixture: (a) a dead row whose `(project, session, agent)` triple matches a
/// landed `agent_sessions` row, (b) a never-landed dead row.
///
/// Target mutant `triple_match_delete_added`: a `DELETE` of dead rows keyed on
/// the triple match. Assertion 1 (both rows still present) and assertion 2
/// (`Some(2)`) each fail against it — (a) is joined away and the count drops
/// to 1.
#[tokio::test]
async fn test_report_never_deletes_dead_rows() {
    let store = new_store("cleanup_never_deletes_dead");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let _shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let (_episodic_dir, episodic, inbox) = episodic_fixture("cleanup-never-delete").await;

    // (a) A landed session: the triple below matches it exactly.
    episodic
        .insert_session(
            NewAgentSession {
                project_id: "proj-landed".to_string(),
                session_id: "sess-landed".to_string(),
                agent_id: "agent-landed".to_string(),
                summary: "a landed session".to_string(),
                ..Default::default()
            },
            &FixedEmbedder,
        )
        .await
        .expect("the landed session must insert");
    dead_row(
        &inbox,
        "019a0000-0000-7f00-8000-0000000002a1",
        "proj-landed",
        "sess-landed",
        "agent-landed",
    )
    .await;
    // (b) A never-landed row: no `agent_sessions` row for its pair.
    dead_row(
        &inbox,
        "019a0000-0000-7f00-8000-0000000002a2",
        "proj-never",
        "sess-never",
        "agent-never",
    )
    .await;

    let port = port_with_episodic(&store, prompts, RETENTION_DAYS, Some(Arc::clone(&episodic)));
    let report = port.cleanup().await.expect("cleanup succeeds");

    assert_eq!(
        inbox.count_dead().await.expect("count dead"),
        2,
        "assertion 1: both dead rows survive cleanup()"
    );
    assert_eq!(
        report.inbox_dead_retained,
        Some(2),
        "assertion 2: the report counts both dead rows"
    );
}

/// `queued` rows are load-bearing (D-48 enqueue-before-ack): the count and
/// the report see `dead` only.
///
/// Target mutant: the count includes `queued` rows (answers 3, not 2).
#[tokio::test]
async fn test_report_count_excludes_queued_rows() {
    let store = new_store("cleanup_count_excludes_queued");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let _shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let (_episodic_dir, episodic, inbox) = episodic_fixture("cleanup-excludes-queued").await;

    dead_row(
        &inbox,
        "019a0000-0000-7f00-8000-0000000002b1",
        "proj-q",
        "sess-q",
        "agent-q",
    )
    .await;
    dead_row(
        &inbox,
        "019a0000-0000-7f00-8000-0000000002b2",
        "proj-q",
        "sess-q",
        "agent-q",
    )
    .await;
    inbox
        .enqueue(InboxNewEntry {
            capture_id: "019a0000-0000-7f00-8000-0000000002b3".to_string(),
            project_id: "proj-q".to_string(),
            session_id: "sess-q".to_string(),
            agent_id: "agent-q".to_string(),
            payload: serde_json::json!({ "transcript": "queued" }),
        })
        .await
        .expect("enqueue the queued row");

    let port = port_with_episodic(&store, prompts, RETENTION_DAYS, Some(Arc::clone(&episodic)));
    let report = port.cleanup().await.expect("cleanup succeeds");

    assert_eq!(
        report.inbox_dead_retained,
        Some(2),
        "only the two dead rows are counted"
    );
    assert_eq!(
        inbox.queued_count().await.expect("count queued"),
        1,
        "the queued row is untouched"
    );
}

/// R7: an absent store is `null` (unknown), never `0` (a false true-zero).
///
/// Target mutant: an absent store reported as `Some(0)`, which serialises as
/// `0` where the contract says `null`.
#[tokio::test]
async fn test_report_absent_store_is_null_not_zero() {
    let store = new_store("cleanup_absent_store");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let _shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;

    let port = port_with_episodic(&store, prompts, RETENTION_DAYS, None);
    let report = port.cleanup().await.expect("cleanup succeeds");
    assert_eq!(
        report.inbox_dead_retained, None,
        "an absent store is unknown, not zero"
    );

    let services = ApiServices::unavailable().with_maintenance(port_with_episodic(
        &store,
        Arc::new(UserPromptManager::new(
            store.config.storage_path.clone(),
            store.config.clone(),
            3,
        )),
        RETENTION_DAYS,
        None,
    ));
    let res = post_cleanup(router_with(services)).await;
    assert_eq!(res.status, 200, "cleanup must answer 200");
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert!(
        val["data"]["inboxDeadRetained"].is_null(),
        "the wire carries null, not 0: {}",
        res.body_string()
    );
}

/// N6: a count failure fails the run; it is never folded into `None`
/// (which means "store absent").
///
/// Target mutant: the count error is swallowed into `inbox_dead_retained:
/// None`, so `cleanup()` answers `Ok`.
#[tokio::test]
async fn test_report_count_failure_is_run_failure_not_null() {
    let store = new_store("cleanup_count_failure");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let _shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;

    // A store whose root cannot be created: the count's `ready()` fails.
    let dir = TempDir::new("cleanup-count-failure");
    let blocker = dir.path().join("blocked");
    std::fs::write(&blocker, b"not a directory").expect("write the blocker file");
    let broken_root = blocker.join("nested");
    let broken = Arc::new(EpisodicStore::new(
        broken_root.clone(),
        episodic_config(broken_root),
    ));

    let port = port_with_episodic(&store, prompts, RETENTION_DAYS, Some(broken));
    match port.cleanup().await {
        Ok(report) => panic!("a count failure must fail the run, not answer a report: {report:?}"),
        Err(error) => assert!(
            matches!(error, PortError::Store(_)),
            "the count error surfaces as a store failure: {error:?}"
        ),
    }
}

/// The wire key is camelCase on `POST /api/cleanup`.
///
/// Target mutant: the key is serialised snake_case or omitted.
#[tokio::test]
async fn test_cleanup_dto_emits_inbox_dead_retained_camelcase() {
    let store = new_store("cleanup_camelcase");
    let prompts = Arc::new(UserPromptManager::new(
        store.config.storage_path.clone(),
        store.config.clone(),
        3,
    ));
    let _shard = create_shard(&store, Scope::Project, PROJ_HASH, 0).await;
    let (_episodic_dir, episodic, inbox) = episodic_fixture("cleanup-camelcase").await;
    dead_row(
        &inbox,
        "019a0000-0000-7f00-8000-0000000002c1",
        "proj-w",
        "sess-w",
        "agent-w",
    )
    .await;
    dead_row(
        &inbox,
        "019a0000-0000-7f00-8000-0000000002c2",
        "proj-w",
        "sess-w",
        "agent-w",
    )
    .await;

    let services = ApiServices::unavailable().with_maintenance(port_with_episodic(
        &store,
        prompts,
        RETENTION_DAYS,
        Some(episodic),
    ));
    let res = post_cleanup(router_with(services)).await;
    assert_eq!(res.status, 200, "cleanup must answer 200");
    let val: serde_json::Value = serde_json::from_str(&res.body_string()).unwrap();
    assert_eq!(
        val["data"]["inboxDeadRetained"],
        2,
        "the camelCase key carries the dead-row count: {}",
        res.body_string()
    );
}
