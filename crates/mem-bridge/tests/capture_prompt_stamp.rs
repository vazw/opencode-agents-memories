//! Tests for issue 39: capture-time retention stamp (`source_prompt_created_at`).
//!
//! Anti-zero-writers contract (D2): `write_capture_summary` is the sole production
//! writer of `source_prompt_created_at`.

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, TestDir};
use mem_bridge::capture::{
    CaptureSummaryInput, PromptStampSource, Tier1WriteOutcome, capture_summary_row_id,
    write_capture_summary,
};
use mem_bridge::composition::ProductionPromptStampSource;
use mem_embed::embedder::BoxFuture;
use mem_profile::error::ProfileError;
use mem_profile::prompt::UserPromptManager;
use mem_store::db::Value;
use mem_store::scope::Scope;

const TAG: &str = "opencode_project_a1b2c3d4e5f60001";
const HASH: &str = "a1b2c3d4e5f60001";

struct Fixture {
    _dir: TestDir,
    shards: Arc<mem_store::shard_manager::ShardManager>,
    prompts: Arc<UserPromptManager>,
    scope: mem_bridge::project_scope::ResolvedProjectScope,
}

async fn fixture(label: &str) -> Fixture {
    let dir = TestDir::new(&format!("prompt-stamp-{label}"));
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let prompts = Arc::new(UserPromptManager::new(
        dir.path().to_path_buf(),
        dir.store_config(),
        3,
    ));
    let scope = mem_bridge::project_scope::apply_resolved_scope("/repo/test", TAG, HASH)
        .expect("resolved scope");
    Fixture {
        _dir: dir,
        shards,
        prompts,
        scope,
    }
}

async fn seed_prompt(prompts: &UserPromptManager, prompt_id: &str, created_at: i64) {
    let db = prompts.ready().await.expect("ready prompts db");
    db.execute(
        "INSERT INTO user_prompts (id, session_id, message_id, project_path, content, created_at)
         VALUES (?1, 'sess-1', 'msg-1', '/repo/test', 'prompt text', ?2)",
        vec![
            Value::Text(prompt_id.to_string()),
            Value::Integer(created_at),
        ],
    )
    .await
    .expect("seed prompt row");
}

fn make_input(capture_id: &str, prompt_id: Option<&str>) -> CaptureSummaryInput {
    CaptureSummaryInput {
        capture_id: capture_id.to_string(),
        session_id: "session-1".to_string(),
        summary: "Captured memory summary".to_string(),
        memory_type: "bug".to_string(),
        tags: vec!["auth".to_string()],
        prompt_id: prompt_id.map(str::to_string),
        source_prompt_created_at: None,
    }
}

#[tokio::test]
async fn stamp_is_prompt_row_created_at() {
    let fix = fixture("created-at").await;
    const PROMPT_BIRTH: i64 = 1_700_000_123_456;
    seed_prompt(&fix.prompts, "prompt-birth-1", PROMPT_BIRTH).await;

    let stamp_source = Arc::new(ProductionPromptStampSource::new(fix.prompts.clone()));
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: fix.shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        prompt_stamps: stamp_source,
    };

    let input = make_input("cap-1", Some("prompt-birth-1"));
    let outcome = write_capture_summary(&deps, &fix.scope, &input)
        .await
        .expect("write succeeds");
    assert_eq!(outcome, Tier1WriteOutcome::Written);

    let row_id = capture_summary_row_id(&fix.scope.container_tag, "cap-1");
    let shard = fix
        .shards
        .get_write_shard_allowing_own_lock(Scope::Project, &fix.scope.scope_hash)
        .await
        .expect("shard exists");
    let db = fix
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");

    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at, (source_prompt_created_at IS NULL) AS is_null
         FROM memories WHERE id = ?",
        vec![Value::Text(row_id)],
    )
    .await
    .expect("query row")
    .expect("row exists");

    assert_eq!(
        row.i64("source_prompt_created_at"),
        Some(PROMPT_BIRTH),
        "stamp matches the prompt row's created_at exactly"
    );
    assert_eq!(row.i64("is_null"), Some(0));
}

#[tokio::test]
async fn stamp_is_null_when_prompt_row_absent() {
    let fix = fixture("absent-prompt").await;

    let stamp_source = Arc::new(ProductionPromptStampSource::new(fix.prompts.clone()));
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: fix.shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        prompt_stamps: stamp_source,
    };

    let input = make_input("cap-absent", Some("non-existent-prompt-id"));
    let outcome = write_capture_summary(&deps, &fix.scope, &input)
        .await
        .expect("write succeeds");
    assert_eq!(outcome, Tier1WriteOutcome::Written);

    let row_id = capture_summary_row_id(&fix.scope.container_tag, "cap-absent");
    let shard = fix
        .shards
        .get_write_shard_allowing_own_lock(Scope::Project, &fix.scope.scope_hash)
        .await
        .expect("shard exists");
    let db = fix
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");

    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at, (source_prompt_created_at IS NULL) AS is_null
         FROM memories WHERE id = ?",
        vec![Value::Text(row_id)],
    )
    .await
    .expect("query row")
    .expect("row exists");

    assert_eq!(
        row.get("source_prompt_created_at"),
        Some(&Value::Null),
        "absence must store SQL NULL, never 0"
    );
    assert_eq!(row.i64("source_prompt_created_at"), None);
    assert_eq!(row.i64("is_null"), Some(1));
}

#[tokio::test]
async fn stamp_survives_replayed_capture() {
    let fix = fixture("replay").await;
    const PROMPT_BIRTH: i64 = 1_700_000_999_888;
    seed_prompt(&fix.prompts, "prompt-replay", PROMPT_BIRTH).await;

    let stamp_source = Arc::new(ProductionPromptStampSource::new(fix.prompts.clone()));
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: fix.shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        prompt_stamps: stamp_source,
    };

    let input = make_input("cap-replay", Some("prompt-replay"));
    let outcome1 = write_capture_summary(&deps, &fix.scope, &input)
        .await
        .expect("first write succeeds");
    assert_eq!(outcome1, Tier1WriteOutcome::Written);

    let shard = fix
        .shards
        .get_write_shard_allowing_own_lock(Scope::Project, &fix.scope.scope_hash)
        .await
        .expect("shard exists");
    let db = fix
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");
    let count_before = mem_store::shard_facts::read_stored_count(db.as_ref())
        .await
        .expect("read count");
    assert_eq!(count_before, 1);

    // Second write with the exact same deterministic id
    let outcome2 = write_capture_summary(&deps, &fix.scope, &input)
        .await
        .expect("second write succeeds");
    assert_eq!(
        outcome2,
        Tier1WriteOutcome::Replayed,
        "second run returns Replayed"
    );

    let count_after = mem_store::shard_facts::read_stored_count(db.as_ref())
        .await
        .expect("read count");
    assert_eq!(count_after, 1, "shard count unchanged on replay");

    let row_id = capture_summary_row_id(&fix.scope.container_tag, "cap-replay");
    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at FROM memories WHERE id = ?",
        vec![Value::Text(row_id)],
    )
    .await
    .expect("query row")
    .expect("row exists");

    assert_eq!(
        row.i64("source_prompt_created_at"),
        Some(PROMPT_BIRTH),
        "stamp is preserved byte-identically on replay"
    );
}

struct FaultyPromptStampSource;

impl PromptStampSource for FaultyPromptStampSource {
    fn prompt_created_at<'a>(
        &'a self,
        _prompt_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<i64>, ProfileError>> {
        Box::pin(std::future::ready(Err(ProfileError::Save(
            "injected fault for testing".to_string(),
        ))))
    }
}

#[tokio::test]
async fn failed_prompt_lookup_degrades_to_null_stamp_without_failing_capture() {
    let fix = fixture("fault-degrade").await;

    let stamp_source = Arc::new(FaultyPromptStampSource);
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: fix.shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        prompt_stamps: stamp_source,
    };

    let input = make_input("cap-fault", Some("any-prompt-id"));
    let outcome = write_capture_summary(&deps, &fix.scope, &input)
        .await
        .expect("capture must succeed despite lookup fault");
    assert_eq!(
        outcome,
        Tier1WriteOutcome::Written,
        "lookup failure must not consume or fail the capture attempt"
    );

    let row_id = capture_summary_row_id(&fix.scope.container_tag, "cap-fault");
    let shard = fix
        .shards
        .get_write_shard_allowing_own_lock(Scope::Project, &fix.scope.scope_hash)
        .await
        .expect("shard exists");
    let db = fix
        .shards
        .open_shard(&shard.db_path)
        .await
        .expect("open shard");

    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT source_prompt_created_at, (source_prompt_created_at IS NULL) AS is_null
         FROM memories WHERE id = ?",
        vec![Value::Text(row_id)],
    )
    .await
    .expect("query row")
    .expect("row exists");

    assert_eq!(
        row.get("source_prompt_created_at"),
        Some(&Value::Null),
        "degraded capture persists SQL NULL"
    );
    assert_eq!(row.i64("source_prompt_created_at"), None);
    assert_eq!(row.i64("is_null"), Some(1));
}
