//! Issue 50.4a — the `relevance="N%"` attribute on the one tier that already
//! carries a score: `## Related older sessions`.
//!
//! The fixture is the same three-episode shape as the tier-order test
//! (`old session`/`mid session`/`new session`, ids `sess-0000/1/2`), with no
//! tier-1 rows; all vectors are identical, so the semantic leg's similarity is
//! `1.0` and the rendered attribute is `100%`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::inject::{InjectParams, InjectService};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_older_sessions_carry_the_similarity_attribute() {
    let dir = TestDir::new("inject-relevance-older");
    let counter = Arc::new(AtomicU64::new(0));
    let id_counter = counter.clone();
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_id_factory(Arc::new(
            move || {
                let index = id_counter.fetch_add(1, Ordering::SeqCst);
                format!("sess-{index:04}")
            },
        )),
    );
    store.ready().await.expect("store ready");
    let embedder = Arc::new(FakeEmbedder::with_vector(common::unit_vector(0)));
    let service = InjectService::new(
        store.clone(),
        Arc::new(common::shard_manager(dir.store_config())),
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        embedder.clone(),
        Arc::new(mem_store::platform::SystemProcessProbe),
    );

    for (index, summary) in ["old session", "mid session", "new session"]
        .into_iter()
        .enumerate()
    {
        let session = mem_episodic::record::NewAgentSession {
            project_id: "/srv/proj".to_string(),
            session_id: format!("sess-{index:04}"),
            agent_id: "agent-1".to_string(),
            summary: summary.to_string(),
            ..Default::default()
        };
        store
            .insert_session(session, embedder.as_ref())
            .await
            .expect("seed episode");
    }

    let result = service
        .assemble(&InjectParams {
            project_id: Some("/srv/proj".to_string()),
            agent_id: None,
            token_budget: Some(2_000),
            include: Some(json!({ "recent_count": 1, "query": "zzz" })),
            user_id: None,
            session_id: None,
        })
        .await
        .expect("assemble");
    let block = result.block.expect("the older row renders");
    assert!(
        block.contains(r#"<memory relevance="100%">agent-1: old session</memory>"#),
        "the older-session row carries the real similarity: {block}"
    );
}
