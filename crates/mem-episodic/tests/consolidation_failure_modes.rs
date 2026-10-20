//! W4.3 lane — consolidation failure modes, ordering and the skip/abort
//! classification (AC-4).
//!
//! The seams under test are `Consolidator::run`, the
//! `PermanentMemorySink` port and the `EpisodicBatchWriter` port. The
//! summarizer-side seams (`stripped_source_summary`, `build_transcript`,
//! `fit_prefix`, `ProviderSummarizer`) are exercised here too, because the
//! summarizer is the first fallible phase of the per-agent pipeline.

mod common;

use std::sync::Arc;

use common::{ConsolidationFixture, DAY_MILLIS, FakeProvider, FakeSummarizer, RecordingSink};
use mem_ai::provider::AiProvider;
use mem_episodic::consolidation::SkipReason;
use mem_episodic::record::AgentSession;
use mem_episodic::summarizer::{
    MAX_TRANSCRIPT_BYTES, MILESTONE_SYSTEM_PROMPT, MilestoneSummarizer, ProviderSummarizer,
    build_transcript, fit_prefix, stripped_source_summary,
};
use mem_store::db::Value;

/// A minimal session carrying only the fields the transcript builder reads.
fn session(agent_name: &str, summary: &str) -> AgentSession {
    AgentSession {
        id: "row-1".to_string(),
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-a".to_string(),
        parent_session_id: None,
        summary: summary.to_string(),
        metadata: mem_episodic::payload::SessionMetadata {
            timestamp: 7,
            agent_name: agent_name.to_string(),
            ..Default::default()
        },
        embedding: Vec::new(),
        created_at: 11,
        updated_at: 11,
        deleted_at: None,
        orphaned: false,
    }
}

#[test]
fn test_stripped_source_summary_redacts_private_tags_and_trims() {
    // The `<private>` region is replaced by the redaction marker, never
    // surfaced verbatim.
    assert_eq!(
        stripped_source_summary("  hello <private>secret</private> world  "),
        "hello [REDACTED] world"
    );
    assert_eq!(stripped_source_summary("   "), "");
    assert_eq!(
        stripped_source_summary("<private>only</private>"),
        "[REDACTED]"
    );
}

#[test]
fn test_build_transcript_filters_private_content_and_omits_empty_fields() {
    let mut row = session(
        "Builder <private>hidden</private>",
        "did <private>secret</private> work",
    );
    row.metadata.decisions = vec![mem_episodic::record::Decision {
        description: "chose <private>token</private> X".to_string(),
    }];
    row.metadata.files_touched = vec!["src/lib.rs".to_string()];
    row.metadata.open_threads = vec!["   ".to_string()];
    row.metadata.completed_work = vec![mem_episodic::record::WorkItem {
        kind: mem_episodic::record::WorkKind::Feature,
        description: "built <private>thing</private>".to_string(),
    }];

    let chunks = build_transcript(std::slice::from_ref(&row));
    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert!(
        chunk.starts_with("[session session-1 · agent Builder [REDACTED] · 11]"),
        "header: {chunk}"
    );
    assert!(chunk.contains("Summary: did [REDACTED] work"), "{chunk}");
    assert!(
        chunk.contains("Completed: feature: built [REDACTED]"),
        "{chunk}"
    );
    assert!(chunk.contains("Decisions: chose [REDACTED] X"), "{chunk}");
    assert!(chunk.contains("Files: src/lib.rs"), "{chunk}");
    // The empty open-thread field is omitted entirely.
    assert!(!chunk.contains("Open threads:"), "{chunk}");
    // The private text never reaches the transcript.
    assert!(!chunk.contains("secret"), "{chunk}");
    assert!(!chunk.contains("hidden"), "{chunk}");
    assert!(!chunk.contains("token"), "{chunk}");
}

#[test]
fn test_fit_prefix_returns_the_largest_fitting_prefix() {
    let chunks = vec!["aaaa".to_string(), "bbbbbb".to_string(), "cc".to_string()];
    assert_eq!(fit_prefix(&chunks, 100), 3);
    assert_eq!(fit_prefix(&chunks, 12), 3);
    assert_eq!(fit_prefix(&chunks, 11), 2);
    assert_eq!(fit_prefix(&chunks, 4), 1);
    assert_eq!(fit_prefix(&chunks, 3), 0);
    assert_eq!(fit_prefix(&chunks, 0), 0);
    assert_eq!(fit_prefix(&[], 10), 0);
}

#[test]
fn test_transcript_cap_is_the_documented_bound() {
    assert_eq!(MAX_TRANSCRIPT_BYTES, 262_144);
    assert!(MILESTONE_SYSTEM_PROMPT.contains("milestone"));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_provider_summarizer_collects_text_chunks_until_done() {
    let provider: Arc<dyn AiProvider> = Arc::new(FakeProvider::with_text_chunks(vec![
        "first ".to_string(),
        "second".to_string(),
    ]));
    let summarizer = ProviderSummarizer::new(provider, "test-model");
    let summary = summarizer.summarize("transcript").await.expect("summarize");
    assert_eq!(summary, "first second");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_provider_summarizer_reports_provider_error_as_summary_error() {
    let provider: Arc<dyn AiProvider> = Arc::new(FakeProvider::failing("boom"));
    let summarizer = ProviderSummarizer::new(provider, "test-model");
    let error = summarizer
        .summarize("transcript")
        .await
        .expect_err("provider failure");
    assert!(
        matches!(&error, mem_episodic::EpisodicError::Summary { message } if message.contains("boom")),
        "unexpected error: {error}"
    );
}

/// The `AiProvider` seam must receive the transcript as the USER message and
/// the pinned system prompt as the system prompt.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_provider_summarizer_sends_transcript_as_user_message() {
    let provider = Arc::new(FakeProvider::with_text_chunks(vec!["ok".to_string()]));
    let dyn_provider: Arc<dyn AiProvider> = provider.clone();
    let summarizer = ProviderSummarizer::new(dyn_provider, "test-model");
    summarizer
        .summarize("the transcript body")
        .await
        .expect("summarize");
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "test-model");
    assert_eq!(
        requests[0].system.as_deref(),
        Some(MILESTONE_SYSTEM_PROMPT),
        "the pinned system prompt rides along"
    );
    let user = requests[0]
        .messages
        .iter()
        .find(|message| matches!(message.role, mem_ai::provider::request::Role::User))
        .expect("a user message");
    assert_eq!(user.content, "the transcript body");
}

// ---------------------------------------------------------------------------
// The per-agent skip / run-abort classification (AC-4)
// ---------------------------------------------------------------------------

/// The default window: 30 days.
const DEFAULT_AGE_DAYS: u32 = 30;

/// The consolidator over the recording batch writer.
fn recording_consolidator(
    fixture: &ConsolidationFixture,
) -> mem_episodic::consolidation::Consolidator {
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    fixture.consolidator(DEFAULT_AGE_DAYS, writer)
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_summarizer_failure_skips_agent_and_performs_zero_sink_calls() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    let summarizer = Arc::new(FakeSummarizer::failing("provider down"));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        summarizer,
        fixture.embedder.clone(),
        fixture.sink.clone(),
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(report.agents_skipped[0].reason, SkipReason::SummarizerError);
    assert_eq!(fixture.sink.call_count(), 0, "zero sink calls");
    assert_eq!(fixture.recorder.call_count(), 0, "zero batch writes");
    assert_eq!(report.milestones_written, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_empty_summary_skips_agent_and_performs_zero_sink_calls() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    let summarizer = Arc::new(FakeSummarizer::returning("   "));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        summarizer,
        fixture.embedder.clone(),
        fixture.sink.clone(),
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(report.agents_skipped[0].reason, SkipReason::EmptySummary);
    assert_eq!(fixture.sink.call_count(), 0);
    assert_eq!(fixture.recorder.call_count(), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_oversize_summary_skips_agent_and_performs_zero_sink_calls() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    let oversize = "x".repeat(100_001);
    let summarizer = Arc::new(FakeSummarizer::returning(oversize));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        summarizer,
        fixture.embedder.clone(),
        fixture.sink.clone(),
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(report.agents_skipped[0].reason, SkipReason::OversizeSummary);
    assert_eq!(fixture.sink.call_count(), 0);
    assert_eq!(fixture.recorder.call_count(), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_summarizer_failure_for_one_agent_does_not_stop_the_next_agent() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "agent a session",
            &[],
        )
        .await;
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-b",
            "agent b session",
            &[],
        )
        .await;

    // The summarizer fails only for agent-a's transcript.
    let failing_agent = Arc::new(FailingAgentSummarizer::new("agent-a"));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        failing_agent,
        fixture.embedder.clone(),
        fixture.sink.clone(),
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_considered, 2);
    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(report.agents_skipped[0].agent_id, "agent-a");
    assert_eq!(report.agents_skipped[0].reason, SkipReason::SummarizerError);
    assert_eq!(
        report.milestones_written, 1,
        "the next agent is consolidated"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_metadata_decode_failure_skips_only_that_agent_and_deletes_nothing() {
    let fixture = ConsolidationFixture::new();
    let corrupt = fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "agent a session",
            &[],
        )
        .await;
    let healthy = fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-b",
            "agent-b",
            "agent b session",
            &[],
        )
        .await;

    // Corrupt the metadata of agent-a's row.
    let db = fixture.store.ready().await.expect("ready");
    db.execute(
        "UPDATE agent_sessions SET metadata = ? WHERE id = ?",
        vec![
            Value::Text("{not json".to_string()),
            Value::Text(corrupt.clone()),
        ],
    )
    .await
    .expect("corrupt metadata");

    let report = fixture
        .real_consolidator(DEFAULT_AGE_DAYS)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(report.agents_skipped[0].agent_id, "agent-a");
    assert_eq!(
        report.agents_skipped[0].reason,
        SkipReason::MetadataDecodeError
    );
    assert_eq!(
        report.milestones_written, 1,
        "the healthy agent is consolidated"
    );

    // The corrupt agent's row is still present.
    let db = fixture.store.ready().await.expect("ready");
    let remaining = mem_store::db::query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM agent_sessions WHERE id = ?",
        vec![Value::Text(corrupt.clone())],
    )
    .await
    .expect("count corrupt row")
    .expect("count row");
    assert_eq!(
        remaining.i64("count"),
        Some(1),
        "the corrupt agent's row is NOT deleted"
    );
    let remaining = mem_store::db::query_one(
        db.as_ref(),
        "SELECT COUNT(*) AS count FROM agent_sessions WHERE id = ?",
        vec![Value::Text(healthy.clone())],
    )
    .await
    .expect("count healthy row")
    .expect("count row");
    assert_eq!(
        remaining.i64("count"),
        Some(0),
        "the healthy agent's row was consolidated"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_dimension_mismatch_on_milestone_embedding_writes_nothing_for_that_agent() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    let embedder = Arc::new(common::FakeEmbedder::with_vector(vec![0.1_f32; 4]));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        fixture.summarizer.clone(),
        embedder,
        fixture.sink.clone(),
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(
        report.agents_skipped[0].reason,
        SkipReason::DimensionMismatch
    );
    assert_eq!(fixture.sink.call_count(), 0, "no promotion was attempted");
    assert_eq!(fixture.recorder.call_count(), 0, "nothing was written");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_dimension_mismatch_on_promotion_embedding_writes_nothing_for_that_agent() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    // The milestone embedding (first call) has the right dimension count; the
    // promotion embedding (second call) does not.
    let embedder = Arc::new(common::FakeEmbedder::with_vectors(vec![
        vec![0.1_f32; 8],
        vec![0.2_f32; 4],
    ]));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        fixture.summarizer.clone(),
        embedder,
        fixture.sink.clone(),
        writer,
    );

    let report = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(
        report.agents_skipped[0].reason,
        SkipReason::DimensionMismatch
    );
    assert_eq!(fixture.sink.call_count(), 0, "no promotion was attempted");
    assert_eq!(fixture.recorder.call_count(), 0, "nothing was written");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_sink_error_aborts_run_with_zero_batch_writes() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    let sink = Arc::new(RecordingSink::failing());
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        fixture.summarizer.clone(),
        fixture.embedder.clone(),
        sink,
        writer,
    );

    let error = consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect_err("the sink error aborts the run");
    assert!(
        matches!(error, mem_episodic::EpisodicError::Store(_)),
        "{error}"
    );
    assert_eq!(
        fixture.recorder.call_count(),
        0,
        "the recording batch writer recorded zero calls"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_zero_fitting_transcript_rows_skips_agent_and_performs_zero_sink_calls() {
    let fixture = ConsolidationFixture::new();
    // A row whose transcript chunk alone exceeds the transcript cap: 70
    // completed-work items of 4_096 bytes each.
    let bulky = "b".repeat(4_096);
    fixture
        .clock
        .set(common::CONSOLIDATION_NOW - 40 * DAY_MILLIS);
    let new = mem_episodic::record::NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-bulky".to_string(),
        agent_id: "agent-a".to_string(),
        summary: "the source summary".to_string(),
        completed_work: (0..70)
            .map(|_| mem_episodic::record::WorkItem {
                kind: mem_episodic::record::WorkKind::Feature,
                description: bulky.clone(),
            })
            .collect(),
        ..Default::default()
    };
    fixture
        .store
        .insert_session(new, fixture.embedder.as_ref())
        .await
        .expect("insert bulky row");
    fixture.clock.set(common::CONSOLIDATION_NOW);

    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let report = recording_consolidator(&fixture)
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");
    drop(writer);

    assert_eq!(report.agents_skipped.len(), 1);
    assert_eq!(
        report.agents_skipped[0].reason,
        SkipReason::NoFittingTranscriptRows
    );
    assert_eq!(fixture.sink.call_count(), 0);
    assert_eq!(fixture.recorder.call_count(), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_no_batch_is_attempted_before_the_summary_is_accepted() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the source summary",
            &["a durable decision"],
        )
        .await;
    let summarizer = Arc::new(FakeSummarizer::failing("provider down"));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> =
        fixture.recorder.clone();
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        summarizer,
        fixture.embedder.clone(),
        fixture.sink.clone(),
        writer,
    );

    consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(
        fixture.recorder.call_count(),
        0,
        "no batch is attempted before the summary is accepted"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_milestone_summary_stripped_and_transcript_privacy_filtered() {
    let fixture = ConsolidationFixture::new();
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            "project-a",
            "session-a",
            "agent-a",
            "the secret is <private>hunter2</private> inside",
            &[],
        )
        .await;
    let summarizer = Arc::new(FakeSummarizer::returning(
        "the merged summary mentions <private>hunter2</private>",
    ));
    let writer: Arc<dyn mem_episodic::consolidation::EpisodicBatchWriter> = Arc::new(
        mem_episodic::consolidation::StoreBatchWriter::new(fixture.store.clone()),
    );
    let consolidator = fixture.consolidator_with(
        DEFAULT_AGE_DAYS,
        summarizer.clone(),
        fixture.embedder.clone(),
        fixture.sink.clone(),
        writer,
    );

    consolidator
        .run("project-a", &ConsolidationFixture::scope())
        .await
        .expect("run");

    // The transcript the summarizer received carries no private text.
    let transcripts = summarizer.transcripts();
    assert_eq!(transcripts.len(), 1);
    assert!(
        !transcripts[0].contains("hunter2"),
        "the transcript is privacy filtered: {}",
        transcripts[0]
    );

    // The stored milestone summary is stripped too.
    let milestones = fixture
        .store
        .list_milestones("project-a", 10)
        .await
        .expect("list milestones");
    assert_eq!(milestones.len(), 1);
    assert!(
        !milestones[0].summary.contains("hunter2"),
        "{}",
        milestones[0].summary
    );
}

/// A summarizer that fails only for one agent's transcript.
struct FailingAgentSummarizer {
    failing_agent: String,
}

impl FailingAgentSummarizer {
    fn new(failing_agent: &str) -> Self {
        Self {
            failing_agent: failing_agent.to_string(),
        }
    }
}

impl MilestoneSummarizer for FailingAgentSummarizer {
    fn summarize<'a>(
        &'a self,
        transcript: &'a str,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<String, mem_episodic::EpisodicError>> {
        let failing_agent = self.failing_agent.clone();
        let transcript = transcript.to_string();
        Box::pin(async move {
            if transcript.contains(&format!("agent {failing_agent} ")) {
                return Err(mem_episodic::EpisodicError::Summary {
                    message: "provider down".to_string(),
                });
            }
            Ok("a merged milestone summary".to_string())
        })
    }
}
