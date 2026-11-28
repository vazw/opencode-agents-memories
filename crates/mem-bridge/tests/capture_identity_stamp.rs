//! Issue 174 — the accept-time identity stamp travels with the capture.
//!
//! The drain still resolves the scope for the tag/shard, but a stamped entry's
//! identity OVERWRITES that resolution, so the memory row's `user_email` is the
//! value the accept path resolved — even when the resolver now answers
//! differently. A legacy entry (no key) keeps the old behaviour.

mod common;

use std::sync::Arc;

use serde_json::json;

use mem_bridge::capture::{CaptureParams, CaptureService, CaptureWriterDeps, DefaultTier1Writer};
use mem_bridge::project_scope::{ProjectScopeError, ResolvedProjectScope};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_episodic::store::EpisodicStore;
use mem_store::db::{Value, query_one};
use mem_store::shard_manager::ShardManager;

const TAG: &str = "opencode_project_0123456789abcdef";
const HASH: &str = "0123456789abcdef";

/// Resolves every project to the same scope with a FIXED `user_email`, so the
/// drain's own resolution is predictably different from the stamp.
struct IdentityResolver {
    email: Option<&'static str>,
}

impl ProjectScopeResolver for IdentityResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        let mut scope = mem_bridge::project_scope::apply_resolved_scope(project_id, TAG, HASH)?;
        scope.user_email = self.email.map(str::to_string);
        Ok(scope)
    }
}

struct Fixture {
    _dir: common::TestDir,
    store: Arc<EpisodicStore>,
    shards: Arc<ShardManager>,
    capture: CaptureService,
}

fn fixture(prefix: &str, email: Option<&'static str>) -> Fixture {
    let dir = common::TestDir::new(prefix);
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    let shards = Arc::new(common::shard_manager(dir.store_config()));
    let embedder: Arc<dyn mem_embed::embedder::Embedder> =
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(2)));
    let capture = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(DefaultTier1Writer::new(CaptureWriterDeps {
            shards: shards.clone(),
            embedder: embedder.clone(),
            prompt_stamps: Arc::new(mem_bridge::capture::AbsentPromptStampSource),
        })),
        embedder,
        Some(Arc::new(common::FixedSummarizer {
            summary: "Fixed the retry bug".to_string(),
            tags: vec!["auth".to_string()],
            memory_type: "bug".to_string(),
            extension: mem_bridge::capture::CaptureExtension::default(),
        })),
        Arc::new(IdentityResolver { email }),
        131_072,
    );
    Fixture {
        _dir: dir,
        store,
        shards,
        capture,
    }
}

fn params(capture_id: &str, session_id: &str) -> CaptureParams {
    CaptureParams {
        session_id: session_id.to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        metadata: Some(json!({
            "promptId": "p-1",
            "autoCaptureContext": { "userPrompt": "fix the retry bug" }
        })),
        ..Default::default()
    }
}

impl Fixture {
    /// The tier-1 row's `user_email` for one session.
    async fn memory_user_email(&self, session_id: &str) -> Option<String> {
        self.store.ready().await.expect("store ready");
        let shard = self
            .shards
            .get_write_shard_allowing_own_lock(mem_core::scope::Scope::Project, HASH)
            .await
            .expect("shard");
        let db = self
            .shards
            .open_shard(&shard.db_path)
            .await
            .expect("open shard");
        query_one(
            db.as_ref(),
            "SELECT user_email FROM memories WHERE session_id = ?",
            vec![Value::Text(session_id.to_string())],
        )
        .await
        .expect("read the tier-1 row")
        .expect("the capture-summary row exists")
        .text("user_email")
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_memory_row_carries_the_accept_time_identity() {
    // The drain's own resolver would say `b@y`; the stamp says `a@x`.
    let fx = fixture("capture-identity-stamp", Some("b@y"));
    fx.capture
        .enqueue_with_identity(&params("capture-stamp", "session-stamp"), Some("a@x"))
        .await
        .expect("enqueue stamped");
    let report = fx.capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "{report:?}");

    assert_eq!(
        fx.memory_user_email("session-stamp").await.as_deref(),
        Some("a@x"),
        "the memory row uses the accept-time stamp, not the drain's re-resolution"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_unstamped_entry_falls_back_to_the_resolver() {
    let fx = fixture("capture-identity-legacy", Some("b@y"));
    fx.capture
        .enqueue(&params("capture-legacy", "session-legacy"))
        .await
        .expect("enqueue legacy");
    let report = fx.capture.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "{report:?}");

    assert_eq!(
        fx.memory_user_email("session-legacy").await.as_deref(),
        Some("b@y"),
        "a legacy entry keeps the drain-time resolution"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_resolve_identity_maps_unknown_and_blank_to_none() {
    let missing = fixture("capture-identity-none", None);
    assert_eq!(
        missing.capture.resolve_identity("/srv/proj"),
        None,
        "a scope with no email resolves to None (SQL NULL), never a literal"
    );

    let blank = fixture("capture-identity-blank", Some("   "));
    assert_eq!(
        blank.capture.resolve_identity("/srv/proj"),
        None,
        "a whitespace-only email is not an identity"
    );

    let real = fixture("capture-identity-real", Some("a@x"));
    assert_eq!(
        real.capture.resolve_identity("/srv/proj"),
        Some("a@x".to_string())
    );
}
