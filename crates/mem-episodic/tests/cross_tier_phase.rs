//! W7.1 — the cross-tier phase inside the consolidator: it runs AFTER the
//! durable batch, sees the milestone, surfaces all three counters, and is
//! gated by the pure Q7 due-check BEFORE anything is written.

mod common;

use std::sync::{Arc, Mutex};

use common::{ConsolidationFixture, DAY_MILLIS, FakeEmbedder, FakeSummarizer, RecordingSink};
use mem_embed::embedder::BoxFuture;
use mem_episodic::EpisodicError;
use mem_episodic::consolidation::{
    ConsolidationDeps, ConsolidationReport, Consolidator, EpisodicBatchWriter, SkipReason,
};
use mem_episodic::cross_tier::{
    CrossTierContext, CrossTierReport, CrossTierTask, NoopCrossTierTask, SessionRef,
};
use mem_episodic::promotion::PromotableFact;
use mem_store::platform::SystemProcessProbe;

const PROJECT: &str = "project-a";
const AGENT: &str = "agent-a";

/// What the stub saw, owned so the future can outlive the borrowed context.
#[derive(Debug, Clone, PartialEq)]
struct CapturedContext {
    project_id: String,
    agent_id: String,
    milestone_id: String,
    sessions: Vec<SessionRef>,
    facts: Vec<PromotableFact>,
    now: i64,
    milestone_queryable: bool,
}

/// A recording cross-tier task with a controllable due answer and outcome.
struct StubCrossTierTask {
    due: bool,
    outcome: Result<CrossTierReport, String>,
    captured: Mutex<Vec<CapturedContext>>,
    events: Option<Arc<Mutex<Vec<String>>>>,
    store: Option<Arc<mem_episodic::store::EpisodicStore>>,
}

impl StubCrossTierTask {
    fn new(due: bool) -> Self {
        Self {
            due,
            outcome: Ok(CrossTierReport::default()),
            captured: Mutex::new(Vec::new()),
            events: None,
            store: None,
        }
    }

    fn with_report(due: bool, report: CrossTierReport) -> Self {
        Self {
            due,
            outcome: Ok(report),
            captured: Mutex::new(Vec::new()),
            events: None,
            store: None,
        }
    }

    fn failing(due: bool) -> Self {
        Self {
            due,
            outcome: Err("cross-tier provider failed".to_string()),
            captured: Mutex::new(Vec::new()),
            events: None,
            store: None,
        }
    }

    fn observing_store(
        due: bool,
        store: Arc<mem_episodic::store::EpisodicStore>,
        events: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            due,
            outcome: Ok(CrossTierReport::default()),
            captured: Mutex::new(Vec::new()),
            events: Some(events),
            store: Some(store),
        }
    }

    fn captured(&self) -> Vec<CapturedContext> {
        self.captured.lock().expect("captured lock").clone()
    }
}

impl CrossTierTask for StubCrossTierTask {
    fn promotion_due(&self, _rows: &[mem_episodic::record::AgentSession], _now: i64) -> bool {
        self.due
    }

    fn run<'a>(
        &'a self,
        context: &'a CrossTierContext<'a>,
    ) -> BoxFuture<'a, Result<CrossTierReport, EpisodicError>> {
        let captured = CapturedContext {
            project_id: context.project_id.to_string(),
            agent_id: context.agent_id.to_string(),
            milestone_id: context.milestone_id.to_string(),
            sessions: context.sessions.to_vec(),
            facts: context.facts.to_vec(),
            now: context.now,
            // The query is deferred into the async block below; this flag is
            // filled there and pushed as a second copy.
            milestone_queryable: false,
        };
        let due = self.due;
        let outcome = self.outcome.clone();
        let store = self.store.clone();
        let events = self.events.clone();
        let captured_lock = &self.captured;
        Box::pin(async move {
            if let Some(events) = &events {
                events
                    .lock()
                    .expect("event log")
                    .push("cross-tier".to_string());
            }
            let milestone_queryable = match &store {
                Some(store) => store
                    .list_milestones(PROJECT, 10)
                    .await
                    .map(|rows| rows.iter().any(|row| row.id == captured.milestone_id))
                    .unwrap_or(false),
                None => false,
            };
            captured_lock
                .lock()
                .expect("captured lock")
                .push(CapturedContext {
                    milestone_queryable,
                    ..captured
                });
            let _ = due;
            match outcome {
                Ok(report) => Ok(report),
                Err(message) => Err(EpisodicError::CrossTier { message }),
            }
        })
    }
}

fn consolidator_with(fixture: &ConsolidationFixture, task: Arc<dyn CrossTierTask>) -> Consolidator {
    let writer: Arc<dyn EpisodicBatchWriter> = fixture.recorder.clone();
    let deps = ConsolidationDeps {
        summarizer: Arc::new(FakeSummarizer::returning("a merged milestone summary")),
        embedder: Arc::new(FakeEmbedder::with_vector(vec![0.1_f32; 8])),
        sink: Arc::new(RecordingSink::new()),
        batch_writer: writer,
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe: Arc::new(SystemProcessProbe),
        cross_tier: task,
    };
    Consolidator::new(
        fixture.store.clone(),
        mem_episodic::consolidation::ConsolidationPolicy::new(30).expect("policy"),
        deps,
    )
}

fn consolidator_with_writer(
    fixture: &ConsolidationFixture,
    task: Arc<dyn CrossTierTask>,
    writer: Arc<dyn EpisodicBatchWriter>,
) -> Consolidator {
    let deps = ConsolidationDeps {
        summarizer: fixture.summarizer.clone(),
        embedder: fixture.embedder.clone(),
        sink: fixture.sink.clone(),
        batch_writer: writer,
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe: Arc::new(SystemProcessProbe),
        cross_tier: task,
    };
    Consolidator::new(
        fixture.store.clone(),
        mem_episodic::consolidation::ConsolidationPolicy::new(30).expect("policy"),
        deps,
    )
}

async fn seed(fixture: &ConsolidationFixture) -> String {
    fixture
        .insert_at(
            -40 * DAY_MILLIS,
            PROJECT,
            "session-a",
            AGENT,
            "the source summary",
            &["a durable decision"],
        )
        .await
}

/// The REAL durable batch writer plus an event-log entry, so both the actual
/// commit and the phase ORDER are observable.
struct LoggingStoreWriter {
    inner: mem_episodic::consolidation::StoreBatchWriter,
    events: Arc<Mutex<Vec<String>>>,
}

impl EpisodicBatchWriter for LoggingStoreWriter {
    fn write_batch<'a>(
        &'a self,
        statements: Vec<mem_store::db::Statement>,
    ) -> BoxFuture<'a, Result<Vec<mem_store::db::ExecResult>, EpisodicError>> {
        Box::pin(async move {
            let results = self.inner.write_batch(statements).await?;
            self.events
                .lock()
                .expect("events")
                .push("batch".to_string());
            Ok(results)
        })
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_cross_tier_phase_runs_after_the_durable_batch() {
    let fixture = ConsolidationFixture::new();
    seed(&fixture).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let writer: Arc<dyn EpisodicBatchWriter> = Arc::new(LoggingStoreWriter {
        inner: mem_episodic::consolidation::StoreBatchWriter::new(fixture.store.clone()),
        events: events.clone(),
    });
    let task = Arc::new(StubCrossTierTask::observing_store(
        true,
        fixture.store.clone(),
        events.clone(),
    ));

    let report = consolidator_with_writer(&fixture, task.clone(), writer)
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.milestones_written, 1);
    assert_eq!(
        events.lock().expect("events").clone(),
        vec!["batch".to_string(), "cross-tier".to_string()],
        "the phase runs after the durable batch"
    );
    let captured = task.captured();
    assert_eq!(captured.len(), 1);
    assert!(
        captured[0].milestone_queryable,
        "the milestone is already queryable when the phase runs"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_cross_tier_phase_receives_the_consolidated_rows_and_facts() {
    let fixture = ConsolidationFixture::new();
    let row_id = seed(&fixture).await;
    let task = Arc::new(StubCrossTierTask::new(true));

    consolidator_with(&fixture, task.clone())
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect("run");

    let captured = task.captured();
    assert_eq!(captured.len(), 1);
    let context = &captured[0];
    assert_eq!(context.project_id, PROJECT);
    assert_eq!(context.agent_id, AGENT);
    assert!(!context.milestone_id.is_empty());
    assert_eq!(
        context.sessions,
        vec![SessionRef {
            row_id: row_id.clone(),
            session_id: "session-a".to_string(),
            agent_id: AGENT.to_string(),
        }],
        "sessions are the consolidated rows' SessionRef prefix"
    );
    assert_eq!(context.facts.len(), 1, "one promotable fact selected");
    assert_eq!(context.facts[0].content, "a durable decision");
    assert_eq!(context.facts[0].source_row_ids, vec![row_id]);
    assert!(context.now > 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_cross_tier_report_surfaces_all_three_counters() {
    let fixture = ConsolidationFixture::new();
    seed(&fixture).await;
    let task = Arc::new(StubCrossTierTask::with_report(
        true,
        CrossTierReport {
            promoted: 2,
            already_present: 1,
            skipped: 3,
        },
    ));

    let report: ConsolidationReport = consolidator_with(&fixture, task)
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(
        report.cross_tier,
        CrossTierReport {
            promoted: 2,
            already_present: 1,
            skipped: 3,
        },
        "all three counters survive into the consolidation report"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_noop_cross_tier_task_leaves_the_report_at_zero() {
    let fixture = ConsolidationFixture::new();
    seed(&fixture).await;
    let report = consolidator_with(&fixture, Arc::new(NoopCrossTierTask))
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect("run");
    assert_eq!(report.cross_tier, CrossTierReport::default());
    assert_eq!(report.milestones_written, 1, "consolidation still happened");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_not_due_agent_is_skipped_before_the_write_batch() {
    let fixture = ConsolidationFixture::new();
    seed(&fixture).await;
    let task = Arc::new(StubCrossTierTask::new(false));

    let report = consolidator_with(&fixture, task.clone())
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect("run");

    assert_eq!(report.agents_considered, 1);
    assert_eq!(
        report.agents_skipped,
        vec![mem_episodic::consolidation::SkippedAgent {
            agent_id: AGENT.to_string(),
            reason: SkipReason::PromotionNotDue,
        }]
    );
    assert_eq!(report.milestones_written, 0);
    assert_eq!(report.sessions_consolidated, 0);
    assert_eq!(fixture.recorder.call_count(), 0, "zero batch statements");
    assert_eq!(fixture.sink.call_count(), 0, "zero legacy sink calls");
    assert_eq!(
        fixture
            .store
            .count_sessions(Some(PROJECT))
            .await
            .expect("count"),
        1,
        "the session row is still present for a later run"
    );
    assert_eq!(
        fixture
            .store
            .list_milestones(PROJECT, 10)
            .await
            .expect("milestones")
            .len(),
        0
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_cross_tier_error_propagates_after_the_batch_committed() {
    let fixture = ConsolidationFixture::new();
    seed(&fixture).await;
    let task = Arc::new(StubCrossTierTask::failing(true));

    let writer: Arc<dyn EpisodicBatchWriter> = Arc::new(
        mem_episodic::consolidation::StoreBatchWriter::new(fixture.store.clone()),
    );
    let error = consolidator_with_writer(&fixture, task, writer)
        .run(PROJECT, &ConsolidationFixture::scope())
        .await
        .expect_err("the cross-tier error propagates");
    assert!(
        matches!(error, EpisodicError::CrossTier { .. }),
        "{error:?}"
    );

    // The durable batch DID commit: the milestone exists and the session row
    // is gone.
    let milestones = fixture
        .store
        .list_milestones(PROJECT, 10)
        .await
        .expect("milestones");
    assert_eq!(milestones.len(), 1);
    assert_eq!(
        fixture
            .store
            .count_sessions(Some(PROJECT))
            .await
            .expect("count"),
        0
    );
}
