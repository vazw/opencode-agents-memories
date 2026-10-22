//! W7.1 — capture-path sound memory: the additive payload, the flag-gated
//! retention writes, the transcript fallback, the privacy strip, the
//! dead-letter gates and the agent role.

mod common;

use std::sync::Arc;

use common::{FailingSummarizer, FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_bridge::capture::{
    AGENT_ROLE_INVALID_MESSAGE, CaptureParams, CaptureService, SoundMemorySettings,
    parse_capture_agent_role,
};
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

async fn fixture(
    dir: &TestDir,
    settings: SoundMemorySettings,
    summarizer_fails: bool,
) -> (CaptureService, Arc<EpisodicStore>) {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let summarizer: Option<Arc<dyn mem_bridge::capture_prompt::CaptureSummarizer>> =
        if summarizer_fails {
            Some(Arc::new(FailingSummarizer))
        } else {
            None
        };
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        summarizer,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
    .with_sound_memory(settings);
    (service, store)
}

fn params(
    raw_context: Option<&str>,
    transcript: Option<&str>,
    agent_role: Option<&str>,
) -> CaptureParams {
    CaptureParams {
        session_id: "sess-w71".to_string(),
        capture_id: "019a0000-0000-7f00-8000-0000000000aa".to_string(),
        project_id: "/srv/proj".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: Some("build".to_string()),
        parent_session_id: None,
        transcript: transcript.map(str::to_string),
        summary: Some("the captured summary".to_string()),
        metadata: Some(json!({ "promptId": "p-1" })),
        raw_context: raw_context.map(str::to_string),
        agent_role: agent_role.map(str::to_string),
    }
}

const ON: SoundMemorySettings = SoundMemorySettings {
    enabled: true,
    raw_context_required: false,
};

const ON_REQUIRED: SoundMemorySettings = SoundMemorySettings {
    enabled: true,
    raw_context_required: true,
};

const OFF: SoundMemorySettings = SoundMemorySettings {
    enabled: false,
    raw_context_required: false,
};

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_v1_capture_payload_without_raw_fields_is_unchanged() {
    let dir = TestDir::new("w71-v1-payload");
    let (service, store) = fixture(&dir, OFF, false).await;
    // A V1 payload: the two new fields are absent from the JSON entirely.
    let payload: CaptureParams = mem_store::serde_json::from_value(json!({
        "session_id": "sess-w71",
        "capture_id": "019a0000-0000-7f00-8000-0000000000aa",
        "project_id": "/srv/proj",
        "agent_id": "agent-1",
        "agent_name": "build",
        "transcript": "t",
        "summary": "the captured summary",
        "metadata": { "promptId": "p-1" }
    }))
    .expect("a V1 payload still deserializes");
    assert_eq!(payload.raw_context, None);
    assert_eq!(payload.agent_role, None);

    service.enqueue(&payload).await.expect("enqueue");
    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1);

    let sessions = store.list_sessions("/srv/proj", 10).await.expect("list");
    assert_eq!(sessions.len(), 1, "the session row is written as before");
    assert_eq!(sessions[0].summary, "the captured summary");
    assert!(
        store
            .raw_context_for_session("/srv/proj", "sess-w71", "agent-1")
            .await
            .expect("raw")
            .is_none(),
        "the flag is off: no retention row"
    );
    assert_eq!(
        store
            .agent_role_for("/srv/proj", "sess-w71", "agent-1")
            .await
            .expect("role")
            .as_deref(),
        Some("main")
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_persists_raw_context_and_segments_when_sound_memory_is_on() {
    let dir = TestDir::new("w71-raw-on");
    let (service, store) = fixture(&dir, ON, false).await;
    let raw = "first paragraph\nstill first\n\nsecond paragraph";
    service
        .enqueue(&params(Some(raw), Some("the transcript"), None))
        .await
        .expect("enqueue");
    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1);

    let stored = store
        .raw_context_for_session("/srv/proj", "sess-w71", "agent-1")
        .await
        .expect("raw")
        .expect("present");
    assert_eq!(stored.raw_context, raw, "byte-equal to the payload");
    assert_eq!(stored.byte_len as usize, raw.len());

    let segments = store
        .segments_for_session("/srv/proj", "sess-w71", "agent-1")
        .await
        .expect("segments");
    let rebuilt: String = segments
        .iter()
        .map(|segment| segment.raw_content.as_str())
        .collect();
    assert_eq!(rebuilt, raw, "segments partition the raw bytes");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_falls_back_to_the_transcript_as_raw_context() {
    let dir = TestDir::new("w71-transcript-fallback");
    let (service, store) = fixture(&dir, ON, false).await;
    let transcript = "the user asked about retries\n\nthe agent fixed it";
    service
        .enqueue(&params(None, Some(transcript), None))
        .await
        .expect("enqueue");
    assert_eq!(service.drain_once().await.expect("drain").done, 1);

    let stored = store
        .raw_context_for_session("/srv/proj", "sess-w71", "agent-1")
        .await
        .expect("raw")
        .expect("the transcript is the fallback raw context");
    assert_eq!(stored.raw_context, transcript);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_writes_no_retention_rows_when_the_flag_is_off() {
    let dir = TestDir::new("w71-flag-off");
    let (service, store) = fixture(&dir, OFF, false).await;
    service
        .enqueue(&params(
            Some("raw bytes that must NOT be stored"),
            Some("transcript"),
            None,
        ))
        .await
        .expect("enqueue");
    assert_eq!(service.drain_once().await.expect("drain").done, 1);

    // The session row is still written (the flag gates only retention).
    assert_eq!(
        store
            .list_sessions("/srv/proj", 10)
            .await
            .expect("list")
            .len(),
        1
    );
    assert!(
        store
            .raw_context_for_session("/srv/proj", "sess-w71", "agent-1")
            .await
            .expect("raw")
            .is_none()
    );
    assert!(
        store
            .segments_for_session("/srv/proj", "sess-w71", "agent-1")
            .await
            .expect("segments")
            .is_empty()
    );
    // The flags-off proof includes the facts tier: no facts.db is created.
    assert!(
        !dir.path().join("facts.db").exists(),
        "flags off create no facts.db"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_private_blocks_are_absent_from_stored_raw_context() {
    let dir = TestDir::new("w71-private");
    let (service, store) = fixture(&dir, ON, false).await;
    let raw = "public start <private>the secret token</private> public end";
    service
        .enqueue(&params(Some(raw), Some("t"), None))
        .await
        .expect("enqueue");
    assert_eq!(service.drain_once().await.expect("drain").done, 1);

    let stored = store
        .raw_context_for_session("/srv/proj", "sess-w71", "agent-1")
        .await
        .expect("raw")
        .expect("present");
    assert!(
        !stored.raw_context.contains("the secret token"),
        "private block stripped: {:?}",
        stored.raw_context
    );
    assert!(stored.raw_context.contains("public start"));
    assert_eq!(
        stored.raw_context,
        mem_ai::privacy::strip_private_content(raw),
        "the stored bytes are the STRIPPED bytes"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_missing_raw_text_is_dead_lettered_when_required() {
    let dir = TestDir::new("w71-required");
    // The summarizer FAILS: if it were reached the outcome would be retryable,
    // so a dead letter proves the gate ran before the summarizer.
    let (service, store) = fixture(&dir, ON_REQUIRED, true).await;
    service
        .enqueue(&params(None, None, None))
        .await
        .expect("enqueue");
    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.dead_lettered, 1);
    assert_eq!(report.retryable, 0, "the summarizer was never called");
    assert_eq!(report.done, 0);

    assert_eq!(
        store
            .list_sessions("/srv/proj", 10)
            .await
            .expect("list")
            .len(),
        0,
        "no session row for a dead-lettered payload"
    );
    let db = store.ready().await.expect("ready");
    let status = mem_store::db::query_one(
        db.as_ref(),
        "SELECT status FROM capture_inbox WHERE capture_id = ?",
        vec![mem_store::db::Value::Text(
            "019a0000-0000-7f00-8000-0000000000aa".to_string(),
        )],
    )
    .await
    .expect("status query")
    .expect("the dead row is kept");
    assert_eq!(status.text("status").as_deref(), Some("dead"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_persists_the_supplied_agent_role() {
    let dir = TestDir::new("w71-agent-role");
    let (service, store) = fixture(&dir, ON, false).await;
    service
        .enqueue(&params(Some("raw"), Some("t"), Some("reviewer")))
        .await
        .expect("enqueue");
    assert_eq!(service.drain_once().await.expect("drain").done, 1);
    assert_eq!(
        store
            .agent_role_for("/srv/proj", "sess-w71", "agent-1")
            .await
            .expect("role")
            .as_deref(),
        Some("reviewer")
    );

    // Omitted role: the v6 default stays `main`.
    let dir = TestDir::new("w71-agent-role-default");
    let (service, store) = fixture(&dir, ON, false).await;
    service
        .enqueue(&params(Some("raw"), Some("t"), None))
        .await
        .expect("enqueue");
    assert_eq!(service.drain_once().await.expect("drain").done, 1);
    assert_eq!(
        store
            .agent_role_for("/srv/proj", "sess-w71", "agent-1")
            .await
            .expect("role")
            .as_deref(),
        Some("main")
    );

    // An unknown role is dead-lettered with a FIXED message that never echoes
    // the value, and writes no session row.
    let dir = TestDir::new("w71-agent-role-bad");
    let (service, store) = fixture(&dir, OFF, false).await;
    service
        .enqueue(&params(Some("raw"), Some("t"), Some("boss")))
        .await
        .expect("enqueue");
    let report = service.drain_once().await.expect("drain");
    assert_eq!(report.dead_lettered, 1);
    assert_eq!(
        store
            .list_sessions("/srv/proj", 10)
            .await
            .expect("list")
            .len(),
        0
    );

    let error = parse_capture_agent_role(Some("boss")).expect_err("unknown role");
    assert_eq!(error, AGENT_ROLE_INVALID_MESSAGE);
    assert!(
        !error.contains("boss"),
        "the message never echoes the value"
    );
}
