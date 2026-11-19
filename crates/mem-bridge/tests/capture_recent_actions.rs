//! Issue 149 — capture persists the child's recent tool-call actions, and a
//! skip with tool calls writes a deterministic floor row.
//!
//! `test_skip_with_only_text_responses_writes_no_row` is a PASS-TODAY
//! invariant, not a RED test: the `Skip` arm writes nothing today, so it
//! compiles and passes against unmodified code, and it must keep passing
//! after the deterministic row lands (a skip without tool calls stays silent).

mod common;

use std::sync::Arc;

use common::{FixedScopeResolver, TestDir};
use mem_bridge::capture::{CaptureParams, CaptureService, Tier1Writer};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

const PROJECT: &str = "/srv/proj";
const SESSION: &str = "sess-1";
const AGENT: &str = "agent-1";

async fn fixture(
    dir: &TestDir,
    writer: Arc<dyn Tier1Writer>,
    summarizer: Arc<dyn mem_bridge::capture_prompt::CaptureSummarizer>,
) -> (CaptureService, Arc<EpisodicStore>) {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        writer,
        Arc::new(common::FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(summarizer),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );
    (service, store)
}

fn params(capture_id: &str, metadata: mem_store::serde_json::Value) -> CaptureParams {
    CaptureParams {
        session_id: SESSION.to_string(),
        capture_id: capture_id.to_string(),
        project_id: PROJECT.to_string(),
        agent_id: AGENT.to_string(),
        agent_name: None,
        parent_session_id: None,
        transcript: None,
        summary: None,
        metadata: Some(metadata),
        ..Default::default()
    }
}

fn bash_calls(inputs: &[&str]) -> Vec<mem_store::serde_json::Value> {
    inputs
        .iter()
        .map(|input| json!({ "name": "bash", "input": input }))
        .collect()
}

/// A skip carrying one `bash` tool call writes exactly one deterministic row
/// and no tier-1 content and no raw-context row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_skip_with_tool_calls_writes_a_deterministic_row_with_actions() {
    let dir = TestDir::new("w149-skip-actions");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let (service, store) = fixture(&dir, recorder.clone(), Arc::new(common::SkipSummarizer)).await;

    let payload = params(
        "019a0000-0000-7f00-8000-000000000101",
        json!({ "autoCaptureContext": {
            "userPrompt": "do the thing",
            "textResponses": [],
            "toolCalls": [{ "name": "bash", "input": "command: \"cargo test\"" }]
        } }),
    );
    service.enqueue(&payload).await.expect("enqueue");
    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "the skip completes the item");

    let rows = store.list_sessions(PROJECT, 10).await.expect("list");
    assert_eq!(rows.len(), 1, "exactly one deterministic row");
    let row = &rows[0];
    assert_eq!(
        row.metadata.recent_actions,
        vec!["bash: command: \"cargo test\"".to_string()]
    );
    assert!(row.metadata.capture_deterministic);
    assert!(row.metadata.files_touched.is_empty());
    assert!(row.metadata.open_threads.is_empty());
    assert_eq!(recorder.writes(), 0, "no tier-1 write on the skip path");
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw context read")
            .is_none(),
        "no raw-context row on the skip path"
    );
}

/// PASS-TODAY INVARIANT (not RED). A skip whose only content is text
/// responses carries no tool calls, so no row is written. Passes against the
/// unmodified `Skip` arm and must keep passing after the deterministic row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_skip_with_only_text_responses_writes_no_row() {
    let dir = TestDir::new("w149-skip-text");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let (service, store) = fixture(&dir, recorder.clone(), Arc::new(common::SkipSummarizer)).await;

    let payload = params(
        "019a0000-0000-7f00-8000-000000000102",
        json!({ "autoCaptureContext": {
            "userPrompt": "hello",
            "textResponses": ["just chatting"],
            "toolCalls": []
        } }),
    );
    service.enqueue(&payload).await.expect("enqueue");
    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1);
    assert!(
        store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .is_empty(),
        "no row without tool calls"
    );
    assert_eq!(recorder.writes(), 0);
}

/// A curated capture persists the same action lines on its row.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_curated_capture_persists_recent_actions() {
    let dir = TestDir::new("w149-curated-actions");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let summarizer = Arc::new(common::FixedSummarizer {
        summary: "curated summary".to_string(),
        ..Default::default()
    });
    let (service, store) = fixture(&dir, recorder, summarizer).await;

    let payload = params(
        "019a0000-0000-7f00-8000-000000000103",
        json!({ "autoCaptureContext": {
            "userPrompt": "do the thing",
            "textResponses": [],
            "toolCalls": [{ "name": "bash", "input": "command: \"cargo test\"" }]
        } }),
    );
    service.enqueue(&payload).await.expect("enqueue");
    service.drain_once().await.expect("drain");

    let rows = store.list_sessions(PROJECT, 10).await.expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary, "curated summary");
    assert_eq!(
        rows[0].metadata.recent_actions,
        vec!["bash: command: \"cargo test\"".to_string()]
    );
    assert!(!rows[0].metadata.capture_deterministic);
}

/// Twelve calls keep the newest eight, in chronological order.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_actions_keep_the_newest_eight_in_order() {
    let dir = TestDir::new("w149-newest-eight");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let (service, store) = fixture(&dir, recorder, Arc::new(common::SkipSummarizer)).await;

    let inputs: Vec<String> = (0..12).map(|n| format!("t{n}")).collect();
    let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let payload = params(
        "019a0000-0000-7f00-8000-000000000104",
        json!({ "autoCaptureContext": {
            "userPrompt": "do the thing",
            "textResponses": [],
            "toolCalls": bash_calls(&refs)
        } }),
    );
    service.enqueue(&payload).await.expect("enqueue");
    service.drain_once().await.expect("drain");

    let rows = store.list_sessions(PROJECT, 10).await.expect("list");
    let actions = &rows[0].metadata.recent_actions;
    assert_eq!(actions.len(), 8, "newest eight only");
    assert_eq!(actions.first().map(String::as_str), Some("bash: t4"));
    assert_eq!(actions.last().map(String::as_str), Some("bash: t11"));
}

/// A 400-character multibyte input is cut to at most 160 bytes, on a char
/// boundary, and stays valid UTF-8 (a `String` is always valid, so the length
/// bound and the prefix check carry the assertion).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_action_line_truncates_at_the_byte_cap_on_a_char_boundary() {
    let dir = TestDir::new("w149-byte-cap");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let (service, store) = fixture(&dir, recorder, Arc::new(common::SkipSummarizer)).await;

    let input = "é".repeat(400);
    let payload = params(
        "019a0000-0000-7f00-8000-000000000105",
        json!({ "autoCaptureContext": {
            "userPrompt": "do the thing",
            "textResponses": [],
            "toolCalls": [{ "name": "bash", "input": input }]
        } }),
    );
    service.enqueue(&payload).await.expect("enqueue");
    service.drain_once().await.expect("drain");

    let rows = store.list_sessions(PROJECT, 10).await.expect("list");
    let line = &rows[0].metadata.recent_actions[0];
    assert!(line.len() <= 160, "line is {} bytes", line.len());
    assert!(line.len() > 150, "cut close to the cap, not far below it");
    assert!(
        input.starts_with(&line[6..]),
        "prefix of the input after `bash: `"
    );
}

/// A `<private>` region in a tool input is removed before the line persists.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_private_region_is_stripped_from_persisted_actions() {
    let dir = TestDir::new("w149-private");
    let recorder = Arc::new(common::RecorderTier1Writer::new(0));
    let (service, store) = fixture(&dir, recorder, Arc::new(common::SkipSummarizer)).await;

    let payload = params(
        "019a0000-0000-7f00-8000-000000000106",
        json!({ "autoCaptureContext": {
            "userPrompt": "do the thing",
            "textResponses": [],
            "toolCalls": [{ "name": "bash", "input": "command: \"echo <private>secret</private>\"" }]
        } }),
    );
    service.enqueue(&payload).await.expect("enqueue");
    service.drain_once().await.expect("drain");

    let rows = store.list_sessions(PROJECT, 10).await.expect("list");
    let line = &rows[0].metadata.recent_actions[0];
    assert!(!line.contains("secret"), "persisted line: {line}");
    assert!(line.starts_with("bash: command:"), "persisted line: {line}");
}
