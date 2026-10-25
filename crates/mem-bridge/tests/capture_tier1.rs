//! W5.3 Step 6 — the session-linked tier-1 capture-summary writer over a
//! REAL shard (blueprint Step 6; Q13; A1).
//!
//! Q13: the producing session's id is written BOTH as the explicit
//! `session_id` column AND as JSON `metadata.sessionID` (same string);
//! `get_memories_by_session_id` finds the row; a replay of the same
//! `capture_id` leaves exactly one row.
//!
//! A1: a tier-1 write resolved while THIS process holds the consolidation
//! operation lock must succeed through
//! `get_write_shard → own-pid retry → get_write_shard_allowing_own_lock`
//! (own tolerated, foreign refused).

mod common;
use std::fs;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use common::{FakeEmbedder, OwnedAndForeignProbe, TestDir};
use mem_bridge::capture::{
    CaptureSummaryInput, Tier1WriteOutcome, capture_summary_row_id, write_capture_summary,
};
use mem_bridge::project_scope::resolve_project_scope;
use mem_store::serde_json::json;
use mem_store::shard_manager::{ShardInfo, ShardManager};
use mem_store::vector::get_memories_by_session_id;

// The tag resolver fixture (real git repo, hermetic temp dir).
fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("tier-1 tests require a git binary on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scratch_repo(name: &str) -> PathBuf {
    let dir = common::temp_dir(&format!("w53-tier1-{name}"));
    git(&["init", "-q", "-b", "main"], &dir);
    dir
}

fn input(capture_id: &str, session_id: &str) -> CaptureSummaryInput {
    CaptureSummaryInput {
        capture_id: capture_id.to_string(),
        session_id: session_id.to_string(),
        summary: "Fixed the login handler retry bug.\n\nTags: auth, bug-fix".to_string(),
        memory_type: "bug".to_string(),
        tags: vec!["auth".to_string(), "bug-fix".to_string()],
        prompt_id: Some("prompt-7".to_string()),
    }
}

async fn fixture(name: &str) -> (TestDir, mem_store::shard_manager::ShardManager, PathBuf) {
    let dir = TestDir::new(&format!("w53-tier1-fix-{name}"));
    let shards = common::shard_manager_with_probe(
        dir.store_config(),
        Arc::new(OwnedAndForeignProbe {
            foreign_pid: 9_999_999,
        }),
    );
    let repo = scratch_repo(name);
    (dir, shards, repo)
}

/// Q13/AC4: the capture-summary row lands with both session linkages,
/// the exact container tag, and a replay leaves exactly one row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_tier1_row_has_session_id_metadata_query_and_replay_is_idempotent() {
    let (_dir, shards, repo) = fixture("q13").await;
    use std::collections::BTreeMap;
    let env_name: BTreeMap<String, String> = [("PATH", std::env::var("PATH").unwrap_or_default())]
        .map(|(k, v)| (k.to_string(), v))
        .into_iter()
        .collect();
    let git_probe = mem_core::tags::probe::SystemGitProbe::new(&env_name);
    let resolver = mem_core::tags::TagResolver::new("opencode", None, None, &env_name, &git_probe);
    let scope = resolve_project_scope(&repo.to_string_lossy(), &resolver).expect("resolve");

    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: Arc::new(shards.clone()),
        embedder: Arc::new(FakeEmbedder::with_vector(vec![
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ])),
    };

    let outcome = write_capture_summary(
        &deps,
        &scope,
        &input("019a0000-0000-7f00-8000-000000000042", "sess-1"),
    )
    .await
    .expect("the first tier-1 write succeeds");
    assert_eq!(outcome, Tier1WriteOutcome::Written);

    // The deterministic id exists and has the pinned shape.
    let row_id =
        capture_summary_row_id(&scope.container_tag, "019a0000-0000-7f00-8000-000000000042");
    assert!(
        row_id.starts_with("auto-capture-") && row_id.len() == 13 + 64,
        "deterministic id shape: {row_id}"
    );

    let write_shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, &scope.scope_hash)
        .await
        .expect("shard exists after the write");
    let db = shards
        .open_shard(&write_shard.db_path)
        .await
        .expect("open shard");

    // (a) The row exists with the deterministic id.
    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT id, session_id, metadata, container_tag, type, tags FROM memories WHERE id = ?",
        vec![mem_store::db::Value::Text(row_id.clone())],
    )
    .await
    .expect("query")
    .expect("the row exists");
    let metadata_text = row.text("metadata").unwrap_or_default();

    // (b) BOTH session linkages carry the SAME string.
    assert_eq!(
        row.text("session_id").as_deref(),
        Some("sess-1"),
        "the explicit column"
    );
    assert!(
        metadata_text.contains(r#""sessionID":"sess-1""#),
        "metadata.sessionID is the same string: {metadata_text}"
    );
    assert!(
        metadata_text.contains(r#""source":"auto-capture""#),
        "source is pinned: {metadata_text}"
    );
    assert!(
        metadata_text.contains(r#""promptId":"prompt-7""#),
        "promptId rides separately: {metadata_text}"
    );
    assert_eq!(
        row.text("container_tag").as_deref(),
        Some(scope.container_tag.as_str())
    );
    assert_eq!(row.text("type").as_deref(), Some("bug"));
    assert_eq!(row.text("tags").as_deref(), Some("auth,bug-fix"));

    // (c) The public session query finds it.
    let hits = get_memories_by_session_id(db.as_ref(), "sess-1")
        .await
        .expect("session query");
    assert_eq!(hits.len(), 1, "exactly the capture-summary row");

    // (d) Replay of the SAME capture_id: exactly one row, count bumped once.
    let count_before = write_shard.vector_count;
    let replay = write_capture_summary(
        &deps,
        &scope,
        &input("019a0000-0000-7f00-8000-000000000042", "sess-1"),
    )
    .await
    .expect("replay");
    assert_eq!(replay, Tier1WriteOutcome::Replayed);
    let count_after = {
        let shard = shards
            .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, &scope.scope_hash)
            .await
            .expect("shard");
        shard.vector_count
    };
    assert_eq!(count_before, count_after, "no second count bump on replay");
    let hits = get_memories_by_session_id(db.as_ref(), "sess-1")
        .await
        .expect("session query");
    assert_eq!(hits.len(), 1, "exactly one row after replay");
}

/// A1: the tier-1 write succeeds while the operation lock file records THIS
/// process as the holder (the engine's own consolidation window); a
/// FOREIGN live pid is refused and reported as a deferral (never a
/// consumed attempt).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_tier1_write_tolerates_own_consolidation_lock() {
    let (dir, shards, repo) = fixture("own-lock").await;
    use std::collections::BTreeMap;
    let env_name: BTreeMap<String, String> = [("PATH", std::env::var("PATH").unwrap_or_default())]
        .map(|(k, v)| (k.to_string(), v))
        .into_iter()
        .collect();
    let git_probe = mem_core::tags::probe::SystemGitProbe::new(&env_name);
    let resolver = mem_core::tags::TagResolver::new("opencode", None, None, &env_name, &git_probe);
    let scope = resolve_project_scope(&repo.to_string_lossy(), &resolver).expect("resolve");

    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: Arc::new(shards.clone()),
        embedder: Arc::new(FakeEmbedder::with_vector(vec![
            0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ])),
    };

    // The engine's own consolidation holds the lock for its whole run —
    // the exact same file `Consolidator::run` owns.
    let lock = mem_store::operation_lock::acquire_turso_operation_lock(
        dir.path(),
        Arc::new(OwnedAndForeignProbe {
            foreign_pid: 9_999_999,
        }),
        "episodic-consolidation",
    )
    .expect("the test holds its own operation lock");

    let outcome = write_capture_summary(
        &deps,
        &scope,
        &input("019a0000-0000-7f00-8000-000000000043", "sess-2"),
    )
    .await
    .expect("the own-lock window must not fail the write");
    assert_eq!(outcome, Tier1WriteOutcome::Written);
    lock.release();

    // A FOREIGN live pid (pid=999999 reported alive by the fake probe) is
    // refused: the write leg reports a DEFERRAL, not a write and not a
    // generic error — the inbox must NOT consume an attempt for it.
    let foreign = mem_store::operation_lock::acquire_turso_operation_lock(
        dir.path(),
        Arc::new(OwnedAndForeignProbe {
            foreign_pid: 9_999_999,
        }),
        "episodic-consolidation-foreign",
    )
    .expect("lock acquired");
    // Rewrite the lock body to a foreign pid that the fake probe keeps
    // alive.
    foreign.release();
    fake_lock_file(&dir, 9_999_999);
    let outcome = write_capture_summary(
        &deps,
        &scope,
        &input("019a0000-0000-7f00-8000-000000000044", "sess-2"),
    )
    .await;
    match outcome {
        Err(mem_bridge::capture::CaptureWriterError::Deferred { .. }) => {}
        other => panic!("a foreign live lock is a deferral, got {other:?}"),
    }
}

fn fake_lock_file(dir: &TestDir, pid: u32) {
    let body = json!({
        "pid": pid,
        "timestamp": "2026-10-21T00:00:00Z",
        "operation": "episodic-consolidation",
    });
    fs::write(dir.path().join(".turso-operation.lock"), body.to_string()).expect("write lock");
}

// ---------------------------------------------------------------------------
// Security mandate 3 (r1 rework): the tier-1 metadata document is built from
// wire-controlled strings and MUST be serialized, never interpolated.
// ---------------------------------------------------------------------------

/// A `session_id`/`promptId` containing JSON quotes must remain a VALUE:
/// the metadata document stays well-formed, keeps exactly one `source` key
/// (so the row can never be re-classified as a promoted fact), and the
/// embedded value round-trips verbatim.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_metadata_json_survives_quotes_in_session_id() {
    use common::{FakeEmbedder, FixedScopeResolver};
    use mem_bridge::inject::{InjectParams, InjectService};
    use mem_episodic::store::EpisodicStore;

    const TAG: &str = "opencode_project_0123456789abcdef";
    const HASH: &str = "0123456789abcdef";

    let dir = TestDir::new("w53-tier1-metadata-json");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let deps = mem_bridge::capture::CaptureWriterDeps {
        shards: shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(3))),
    };
    let scope =
        mem_bridge::project_scope::apply_resolved_scope("/srv/proj", TAG, HASH).expect("scope");

    let evil_session = "sess-1\",\"source\":\"episodic-consolidation";
    let mut evil_input = input("019a0000-0000-7f00-8000-000000000045", evil_session);
    evil_input.prompt_id = Some("p\",\"source\":\"episodic-consolidation".to_string());
    let outcome = write_capture_summary(&deps, &scope, &evil_input)
        .await
        .expect("the write must not fail on adversarial quoting");
    assert_eq!(outcome, Tier1WriteOutcome::Written);

    let shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("shard");
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT metadata FROM memories WHERE session_id = ?",
        vec![mem_store::db::Value::Text(evil_session.to_string())],
    )
    .await
    .expect("query")
    .expect("the row exists under the verbatim session id");
    let raw = row.text("metadata").expect("metadata");
    let value: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(&raw).expect("well-formed JSON");
    assert_eq!(
        value["source"], "auto-capture",
        "the classifier key cannot be rewritten: {raw}"
    );
    assert_eq!(
        value["sessionID"], evil_session,
        "the value round-trips verbatim"
    );
    assert_eq!(
        value["promptId"], "p\",\"source\":\"episodic-consolidation",
        "the prompt id round-trips verbatim"
    );
    assert_eq!(
        raw.matches("\"source\"").count(),
        1,
        "no duplicate keys (serde keeps the LAST duplicate, so a parse-only check would miss injection): {raw}"
    );
    assert_eq!(raw.matches("\"sessionID\"").count(), 1);
    assert_eq!(
        value.as_object().map(|object| object.len()),
        Some(4),
        "source, sessionID, captureTimestamp, promptId: {raw}"
    );

    // End-to-end: the row classifies as a captured summary, never a fact.
    let inject = InjectService::new(
        store.clone(),
        shards.clone(),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(3))),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );
    let result = inject
        .assemble(&InjectParams {
            project_id: Some("/srv/proj".to_string()),
            agent_id: None,
            token_budget: Some(2_000),
            include: None,
        })
        .await
        .expect("assemble");
    let block = result.block.expect("the capture-summary row renders");
    assert!(
        block.contains("## Captured session summaries"),
        "a capture row renders under the summaries heading: {block}"
    );
    assert!(
        !block.contains("## Project facts"),
        "a capture row is never a promoted fact: {block}"
    );
}

/// Step 6/Step 7: the provider's parsed `type` reaches the tier-1 `type`
/// column (never the hardcoded "memory" default), and the summarizer's
/// `filesTouched` / `completedWork` / `decisions` / `openThreads` reach the
/// episodic row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_drain_persists_provider_type_and_extension_fields() {
    use common::{FakeEmbedder, FixedScopeResolver, FixedSummarizer};
    use mem_bridge::capture::{
        CaptureParams, CaptureService, CaptureWriterDeps, DefaultTier1Writer,
    };
    use mem_episodic::record::{Decision, WorkItem, WorkKind};
    use mem_episodic::store::EpisodicStore;

    const TAG: &str = "opencode_project_0123456789abcdef";
    const HASH: &str = "0123456789abcdef";

    let dir = TestDir::new("w53-tier1-drain");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let embedder: Arc<dyn mem_embed::embedder::Embedder> =
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(2)));
    let capture = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(DefaultTier1Writer::new(CaptureWriterDeps {
            shards: shards.clone(),
            embedder: embedder.clone(),
        })),
        embedder.clone(),
        Some(Arc::new(FixedSummarizer {
            summary: "Fixed the retry bug in the handler".to_string(),
            tags: vec!["auth".to_string()],
            memory_type: "bug".to_string(),
            extension: mem_bridge::capture::CaptureExtension {
                files_touched: vec!["crates/login.rs".to_string()],
                completed_work: vec![WorkItem {
                    kind: WorkKind::BugFix,
                    description: "stopped the retry storm".to_string(),
                }],
                decisions: vec![Decision {
                    description: "cap retries at three".to_string(),
                }],
                open_threads: vec!["wire the metric".to_string()],
            },
        })),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );

    capture
        .enqueue(&CaptureParams {
            session_id: "sess-1".to_string(),
            capture_id: "019a0000-0000-7f00-8000-0000000000d1".to_string(),
            project_id: "/srv/proj".to_string(),
            agent_id: "agent-1".to_string(),
            metadata: Some(json!({
                "promptId": "p-1",
                "autoCaptureContext": { "userPrompt": "fix the retry bug" }
            })),
            ..Default::default()
        })
        .await
        .expect("enqueue");

    let report = capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "the entry completed both tiers: {report:?}");
    assert_eq!(report.retryable + report.dead_lettered, 0);

    // (a) The episodic row carries the provider extension, TYPED (no stale
    // camelCase duplicates in `extra`).
    let sessions = store.list_sessions("/srv/proj", 10).await.expect("list");
    assert_eq!(sessions.len(), 1);
    let metadata = &sessions[0].metadata;
    assert_eq!(metadata.files_touched, vec!["crates/login.rs".to_string()]);
    assert_eq!(metadata.completed_work.len(), 1);
    assert_eq!(metadata.completed_work[0].kind, WorkKind::BugFix);
    assert_eq!(
        metadata.completed_work[0].description,
        "stopped the retry storm"
    );
    assert_eq!(metadata.decisions[0].description, "cap retries at three");
    assert_eq!(metadata.open_threads, vec!["wire the metric".to_string()]);
    assert!(
        !metadata.extra.contains_key("filesTouched"),
        "the raw camelCase copy must not be duplicated into extra"
    );

    // (b) The tier-1 row's `type` column is the PROVIDER's parsed type.
    let shard = shards
        .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
        .await
        .expect("shard");
    let db = shards.open_shard(&shard.db_path).await.expect("open shard");
    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT type, tags, container_tag FROM memories WHERE session_id = ?",
        vec![mem_store::db::Value::Text("sess-1".to_string())],
    )
    .await
    .expect("query")
    .expect("the capture-summary row exists");
    assert_eq!(
        row.text("type").as_deref(),
        Some("bug"),
        "the provider's parsed type survives the drain"
    );
    assert_eq!(row.text("tags").as_deref(), Some("auth"));
    assert_eq!(row.text("container_tag").as_deref(), Some(TAG));
}

// ---------------------------------------------------------------------------
// W1 (issue 43.1) — the deterministic id is resolved across the WHOLE scope
// ---------------------------------------------------------------------------

/// A cap-1 / 2-dim store config: one row fills a shard, so the very next
/// write resolves a rotated (new) shard. The `FakeEmbedder` must therefore
/// return 2-dim vectors (`rotation_deps`).
fn rotation_config(dir: &TestDir) -> mem_store::config::StoreConfig {
    let mut config = dir.store_config();
    config.embedding_dimensions = 2;
    config.max_vectors_per_shard = 1;
    config
}

/// Capture writer deps over `shards` with 2-dim embeddings.
fn rotation_deps(shards: &Arc<ShardManager>) -> mem_bridge::capture::CaptureWriterDeps {
    mem_bridge::capture::CaptureWriterDeps {
        shards: shards.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(vec![1.0, 0.0])),
    }
}

/// The registry rows of one project scope, ascending by `shard_index`.
async fn shards_in_scope(shards: &ShardManager, scope_hash: &str) -> Vec<ShardInfo> {
    shards
        .get_all_shards(mem_core::scope::Scope::Project, Some(scope_hash))
        .await
        .expect("registry rows")
}

/// `SELECT COUNT(*) FROM memories WHERE id = ?` inside ONE shard file.
async fn count_in_shard(shards: &ShardManager, info: &ShardInfo, id: &str) -> i64 {
    let db = shards.open_shard(&info.db_path).await.expect("open shard");
    let row = mem_store::db::query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM memories WHERE id = ?",
        vec![mem_store::db::Value::Text(id.to_string())],
    )
    .await
    .expect("count query")
    .expect("a count row");
    row.i64("count").unwrap_or(0)
}

/// The same count summed over every shard of the scope whose FILE still
/// exists: a removed shard holds no rows, and opening it would re-create it.
async fn count_across_live_shards(shards: &ShardManager, scope_hash: &str, id: &str) -> i64 {
    let mut total = 0;
    for info in shards_in_scope(shards, scope_hash).await {
        if !info.db_path.exists() {
            continue;
        }
        total += count_in_shard(shards, &info, id).await;
    }
    total
}

/// AC4 — the defect, reproduced: after a rotation the deterministic id lives
/// in the RETIRED shard, so the second write must REPLAY there instead of
/// inserting a duplicate into the active shard.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_replay_after_rotation_is_replayed_and_leaves_one_row_in_the_scope() {
    const TAG: &str = "opencode_project_0123456789abcdef";
    const HASH: &str = "0123456789abcdef";
    const CAPTURE_ID: &str = "019a0000-0000-7f00-8000-0000000000e1";

    let dir = TestDir::new("w53-tier1-rotation-replay");
    let config = rotation_config(&dir);
    let shards = Arc::new(common::shard_manager(config));
    let deps = rotation_deps(&shards);
    let scope =
        mem_bridge::project_scope::apply_resolved_scope("/srv/proj", TAG, HASH).expect("scope");
    let row_id = capture_summary_row_id(TAG, CAPTURE_ID);

    let first = write_capture_summary(&deps, &scope, &input(CAPTURE_ID, "sess-1"))
        .await
        .expect("the first tier-1 write succeeds");
    assert_eq!(first, Tier1WriteOutcome::Written);

    // The second write resolves a shard that ALREADY rotated (cap = 1): the
    // row it must find lives in the retired shard 0.
    let replay = write_capture_summary(&deps, &scope, &input(CAPTURE_ID, "sess-1"))
        .await
        .expect("the replay must not fail");
    assert_eq!(
        replay,
        Tier1WriteOutcome::Replayed,
        "the id exists in the retired shard: this is a replay, not a second insert"
    );

    let registry = shards_in_scope(&shards, HASH).await;
    assert_eq!(
        registry.len(),
        2,
        "the rotation really happened (non-vacuity): {registry:?}"
    );
    assert_eq!(
        count_in_shard(&shards, &registry[0], &row_id).await,
        1,
        "the row sits in the retired shard 0"
    );
    assert_eq!(
        count_in_shard(&shards, &registry[1], &row_id).await,
        0,
        "shard 1 never received a copy"
    );
    assert_eq!(
        count_across_live_shards(&shards, HASH, &row_id).await,
        1,
        "exactly one row with the deterministic id across the whole scope"
    );
}

/// AC5 — capture DEFERS and writes nothing when a scope shard is unreadable.
///
/// The ONE mechanism able to produce the deferral in this fixture: shard 0 is
/// registered in the metadata registry and its `.db` was deliberately removed.
/// Everything else the write needs (the active shard's file, the embeddings,
/// the registry) is healthy.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_is_deferred_and_writes_nothing_when_a_registered_shard_is_unreadable() {
    const TAG: &str = "opencode_project_0123456789abcdef";
    const HASH: &str = "0123456789abcdef";

    let dir = TestDir::new("w53-tier1-scope-shard-missing");
    let config = rotation_config(&dir);
    let shards = Arc::new(common::shard_manager(config.clone()));
    let scope =
        mem_bridge::project_scope::apply_resolved_scope("/srv/proj", TAG, HASH).expect("scope");

    // A lands in shard 0; B's resolution retires the full shard 0 and lands
    // in shard 1.
    for capture_id in [
        "019a0000-0000-7f00-8000-0000000000e2",
        "019a0000-0000-7f00-8000-0000000000e3",
    ] {
        let deps = rotation_deps(&shards);
        let outcome = write_capture_summary(&deps, &scope, &input(capture_id, "sess-1"))
            .await
            .expect("an ordinary tier-1 write succeeds");
        assert_eq!(outcome, Tier1WriteOutcome::Written);
    }

    let registry = shards_in_scope(&shards, HASH).await;
    assert_eq!(
        registry.len(),
        2,
        "A in shard 0, B in shard 1: {registry:?}"
    );
    let shard_zero = registry[0].db_path.clone();
    assert!(shard_zero.exists());

    // Drop every handle to shard 0 (a cached connection would still read the
    // unlinked file), delete its database, then build a FRESH manager over
    // the same root.
    drop(shards);
    std::fs::remove_file(&shard_zero).expect("remove shard 0's database");

    let fresh = Arc::new(common::shard_manager(config));
    let deps = rotation_deps(&fresh);
    let capture_id = "019a0000-0000-7f00-8000-0000000000e4";
    let row_id = capture_summary_row_id(TAG, capture_id);

    let outcome = write_capture_summary(&deps, &scope, &input(capture_id, "sess-1")).await;
    match outcome {
        Err(mem_bridge::capture::CaptureWriterError::Deferred(_)) => {}
        other => panic!(
            "an unreadable scope shard must DEFER (never `Failed`, never a write), got {other:?}"
        ),
    }

    let registry = shards_in_scope(&fresh, HASH).await;
    assert_eq!(
        registry.len(),
        3,
        "the write's resolution retired the full shard 1 and created shard 2 BEFORE the scan: {registry:?}"
    );
    assert!(registry[2].is_active, "shard 2 is the active shard");
    // AC5's "0 rows across all three shards", realised: shard 0's file is gone
    // (0 rows by construction, absence asserted below), so the countable set
    // is the two surviving shards.
    assert!(
        !shard_zero.exists(),
        "the scan must never re-create a missing shard file"
    );
    assert_eq!(
        count_across_live_shards(&fresh, HASH, &row_id).await,
        0,
        "no row with C's id in the two surviving shards"
    );
}
