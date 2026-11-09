//! `GET /api/stats` — the upstream v2.33 contract (upstream
//! `tests/stats-service.test.ts` + `api-handlers.ts` `handleStats`).
//!
//! Every case drives the router seam over a `FakeBackend` wired as the ONLY
//! live port (`ApiServices::unavailable().with_memory(..)`), so the profile
//! section behaves like upstream test #1's missing profile DB (`{exists:false}`)
//! unless a case explicitly wires a profile port. Temp dirs live under
//! `std::env::temp_dir()` (TMPDIR-redirected into `.tmp/scratch`); the process
//! environment is never mutated.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::TempDir;
use common::fakes::{PROJECT_HASH, USER_HASH, USER_ID, USER_TAG, fake_services, memory_row};
use common::{api_request, app_state_with_api, throwaway_log_dir};
use mem_core::scope::Scope;
use mem_web::api::ports::{
    ApiServices, MemoryPort, MemoryRow, MemoryStatsRow, MemoryUpdate, NewMemory, PortError,
    SearchHit, ShardRef, TagRow,
};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;
use serde_json::Value;

/// The configured API token.
const API_TOKEN: &str = "test-token";

/// The seeded project container tag (carries the `_project_` marker).
const PROJECT_TAG: &str = "mem_project_bbbbbbbbbbbbbbbb";

/// One fixture memory row: `type: "context"`, exact tags, exact created_at.
fn row(id: &str, container_tag: &str, created_at: i64, tags: &str) -> MemoryRow {
    let mut row = memory_row(id, container_tag);
    row.created_at = created_at;
    row.updated_at = Some(created_at);
    row.tags = Some(tags.to_string());
    row.memory_type = Some("context".to_string());
    row
}

/// A router over `backend` wired as the memory port only, with the given log
/// directory for the daily scan.
fn stats_router(backend: Arc<common::fakes::FakeBackend>, log_dir: PathBuf) -> axum::Router {
    let services = ApiServices::unavailable().with_memory(backend);
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        log_dir,
        services,
        USER_ID,
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

/// `GET /api/stats` and its parsed envelope.
async fn get_stats(router: axum::Router, query: &str) -> (u16, Value) {
    let response = api_request(
        router,
        "GET",
        &format!("/api/stats{query}"),
        Some(API_TOKEN),
        None,
    )
    .await;
    let body: Value = serde_json::from_str(&response.body_string())
        .unwrap_or_else(|_| panic!("the stats answer is JSON: {}", response.body_string()));
    (response.status, body)
}

/// A local date key `offset` days before today (the handler scans local dates
/// ending at its own `Local::now()`).
fn day_key(offset: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

/// One structured-logger line, upstream `timestamp(date, hour)` shape.
fn log_line(date: &str, hour: &str, content: &str) -> String {
    format!("[{date}T{hour}+00:00] {content}\n")
}

#[tokio::test]
async fn test_stats_aggregates_shards_and_returns_zero_filled_daily_log_counters() {
    let dir = TempDir::new("stats-contract");
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).expect("the log dir is creatable");

    let (backend, _) = fake_services();

    // Back both seeded shards with real (empty) db files and the upstream
    // vector counts — BEFORE taking shard refs, so seeded rows carry the
    // final (repointed) ShardRef identity.
    let user_db = dir.path().join("user_aaaaaaaaaaaaaaaa_shard_0.db");
    let project_db = dir.path().join("project_bbbbbbbbbbbbbbbb_shard_0.db");
    std::fs::write(&user_db, b"").expect("the user db file is writable");
    std::fs::write(&project_db, b"").expect("the project db file is writable");
    backend.set_shard_db_path(Scope::User, USER_HASH, user_db);
    backend.set_shard_db_path(Scope::Project, PROJECT_HASH, project_db);
    backend.set_shard_meta(Scope::User, USER_HASH, 2, true);
    backend.set_shard_meta(Scope::Project, PROJECT_HASH, 1, true);

    let user = backend.user_shard();
    let project = backend.project_shard();

    // Make room for the upstream fixture rows (the canonical W6.2 seeds have
    // other types and tags).
    let _ = backend
        .delete_memory(&user, common::fakes::USER_MEMORY_ID)
        .await;
    let _ = backend
        .delete_memory(&project, common::fakes::MEMORY_ID)
        .await;

    use chrono::TimeZone as _;
    let first_created_at = chrono::Utc
        .with_ymd_and_hms(2026, 10, 12, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    let newest_created_at = chrono::Utc
        .with_ymd_and_hms(2026, 10, 16, 0, 0, 0)
        .unwrap()
        .timestamp_millis();

    backend.seed_memory(
        &user,
        row("user-1", USER_TAG, first_created_at, "shared-tag"),
    );
    backend.seed_memory(
        &user,
        row("user-2", USER_TAG, newest_created_at, "shared-tag"),
    );
    backend.seed_memory(
        &project,
        row("project-1", PROJECT_TAG, newest_created_at, "project-tag"),
    );

    // The log fixture of upstream test #1, with dates relative to local today.
    let (day_1, day_3, day_4) = (day_key(1), day_key(3), day_key(4));
    std::fs::write(
        log_dir.join("opencode-mem-2026-10-01.log"),
        format!(
            "{}{}{}",
            log_line(
                &day_1,
                "10:00:00.000",
                "Auto-capture memory persisted: {\"id\":\"1\"}"
            ),
            log_line(
                &day_1,
                "10:01:00.000",
                "Auto-capture skipped: {\"reason\":\"short\"}"
            ),
            log_line(
                &day_1,
                "10:02:00.000",
                "listMemories: error: {\"error\":\"fixture\"}"
            ),
        ),
    )
    .expect("the dated archive is writable");
    std::fs::write(
        log_dir.join("opencode-mem.log.old"),
        log_line(
            &day_3,
            "11:00:00.000",
            "Compaction memory injected: {\"count\":1}",
        ),
    )
    .expect("the .old log is writable");
    std::fs::write(
        log_dir.join("opencode-mem.log"),
        log_line(&day_4, "09:00:00.000", "profile inject: {\"prefs\":[]}"),
    )
    .expect("the current log is writable");

    let (status, body) = get_stats(stats_router(backend, log_dir), "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], Value::Bool(true));
    let data = &body["data"];

    assert_eq!(data["total"], 3, "{data}");
    assert_eq!(data["byScope"]["user"], 2);
    assert_eq!(data["byScope"]["project"], 1);
    assert_eq!(data["byType"]["context"], 3);
    let generated_at = data["generatedAt"]
        .as_str()
        .expect("generatedAt is a string");
    assert!(
        generated_at.len() >= 11
            && generated_at.as_bytes()[4] == b'-'
            && generated_at.as_bytes()[10] == b'T',
        "generatedAt matches the ISO prefix: {generated_at}"
    );
    assert_eq!(data["profile"], serde_json::json!({ "exists": false }));

    // Per-shard stats.
    let shards = data["shards"].as_array().expect("shards is an array");
    assert_eq!(shards.len(), 2, "{data}");
    let user_stats = shards
        .iter()
        .find(|shard| shard["scope"] == "user")
        .expect("the user shard is present");
    let project_stats = shards
        .iter()
        .find(|shard| shard["scope"] == "project")
        .expect("the project shard is present");

    assert_eq!(user_stats["fileName"], "user_aaaaaaaaaaaaaaaa_shard_0.db");
    assert_eq!(user_stats["fileExists"], Value::Bool(true));
    assert_eq!(user_stats["registeredVectorCount"], 2);
    assert_eq!(user_stats["memoryCount"], 2);
    assert_eq!(user_stats["oldestCreatedAt"], "2026-10-12T00:00:00.000Z");
    assert_eq!(user_stats["newestCreatedAt"], "2026-10-16T00:00:00.000Z");
    assert_eq!(user_stats["duplicateTagCount"], 1);

    assert_eq!(project_stats["fileExists"], Value::Bool(true));
    assert_eq!(project_stats["registeredVectorCount"], 1);
    assert_eq!(project_stats["memoryCount"], 1);
    assert_eq!(project_stats["oldestCreatedAt"], "2026-10-16T00:00:00.000Z");
    assert_eq!(project_stats["newestCreatedAt"], "2026-10-16T00:00:00.000Z");
    assert_eq!(project_stats["duplicateTagCount"], 0);

    // The 14-day zero-filled daily series.
    let daily = data["daily"].as_array().expect("daily is an array");
    assert_eq!(daily.len(), 14, "{data}");
    let expected_dates: Vec<String> = (0..14).rev().map(day_key).collect();
    let dates: Vec<String> = daily
        .iter()
        .map(|day| day["date"].as_str().expect("a date string").to_string())
        .collect();
    assert_eq!(dates, expected_dates);

    let zero_day = &daily[0];
    assert_eq!(
        (
            zero_day["captures"].as_u64(),
            zero_day["captureSkipped"].as_u64(),
            zero_day["profileInjections"].as_u64(),
            zero_day["compactionInjections"].as_u64(),
        ),
        (Some(0), Some(0), Some(0), Some(0))
    );
    assert_eq!(zero_day["errors"], serde_json::json!({}));

    let counted = daily
        .iter()
        .find(|day| day["date"] == day_1)
        .expect("day(1) is in the window");
    assert_eq!(counted["captures"], 1);
    assert_eq!(counted["captureSkipped"], 1);
    assert_eq!(counted["errors"], serde_json::json!({ "listMemories": 1 }));

    let compaction = daily
        .iter()
        .find(|day| day["date"] == day_3)
        .expect("day(3) is in the window");
    assert_eq!(compaction["compactionInjections"], 1);

    let profile_injections = daily
        .iter()
        .find(|day| day["date"] == day_4)
        .expect("day(4) is in the window");
    assert_eq!(profile_injections["profileInjections"], 1);

    assert!(
        !serde_json::to_string(&body)
            .unwrap_or_default()
            .contains("dailyError"),
        "no dailyError key on the happy path: {body}"
    );
}

// -------------------------------------------------------------------------
// Probe (upstream test #2 / web-server.ts:577)
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_stats_probe_returns_success_envelope_without_data() {
    // No ports wired at all: the probe must answer before any shard walk or
    // log scan would touch them.
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        throwaway_log_dir(),
        ApiServices::unavailable(),
        USER_ID,
        "127.0.0.1",
    );
    let response = api_request(
        build_router(Arc::new(state)),
        "GET",
        "/api/stats?probe=1",
        Some(API_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_string(),
        r#"{"success":true}"#,
        "byte-exact upstream probe body, no data key"
    );
}

// -------------------------------------------------------------------------
// Partial-success hardening (upstream test #3)
// -------------------------------------------------------------------------

/// A memory port whose project-shard walk fails while the user shard answers
/// normally (upstream test #3's monkey-patched `getConnection`).
struct FailingShardPort {
    shards: Vec<ShardRef>,
    user_rows: Vec<MemoryRow>,
}

#[async_trait::async_trait]
impl MemoryPort for FailingShardPort {
    async fn shards(
        &self,
        _scope: Option<Scope>,
        _scope_hash: Option<&str>,
    ) -> Result<Vec<ShardRef>, PortError> {
        Ok(self.shards.clone())
    }

    async fn verify_shards(&self, _shards: &[ShardRef]) -> Result<(), PortError> {
        Ok(())
    }

    async fn all_memories(&self, shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError> {
        if shard.scope == Scope::User {
            Ok(self.user_rows.clone())
        } else {
            Err(PortError::Store("project walk failed".to_string()))
        }
    }

    async fn list_memories_by_container_tag(
        &self,
        _shard: &ShardRef,
        _container_tag: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn memory_by_id(
        &self,
        _shard: &ShardRef,
        _id: &str,
    ) -> Result<Option<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn distinct_tags(&self, _shard: &ShardRef) -> Result<Vec<TagRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn insert_memory(
        &self,
        _scope: Scope,
        _scope_hash: &str,
        _memory: NewMemory,
    ) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn update_memory(
        &self,
        _shard: &ShardRef,
        _update: MemoryUpdate,
    ) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn pin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn unpin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn search_in_shard(
        &self,
        _shard: &ShardRef,
        _query_vector: &[f32],
        _container_tag: &str,
        _limit: usize,
        _query_text: Option<&str>,
    ) -> Result<Vec<SearchHit>, PortError> {
        Err(PortError::Unavailable)
    }
}

#[tokio::test]
async fn test_stats_excludes_a_failed_shard_from_totals_with_per_shard_error() {
    let dir = TempDir::new("stats-partial");

    // Both db files exist, so both walks are attempted; only the project
    // walk fails.
    let user_db = dir.path().join("user_aaaaaaaaaaaaaaaa_shard_0.db");
    let project_db = dir.path().join("project_bbbbbbbbbbbbbbbb_shard_0.db");
    std::fs::write(&user_db, b"").expect("the user db file is writable");
    std::fs::write(&project_db, b"").expect("the project db file is writable");

    let mut user_row = memory_row("user-1", "opencode_user_aaaaaaaaaaaaaaaa");
    user_row.created_at = 1_791_763_200_000;
    user_row.tags = Some("kept".to_string());

    let port = FailingShardPort {
        shards: vec![
            ShardRef {
                scope: Scope::Project,
                scope_hash: "bbbbbbbbbbbbbbbb".to_string(),
                shard_index: 0,
                db_path: project_db,
                shard_id: Some(2),
                vector_count: 4,
                is_active: true,
            },
            ShardRef {
                scope: Scope::User,
                scope_hash: "aaaaaaaaaaaaaaaa".to_string(),
                shard_index: 0,
                db_path: user_db,
                shard_id: Some(1),
                vector_count: 2,
                is_active: true,
            },
        ],
        user_rows: vec![user_row.clone(), {
            let mut second = user_row;
            second.id = "user-2".to_string();
            second.created_at = 1_792_108_800_000;
            second
        }],
    };

    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        throwaway_log_dir(),
        ApiServices::unavailable().with_memory(Arc::new(port)),
        USER_ID,
        "127.0.0.1",
    );

    let (status, body) = get_stats(build_router(Arc::new(state)), "").await;
    assert_eq!(status, 200, "{body}");
    let data = &body["data"];

    // Totals from the healthy shard only.
    assert_eq!(data["total"], 2, "{data}");
    assert_eq!(data["byScope"]["user"], 2);
    assert_eq!(data["byScope"]["project"], 0);
    assert_eq!(data["byType"], serde_json::json!({}));

    let shards = data["shards"].as_array().expect("shards is an array");
    let project = shards
        .iter()
        .find(|shard| shard["scope"] == "project")
        .expect("the project shard is present");
    assert_eq!(project["memoryCount"], 0);
    assert_eq!(project["duplicateTagCount"], 0);
    assert_eq!(project["oldestCreatedAt"], Value::Null);
    assert_eq!(project["newestCreatedAt"], Value::Null);
    assert_eq!(
        project["error"], "shard query failed",
        "the fixed D-9 text, never the store message"
    );

    let user = shards
        .iter()
        .find(|shard| shard["scope"] == "user")
        .expect("the user shard is present");
    assert_eq!(user["memoryCount"], 2);
    assert_eq!(user["duplicateTagCount"], 1);
    assert!(
        user["error"].is_null(),
        "the healthy shard carries no error"
    );
}

// -------------------------------------------------------------------------
// Missing shard file (upstream `Shard DB file missing: ${fileName}`)
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_stats_missing_shard_file_sets_file_name_only_error_and_succeeds() {
    let dir = TempDir::new("stats-missing-file");
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).expect("the log dir is creatable");

    let (backend, _) = fake_services();

    // The user shard is healthy (real file + one memory); the added project
    // shard keeps its default db_path, which never exists on disk.
    let user_db = dir.path().join("user_aaaaaaaaaaaaaaaa_shard_0.db");
    std::fs::write(&user_db, b"").expect("the user db file is writable");
    backend.set_shard_db_path(Scope::User, USER_HASH, user_db);
    backend.set_shard_meta(Scope::User, USER_HASH, 1, true);
    let user = backend.user_shard();
    backend.seed_memory(&user, row("user-1", USER_TAG, 1_791_763_200_000, "solo"));

    let dead = backend.add_shard(Scope::Project, "dddd1111eeee2222");

    let (status, body) = get_stats(stats_router(backend, log_dir), "").await;
    assert_eq!(status, 200, "{body}");
    let data = &body["data"];

    let dead_stats = data["shards"]
        .as_array()
        .expect("shards is an array")
        .iter()
        .find(|shard| shard["scopeHash"] == "dddd1111eeee2222")
        .expect("the file-less shard is present");
    assert_eq!(dead_stats["fileExists"], Value::Bool(false));
    assert_eq!(
        dead_stats["error"], "Shard DB file missing: fake-dddd1111eeee2222.db",
        "the verbatim upstream text carrying fileName only"
    );
    let error_text = dead_stats["error"].as_str().expect("an error string");
    assert!(!error_text.contains('/'), "never a path: {error_text}");
    assert_eq!(dead_stats["memoryCount"], 0);

    // The healthy shard is unaffected and the request still succeeds.
    let user_stats = data["shards"]
        .as_array()
        .expect("shards is an array")
        .iter()
        .find(|shard| shard["scope"] == "user")
        .expect("the user shard is present");
    assert_eq!(user_stats["memoryCount"], 1);
    assert_eq!(user_stats["fileExists"], Value::Bool(true));
    assert!(user_stats["error"].is_null());
    assert_eq!(data["total"], 1);

    // The dead shard ref is exercised so the fixture cannot silently rot.
    assert_eq!(dead.scope, Scope::Project);
}

#[tokio::test]
async fn test_stats_order_independent_timestamp_and_tag_accumulation() {
    let dir = TempDir::new("stats-order");
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).expect("the log dir is creatable");

    let (backend, _) = fake_services();

    let user_db = dir.path().join("user_aaaaaaaaaaaaaaaa_shard_0.db");
    let project_db = dir.path().join("project_bbbbbbbbbbbbbbbb_shard_0.db");
    std::fs::write(&user_db, b"").expect("the user db file is writable");
    std::fs::write(&project_db, b"").expect("the project db file is writable");
    backend.set_shard_db_path(Scope::User, USER_HASH, user_db);
    backend.set_shard_db_path(Scope::Project, PROJECT_HASH, project_db);
    backend.set_shard_meta(Scope::User, USER_HASH, 3, true);
    backend.set_shard_meta(Scope::Project, PROJECT_HASH, 0, true);

    let user = backend.user_shard();

    // Out-of-order timestamps: 1500, 3000, 1000
    backend.seed_memory(&user, row("m1", USER_TAG, 1500, "shared-tag"));
    backend.seed_memory(&user, row("m2", USER_TAG, 3000, "shared-tag"));
    backend.seed_memory(&user, row("m3", USER_TAG, 1000, "other-tag"));

    let (status, body) = get_stats(stats_router(backend, log_dir), "").await;
    assert_eq!(status, 200, "{body}");
    let data = &body["data"];

    let user_stats = data["shards"]
        .as_array()
        .expect("shards is an array")
        .iter()
        .find(|shard| shard["scope"] == "user")
        .expect("the user shard is present");
    assert_eq!(user_stats["memoryCount"], 3);
    assert_eq!(user_stats["duplicateTagCount"], 1);
    assert_eq!(user_stats["oldestCreatedAt"], "1970-01-01T00:00:01.000Z");
    assert_eq!(user_stats["newestCreatedAt"], "1970-01-01T00:00:03.000Z");
}

/// A port whose shard listing fails — the stats handler must answer the fixed
/// 500 instead of an empty-total success.
struct FailingListingPort;

#[async_trait::async_trait]
impl MemoryPort for FailingListingPort {
    async fn shards(
        &self,
        _scope: Option<Scope>,
        _scope_hash: Option<&str>,
    ) -> Result<Vec<ShardRef>, PortError> {
        Err(PortError::Store("listing failed".to_string()))
    }

    async fn verify_shards(&self, _shards: &[ShardRef]) -> Result<(), PortError> {
        Ok(())
    }

    async fn all_memories(&self, _shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn list_memories_by_container_tag(
        &self,
        _shard: &ShardRef,
        _container_tag: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn memory_by_id(
        &self,
        _shard: &ShardRef,
        _id: &str,
    ) -> Result<Option<MemoryRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn distinct_tags(&self, _shard: &ShardRef) -> Result<Vec<TagRow>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn insert_memory(
        &self,
        _scope: Scope,
        _scope_hash: &str,
        _memory: NewMemory,
    ) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn update_memory(
        &self,
        _shard: &ShardRef,
        _update: MemoryUpdate,
    ) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn pin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn unpin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn search_in_shard(
        &self,
        _shard: &ShardRef,
        _query_vector: &[f32],
        _container_tag: &str,
        _limit: usize,
        _query_text: Option<&str>,
    ) -> Result<Vec<SearchHit>, PortError> {
        Err(PortError::Unavailable)
    }
}

#[tokio::test]
async fn test_stats_shard_listing_failure_is_a_hard_error() {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        throwaway_log_dir(),
        ApiServices::unavailable().with_memory(Arc::new(FailingListingPort)),
        USER_ID,
        "127.0.0.1",
    );
    let (status, body) = get_stats(build_router(Arc::new(state)), "").await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(
        body,
        serde_json::json!({ "success": false, "error": "Internal Server Error" }),
        "the fixed D-9 internal envelope, never an empty-total success"
    );
}

// -------------------------------------------------------------------------
// Profile section (upstream `collectProfileStats`)
// -------------------------------------------------------------------------

/// A router over the full fake bundle (memory + profile both live, so the
/// shard listing succeeds and the profile section is driven purely by the
/// seeded `ProfileRecord`).
fn profile_router(services: ApiServices) -> axum::Router {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(API_TOKEN),
        None,
        std::path::PathBuf::from("/tmp/unused-dist"),
        throwaway_log_dir(),
        services,
        USER_ID,
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

#[tokio::test]
async fn test_stats_profile_section_counts_profile_arrays() {
    use serde_json::json;

    // Case 1: valid profileData — exact array counts plus the identity fields.
    {
        let (backend, services) = fake_services();
        backend.set_profile_data(json!({
            "preferences": [{"id": "p1"}, {"id": "p2"}],
            "patterns": [{"id": "q1"}],
            "workflows": []
        }));
        let (status, body) = get_stats(profile_router(services), "").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["data"]["profile"],
            json!({
                "exists": true,
                "version": 3,
                "counts": {"preferences": 2, "patterns": 1, "workflows": 0},
                "lastAnalyzedAt": "2023-11-14T22:13:20.000Z",
                "totalPromptsAnalyzed": 12
            }),
            "{body}"
        );
        assert!(
            !body["data"]["profile"]
                .as_object()
                .expect("an object")
                .contains_key("error"),
            "a valid profile carries no error key"
        );
    }

    // Case 2: broken profileData — `exists:true` plus an error, no counts.
    {
        let (backend, services) = fake_services();
        backend.set_profile_raw("not json {".to_string());
        let (status, body) = get_stats(profile_router(services), "").await;
        assert_eq!(status, 200, "{body}");
        let profile = &body["data"]["profile"];
        assert_eq!(profile["exists"], Value::Bool(true));
        assert!(
            profile["error"].is_string(),
            "the broken payload surfaces its error: {profile}"
        );
        assert!(profile["counts"].is_null(), "no counts without a parse");
    }

    // Case 3: no profile — exactly `{exists:false}`.
    {
        let (backend, services) = fake_services();
        backend.clear_profile();
        let (status, body) = get_stats(profile_router(services), "").await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["data"]["profile"],
            json!({ "exists": false }),
            "the unseeded profile is bare {{exists:false}}: {body}"
        );
    }
}

#[tokio::test]
async fn test_stats_uses_narrow_projection_seam() {
    let dir = TempDir::new("stats-seam");
    let (backend, _) = common::fakes::fake_services();
    let user_db = dir.path().join("user_c1c2c3d4e5f67890_shard_0.db");
    let project_db = dir.path().join("project_c1c2c3d4e5f67890_shard_0.db");
    std::fs::write(&user_db, b"").expect("the user db file is writable");
    std::fs::write(&project_db, b"").expect("the project db file is writable");
    backend.set_shard_db_path(Scope::User, USER_HASH, user_db);
    backend.set_shard_db_path(Scope::Project, PROJECT_HASH, project_db);

    let log_dir = throwaway_log_dir();
    let router = stats_router(backend.clone(), log_dir);
    let (_status, _body) = get_stats(router, "").await;
    let calls = backend.calls();
    assert!(
        calls
            .iter()
            .any(|c| c == &format!("stats_memories:{}", PROJECT_HASH)),
        "stats_memories must be called: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|c| c == &format!("all_memories:{}", PROJECT_HASH)),
        "all_memories must NOT be called: {calls:?}"
    );
}

#[tokio::test]
async fn test_stats_memories_default_fallback() {
    // A port that does NOT override stats_memories: it must delegate to
    // all_memories and project into MemoryStatsRow.
    struct DefaultPort {
        rows: Vec<MemoryRow>,
    }
    #[async_trait::async_trait]
    impl MemoryPort for DefaultPort {
        async fn shards(
            &self,
            _scope: Option<Scope>,
            _scope_hash: Option<&str>,
        ) -> Result<Vec<ShardRef>, PortError> {
            Ok(Vec::new())
        }
        async fn verify_shards(&self, _shards: &[ShardRef]) -> Result<(), PortError> {
            Ok(())
        }
        async fn all_memories(&self, _shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError> {
            Ok(self.rows.clone())
        }
        async fn list_memories_by_container_tag(
            &self,
            _shard: &ShardRef,
            _container_tag: &str,
            _limit: usize,
        ) -> Result<Vec<MemoryRow>, PortError> {
            Ok(Vec::new())
        }
        async fn memory_by_id(
            &self,
            _shard: &ShardRef,
            _id: &str,
        ) -> Result<Option<MemoryRow>, PortError> {
            Ok(None)
        }
        async fn distinct_tags(&self, _shard: &ShardRef) -> Result<Vec<TagRow>, PortError> {
            Ok(Vec::new())
        }
        async fn insert_memory(
            &self,
            _scope: Scope,
            _scope_hash: &str,
            _memory: NewMemory,
        ) -> Result<(), PortError> {
            Ok(())
        }
        async fn update_memory(
            &self,
            _shard: &ShardRef,
            _update: MemoryUpdate,
        ) -> Result<(), PortError> {
            Ok(())
        }
        async fn delete_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
            Ok(())
        }
        async fn pin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
            Ok(())
        }
        async fn unpin_memory(&self, _shard: &ShardRef, _id: &str) -> Result<(), PortError> {
            Ok(())
        }
        async fn search_in_shard(
            &self,
            _shard: &ShardRef,
            _query_vector: &[f32],
            _container_tag: &str,
            _limit: usize,
            _query_text: Option<&str>,
        ) -> Result<Vec<SearchHit>, PortError> {
            Ok(Vec::new())
        }
    }

    let shard = ShardRef {
        scope: Scope::Project,
        scope_hash: "aaaaaaaaaaaaaaaa".to_string(),
        shard_index: 0,
        db_path: std::path::PathBuf::from("/tmp/fake.db"),
        shard_id: None,
        vector_count: 0,
        is_active: true,
    };
    let mut r = memory_row("m1", "tag");
    r.container_tag = "tag".to_string();
    r.memory_type = Some("note".to_string());
    r.created_at = 42;
    r.tags = Some("t".to_string());
    let port = DefaultPort { rows: vec![r] };
    let out: Vec<MemoryStatsRow> = port.stats_memories(&shard).await.expect("stats");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].container_tag, "tag");
    assert_eq!(out[0].memory_type, Some("note".to_string()));
    assert_eq!(out[0].created_at, 42);
    assert_eq!(out[0].tags, Some("t".to_string()));
}

#[tokio::test]
async fn test_stats_by_type_skips_empty_kind() {
    let dir = TempDir::new("stats-empty-strings");
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).expect("the log dir is creatable");

    let (backend, _) = fake_services();
    let user_db = dir.path().join("user_aaaaaaaaaaaaaaaa_shard_0.db");
    std::fs::write(&user_db, b"").expect("the user db file is writable");
    backend.set_shard_db_path(Scope::User, USER_HASH, user_db);
    backend.set_shard_meta(Scope::User, USER_HASH, 3, true);
    let user = backend.user_shard();

    // TWO empty-tag rows, not one: a lone `tags: Some("")` contributes
    // `count - 1 == 0` to duplicateTagCount, so one row leaves the tag-filter
    // mutant alive. Both also carry an empty `type`.
    let mut empty_1 = row("empty-1", USER_TAG, 1000, "");
    empty_1.memory_type = Some(String::new());
    let mut empty_2 = row("empty-2", USER_TAG, 2000, "");
    empty_2.memory_type = Some(String::new());
    backend.seed_memory(&user, empty_1);
    backend.seed_memory(&user, empty_2);
    backend.seed_memory(&user, row("typed-1", USER_TAG, 3000, "solo"));

    let (status, body) = get_stats(stats_router(backend, log_dir), "").await;
    assert_eq!(status, 200, "{body}");
    let data = &body["data"];

    assert_eq!(data["total"], 3, "{data}");
    assert_eq!(
        data["byType"],
        serde_json::json!({ "context": 1 }),
        "a row with an empty `type` must contribute no byType key"
    );

    let user_stats = data["shards"]
        .as_array()
        .expect("shards is an array")
        .iter()
        .find(|shard| shard["scope"] == "user")
        .expect("the user shard is present");
    assert_eq!(user_stats["memoryCount"], 3, "every row still counts");
    assert_eq!(
        user_stats["duplicateTagCount"], 0,
        "two empty-string `tags` rows are not a duplicate group"
    );
    assert_eq!(user_stats["oldestCreatedAt"], "1970-01-01T00:00:01.000Z");
    assert_eq!(user_stats["newestCreatedAt"], "1970-01-01T00:00:03.000Z");
}
