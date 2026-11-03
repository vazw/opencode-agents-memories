//! Session exclusion applies only when explicitly requested.
mod common;

use common::{FakeEmbedder, TestDir};
use mem_bridge::inject::InjectParams;
use mem_episodic::record::NewAgentSession;
use serde_json::json;

async fn check(flag: Option<bool>, exclude: bool) {
    let dir = TestDir::new("inject-session-exclusion");
    let fixture = common::build_fixture_runtime(dir.path()).await;
    for (session_id, summary) in [
        ("sess-self", "SELF_SENTINEL"),
        ("sess-other", "OTHER_SENTINEL"),
    ] {
        fixture
            .store
            .insert_session(
                NewAgentSession {
                    project_id: "/srv/proj".into(),
                    session_id: session_id.into(),
                    agent_id: "agent".into(),
                    summary: summary.into(),
                    ..Default::default()
                },
                &FakeEmbedder::with_vector(common::unit_vector(0)),
            )
            .await
            .expect("seed");
    }
    let mut value = json!({"project_id":"/srv/proj", "session_id":"sess-self"});
    if let Some(flag) = flag {
        value["exclude_session"] = json!(flag);
    }
    let params: InjectParams = serde_json::from_value(value).expect("params");
    let block = fixture
        .inject
        .assemble(&params)
        .await
        .expect("assemble")
        .block
        .expect("block");
    assert!(block.contains("OTHER_SENTINEL"));
    assert_eq!(
        block.contains("SELF_SENTINEL"),
        !exclude,
        "session exclusion: {block}"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_assemble_excludes_the_requesting_session_when_asked() {
    check(Some(true), true).await;
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_assemble_preserves_session_when_flag_off() {
    check(Some(false), false).await;
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_assemble_preserves_session_when_flag_omitted() {
    check(None, false).await;
}
