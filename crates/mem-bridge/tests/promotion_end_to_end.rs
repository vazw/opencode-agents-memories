//! W7.1 — the production cross-tier task and, later in this file, the
//! end-to-end capture → consolidate → facts → purge proofs (D1c–f, D6).

mod common;

use std::sync::Arc;

use common::{FakeEmbedder, TestDir};
use mem_bridge::cross_tier::{FACTS_MAX_CONTENT_BYTES, FactsCrossTierTask, cross_tier_task};
use mem_core::dedup::normalize_key;
use mem_core::tier::{MemoryTier, classify_fact_tier};
use mem_episodic::cross_tier::{CrossTierContext, CrossTierTask, SessionRef};
use mem_episodic::error::EpisodicError;
use mem_episodic::promotion::PromotableFact;
use mem_episodic::record::AgentSession;
use mem_facts::store::FactsStore;
use mem_store::db::query_one;

fn facts_store(dir: &TestDir) -> Arc<FactsStore> {
    Arc::new(FactsStore::new(dir.store_config()))
}

async fn open_facts(dir: &TestDir) -> Arc<FactsStore> {
    let store = facts_store(dir);
    store.ready().await.expect("facts ready");
    store
}

fn session_row(
    id: &str,
    project_id: &str,
    agent_id: &str,
    summary: &str,
    decisions: &[&str],
    created_at: i64,
) -> AgentSession {
    AgentSession {
        id: id.to_string(),
        project_id: project_id.to_string(),
        session_id: format!("session-{id}"),
        agent_id: agent_id.to_string(),
        parent_session_id: None,
        summary: summary.to_string(),
        metadata: mem_episodic::payload::SessionMetadata {
            timestamp: created_at,
            agent_name: agent_id.to_string(),
            decisions: decisions
                .iter()
                .map(|description| mem_episodic::record::Decision {
                    description: (*description).to_string(),
                })
                .collect(),
            ..Default::default()
        },
        embedding: Vec::new(),
        created_at,
        updated_at: created_at,
        deleted_at: None,
        capture_id: None,
        orphaned: false,
    }
}

fn session_ref(row_id: &str, session_id: &str, agent_id: &str) -> SessionRef {
    SessionRef {
        row_id: row_id.to_string(),
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
    }
}

fn fact(content: &str, source_row_ids: &[&str]) -> PromotableFact {
    PromotableFact {
        id: format!("fact-candidate-{}", normalize_key(content).len()),
        content: content.to_string(),
        key: normalize_key(content),
        source_row_ids: source_row_ids.iter().map(|id| id.to_string()).collect(),
    }
}

const PROJECT: &str = "project-a";

/// One day in milliseconds.
const DAY_MILLIS: i64 = 86_400_000;
const AGENT: &str = "agent-a";
const MILESTONE: &str = "milestone-1";

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_facts_cross_tier_task_writes_rows_with_the_milestone_as_source_ref() {
    let dir = TestDir::new("w71-cross-tier-write");
    let facts = open_facts(&dir).await;
    let task = FactsCrossTierTask::new(facts.clone(), 7).expect("ttl 7");
    let sessions = vec![session_ref("row-1", "session-a", AGENT)];
    let candidates = vec![fact(
        "The build uses `F32_BLOB(768)` for embeddings.",
        &["row-1"],
    )];
    let context = CrossTierContext {
        project_id: PROJECT,
        agent_id: AGENT,
        milestone_id: MILESTONE,
        sessions: &sessions,
        facts: &candidates,
        now: 1_000,
    };
    let report = task.run(&context).await.expect("run");
    assert_eq!(report.promoted, 1);
    assert_eq!(report.already_present, 0);
    assert_eq!(report.skipped, 0);

    let stored = facts
        .get_by_key(
            PROJECT,
            MemoryTier::Resource,
            "The build uses `F32_BLOB(768)` for embeddings.",
        )
        .await
        .expect("get")
        .expect("the classified tier is Resource");
    assert_eq!(stored.source_ref.as_deref(), Some(MILESTONE));
    assert_eq!(stored.tier, MemoryTier::Resource);
    assert!(
        facts
            .get_by_key(
                PROJECT,
                MemoryTier::Semantic,
                "The build uses F32_BLOB 768 for embeddings."
            )
            .await
            .expect("get")
            .is_none(),
        "the classifier, not a default, chose the tier"
    );
    assert_eq!(
        classify_fact_tier(&candidates[0].content),
        mem_core::tier::FactTier::Resource
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_facts_cross_tier_task_records_one_provenance_row_per_contributing_session() {
    let dir = TestDir::new("w71-cross-tier-provenance");
    let facts = open_facts(&dir).await;
    let task = FactsCrossTierTask::new(facts.clone(), 7).expect("ttl 7");
    let sessions = vec![
        session_ref("row-1", "session-a", AGENT),
        session_ref("row-2", "session-b", AGENT),
    ];
    let candidates = vec![fact(
        "we chose option c for the key scheme",
        &["row-1", "row-2"],
    )];
    let context = CrossTierContext {
        project_id: PROJECT,
        agent_id: AGENT,
        milestone_id: MILESTONE,
        sessions: &sessions,
        facts: &candidates,
        now: 1_000,
    };
    assert_eq!(task.run(&context).await.expect("run").promoted, 1);

    let db = facts.ready().await.expect("ready");
    let rows = db
        .query(
            "SELECT session_id, agent_id, milestone_id FROM fact_provenance ORDER BY session_id",
            Vec::new(),
        )
        .await
        .expect("provenance");
    let rows: Vec<(String, String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.text("session_id").unwrap_or_default(),
                row.text("agent_id").unwrap_or_default(),
                row.text("milestone_id").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "session-a".to_string(),
                AGENT.to_string(),
                MILESTONE.to_string()
            ),
            (
                "session-b".to_string(),
                AGENT.to_string(),
                MILESTONE.to_string()
            ),
        ]
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_facts_cross_tier_task_is_idempotent_for_the_same_milestone() {
    let dir = TestDir::new("w71-cross-tier-idempotent");
    let facts = open_facts(&dir).await;
    let task = FactsCrossTierTask::new(facts.clone(), 7).expect("ttl 7");
    let sessions = vec![session_ref("row-1", "session-a", AGENT)];
    let candidates = vec![
        fact("the default ttl is seven days", &["row-1"]),
        fact("always run the migration tests", &["row-1"]),
    ];
    let context = CrossTierContext {
        project_id: PROJECT,
        agent_id: AGENT,
        milestone_id: MILESTONE,
        sessions: &sessions,
        facts: &candidates,
        now: 1_000,
    };

    let first = task.run(&context).await.expect("first run");
    assert_eq!(first.promoted, 2);
    assert_eq!(first.already_present, 0);
    let version_before = facts
        .get_by_key(
            PROJECT,
            MemoryTier::Semantic,
            "the default ttl is seven days",
        )
        .await
        .expect("get")
        .expect("present")
        .version;

    let second = task.run(&context).await.expect("second run");
    assert_eq!(second.promoted, 0);
    assert_eq!(second.already_present, 2);
    assert_eq!(facts.count(PROJECT).await.expect("count"), 2, "no new rows");
    assert_eq!(
        facts
            .get_by_key(
                PROJECT,
                MemoryTier::Semantic,
                "the default ttl is seven days"
            )
            .await
            .expect("get")
            .expect("present")
            .version,
        version_before,
        "an already-present fact is not rewritten"
    );
    let provenance = query_one(
        facts.ready().await.expect("ready").as_ref(),
        "SELECT COUNT(*) AS count FROM fact_provenance",
        Vec::new(),
    )
    .await
    .expect("count")
    .expect("row");
    assert_eq!(provenance.i64("count"), Some(2));
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_facts_cross_tier_task_skips_oversize_candidates() {
    let dir = TestDir::new("w71-cross-tier-oversize");
    let facts = open_facts(&dir).await;
    let task = FactsCrossTierTask::new(facts.clone(), 7).expect("ttl 7");
    let sessions = vec![session_ref("row-1", "session-a", AGENT)];
    let oversize = "x".repeat(FACTS_MAX_CONTENT_BYTES + 1);
    let candidates = vec![fact(&oversize, &["row-1"])];
    let context = CrossTierContext {
        project_id: PROJECT,
        agent_id: AGENT,
        milestone_id: MILESTONE,
        sessions: &sessions,
        facts: &candidates,
        now: 1_000,
    };
    let report = task.run(&context).await.expect("run");
    assert_eq!(report.skipped, 1);
    assert_eq!(report.promoted, 0);
    assert_eq!(facts.count(PROJECT).await.expect("count"), 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_facts_cross_tier_task_promotion_due_uses_updated_at_and_the_ttl() {
    let dir = TestDir::new("w71-cross-tier-ttl");
    let facts = facts_store(&dir);
    let task = FactsCrossTierTask::new(facts, 7).expect("ttl 7");
    let now = 100 * DAY_MILLIS;

    let old = session_row("row-old", PROJECT, AGENT, "s", &[], now - 8 * DAY_MILLIS);
    let young = session_row("row-young", PROJECT, AGENT, "s", &[], now - 6 * DAY_MILLIS);
    assert!(
        task.promotion_due(std::slice::from_ref(&old), now),
        "8 days ≥ 7"
    );
    assert!(
        !task.promotion_due(std::slice::from_ref(&young), now),
        "6 days < 7"
    );
    assert!(
        !task.promotion_due(&[old, young], now),
        "a mixed set is not due (all rows must be due)"
    );
    assert!(
        task.promotion_due(&[], now),
        "no rows: vacuously due (the agent skips earlier anyway)"
    );

    // The TTL guard at construction.
    let dir = TestDir::new("w71-cross-tier-ttl-zero");
    assert!(FactsCrossTierTask::new(facts_store(&dir), 0).is_err());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_cross_tier_task_wiring_respects_the_flag() {
    let dir = TestDir::new("w71-cross-tier-wiring");
    let facts = facts_store(&dir);
    let now = 100 * DAY_MILLIS;
    let young = session_row("row-young", PROJECT, AGENT, "s", &[], now - DAY_MILLIS);

    // Flag off → Noop: always due, writes nothing.
    let off = cross_tier_task(false, Some(facts.clone()), 7).expect("wire off");
    assert!(off.promotion_due(std::slice::from_ref(&young), now));
    let sessions = vec![session_ref("row-young", "session-a", AGENT)];
    let candidates = vec![fact("a fact", &["row-young"])];
    let context = CrossTierContext {
        project_id: PROJECT,
        agent_id: AGENT,
        milestone_id: MILESTONE,
        sessions: &sessions,
        facts: &candidates,
        now,
    };
    assert_eq!(
        off.run(&context).await.expect("run").promoted,
        0,
        "flag off writes nothing"
    );

    // No store → Noop regardless of the flag.
    let no_store = cross_tier_task(true, None, 7).expect("wire none");
    assert!(no_store.promotion_due(std::slice::from_ref(&young), now));

    // Flag on + store → the production task honours the TTL.
    let on = cross_tier_task(true, Some(facts), 7).expect("wire on");
    assert!(!on.promotion_due(std::slice::from_ref(&young), now));

    // A non-positive TTL is a construction error, never a silently-open gate.
    let err: Result<_, EpisodicError> = cross_tier_task(true, None, 0);
    assert!(err.is_err());
}

/// Keeps the shared fixture imports honest for the task-level tests.
#[allow(dead_code)]
fn _witness(_: FakeEmbedder) {}

// ===========================================================================
// Step 18 — end-to-end proofs: capture → consolidate → facts → purge
// ===========================================================================

use std::sync::atomic::{AtomicI64, Ordering};

use common::{FakeEmbedder as CommonFakeEmbedder, FixedScopeResolver, RecorderTier1Writer};
use mem_bridge::capture::{CaptureService, SoundMemorySettings};
use mem_bridge::purge::PurgeRequest;
use mem_episodic::consolidation::{
    ConsolidationDeps, ConsolidationPolicy, Consolidator, PromotionScope, StoreBatchWriter,
};
use mem_episodic::promotion::{PermanentMemorySink, PromoteOutcome, PromotionRecord};
use mem_episodic::store::EpisodicStore;
use mem_store::db::Value;
use mem_store::platform::SystemProcessProbe;

const T0: i64 = 1_800_000_000_000;
const TTL_DAYS: u32 = 7;

/// A movable store clock.
struct TestClock(Arc<AtomicI64>);

impl TestClock {
    fn new(now: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now)))
    }
    fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
    fn clock(&self) -> mem_episodic::store::Clock {
        let millis = Arc::clone(&self.0);
        Arc::new(move || millis.load(Ordering::SeqCst))
    }
}

/// A deterministic milestone summarizer (the capture `FixedSummarizer` is a
/// different seam: `CaptureSummarizer`).
struct StaticMilestoneSummarizer;

impl mem_episodic::summarizer::MilestoneSummarizer for StaticMilestoneSummarizer {
    fn summarize<'a>(
        &'a self,
        _transcript: &'a str,
    ) -> BoxFuture<'a, Result<String, mem_episodic::EpisodicError>> {
        Box::pin(async { Ok("a merged milestone summary".to_string()) })
    }
}

/// A tier-1 sink that accepts without touching shard storage.
struct AcceptingSink;

impl PermanentMemorySink for AcceptingSink {
    fn promote<'a>(
        &'a self,
        _scope_hash: &'a str,
        _record: &'a PromotionRecord,
    ) -> BoxFuture<'a, Result<PromoteOutcome, mem_episodic::EpisodicError>> {
        Box::pin(async { Ok(PromoteOutcome::Inserted) })
    }
}

use mem_embed::embedder::BoxFuture;

/// The end-to-end fixture: a real episodic store (injected clock), a real
/// facts store, a real capture service with sound memory ON, and a
/// consolidator over the REAL batch writer.
struct E2e {
    #[allow(dead_code)]
    dir: TestDir,
    store: Arc<EpisodicStore>,
    facts: Arc<FactsStore>,
    capture: CaptureService,
    clock: TestClock,
}

const E2E_PROJECT: &str = "/srv/e2e";
const E2E_AGENT: &str = "agent-e2e";

fn promotion_scope() -> PromotionScope {
    PromotionScope {
        container_tag: "opencode_project_0123456789abcdef".to_string(),
        scope_hash: "0123456789abcdef".to_string(),
    }
}

impl E2e {
    async fn new(label: &str) -> Self {
        let dir = TestDir::new(label);
        let clock = TestClock::new(T0);
        let store = Arc::new(
            EpisodicStore::new(dir.path().to_path_buf(), dir.store_config())
                .with_clock(clock.clock()),
        );
        let facts = Arc::new(FactsStore::new(dir.store_config()));
        let capture = CaptureService::new(
            store.clone(),
            mem_episodic::inbox::CaptureInbox::new(store.clone()),
            Arc::new(RecorderTier1Writer::new(0)),
            Arc::new(CommonFakeEmbedder::with_vector(vec![0.1; 8])),
            Some(Arc::new(common::FixedSummarizer {
                summary: "the captured summary".to_string(),
                tags: vec!["e2e".to_string()],
                memory_type: "decision".to_string(),
                extension: mem_bridge::capture::CaptureExtension {
                    decisions: vec![
                        mem_episodic::record::Decision {
                            description: "the first durable decision".to_string(),
                        },
                        mem_episodic::record::Decision {
                            description: "the second durable decision".to_string(),
                        },
                    ],
                    ..Default::default()
                },
            })),
            Arc::new(FixedScopeResolver::default()),
            131_072,
        )
        .with_sound_memory(SoundMemorySettings {
            enabled: true,
            raw_context_required: false,
        });
        store.ready().await.expect("store ready");
        Self {
            dir,
            store,
            facts,
            capture,
            clock,
        }
    }

    async fn capture(&self, session: &str, capture_id: &str, raw: &str) {
        self.capture
            .enqueue(&capture_params(session, capture_id, Some(raw)))
            .await
            .expect("enqueue");
        let report = self.capture.drain_once().await.expect("drain");
        assert_eq!(report.done, 1, "the capture drained: {report:?}");
    }

    fn consolidator(&self, age_days: u32, promotion: bool) -> Consolidator {
        let cross_tier: Arc<dyn mem_episodic::cross_tier::CrossTierTask> = if promotion {
            Arc::new(FactsCrossTierTask::new(self.facts.clone(), TTL_DAYS).expect("ttl"))
        } else {
            Arc::new(mem_episodic::cross_tier::NoopCrossTierTask)
        };
        let deps = ConsolidationDeps {
            summarizer: Arc::new(StaticMilestoneSummarizer),
            embedder: Arc::new(CommonFakeEmbedder::with_vector(vec![0.1; 8])),
            sink: Arc::new(AcceptingSink),
            batch_writer: Arc::new(StoreBatchWriter::new(self.store.clone())),
            prefix_options: mem_embed::prefix::PrefixOptions::default(),
            batch_limit: 200,
            probe: Arc::new(SystemProcessProbe),
            cross_tier,
        };
        Consolidator::new(
            self.store.clone(),
            ConsolidationPolicy::new(age_days).expect("policy"),
            deps,
        )
    }

    async fn consolidate(
        &self,
        age_days: u32,
        promotion: bool,
    ) -> mem_episodic::ConsolidationReport {
        self.consolidator(age_days, promotion)
            .run(E2E_PROJECT, &promotion_scope())
            .await
            .expect("consolidation")
    }

    fn purge_request(&self, session: &str, agent: &str) -> PurgeRequest {
        PurgeRequest {
            project_id: E2E_PROJECT.to_string(),
            session_id: session.to_string(),
            agent_id: agent.to_string(),
        }
    }
}

fn capture_params(
    session: &str,
    capture_id: &str,
    raw_context: Option<&str>,
) -> mem_bridge::capture::CaptureParams {
    mem_bridge::capture::CaptureParams {
        session_id: session.to_string(),
        capture_id: capture_id.to_string(),
        project_id: E2E_PROJECT.to_string(),
        agent_id: E2E_AGENT.to_string(),
        agent_name: Some("e2e".to_string()),
        parent_session_id: None,
        transcript: Some("the session transcript".to_string()),
        summary: None,
        metadata: Some(mem_store::serde_json::json!({ "promptId": "p-e2e" })),
        raw_context: raw_context.map(str::to_string),
        agent_role: None,
    }
}

async fn provenance_sessions(facts: &FactsStore, project: &str) -> Vec<String> {
    let db = facts.ready().await.expect("facts ready");
    let rows = db
        .query(
            "SELECT session_id FROM fact_provenance WHERE project_id = ? ORDER BY session_id",
            vec![Value::Text(project.to_string())],
        )
        .await
        .expect("provenance");
    rows.iter()
        .filter_map(|row| row.text("session_id"))
        .collect()
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_consolidate_and_promote_end_to_end() {
    let e2e = E2e::new("w71-e2e-happy").await;
    let raw = "the first paragraph of the session\n\nsecond paragraph\nwith a second line";
    e2e.capture("session-e2e", "cap-e2e-1", raw).await;

    // Raw byte-equal + segments partition it, immediately after capture.
    let stored = e2e
        .store
        .raw_context_for_session(E2E_PROJECT, "session-e2e", E2E_AGENT)
        .await
        .expect("raw")
        .expect("present");
    assert_eq!(stored.raw_context, raw);
    let segments = e2e
        .store
        .segments_for_session(E2E_PROJECT, "session-e2e", E2E_AGENT)
        .await
        .expect("segments");
    let rebuilt: String = segments
        .iter()
        .map(|segment| segment.raw_content.as_str())
        .collect();
    assert_eq!(rebuilt, raw);

    // The session is one day old but the age gate is 30 days: advance 40.
    e2e.clock.set(T0 + 40 * DAY_MILLIS);
    let report = e2e.consolidate(30, true).await;
    assert_eq!(report.sessions_consolidated, 1);
    assert_eq!(report.cross_tier.promoted, 2, "{report:?}");
    assert_eq!(report.cross_tier.skipped, 0);

    // The session row is gone; the milestone exists; the facts carry the
    // milestone id and provenance naming the session.
    assert_eq!(
        e2e.store
            .count_sessions(Some(E2E_PROJECT))
            .await
            .expect("count"),
        0
    );
    let milestones = e2e
        .store
        .list_milestones(E2E_PROJECT, 10)
        .await
        .expect("milestones");
    assert_eq!(milestones.len(), 1);
    let milestone_id = milestones[0].id.clone();

    let facts = e2e.facts.list(E2E_PROJECT, 10).await.expect("facts");
    assert_eq!(facts.len(), 2);
    for fact in &facts {
        assert_eq!(fact.source_ref.as_deref(), Some(milestone_id.as_str()));
        assert!(!fact.is_deleted());
    }
    assert_eq!(
        provenance_sessions(&e2e.facts, E2E_PROJECT).await,
        vec!["session-e2e".to_string(), "session-e2e".to_string()]
    );

    // M1: the raw context is STILL byte-equal after the session row is gone.
    let stored = e2e
        .store
        .raw_context_for_session(E2E_PROJECT, "session-e2e", E2E_AGENT)
        .await
        .expect("raw")
        .expect("raw survives the whole pipeline");
    assert_eq!(stored.raw_context, raw);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_flags_off_produce_zero_writes() {
    // Sound memory OFF and the no-op cross-tier task: the whole path writes
    // no retention rows and never creates facts.db.
    let dir = TestDir::new("w71-e2e-flags-off");
    let clock = TestClock::new(T0);
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock.clock()),
    );
    store.ready().await.expect("ready");
    let capture = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(CommonFakeEmbedder::with_vector(vec![0.1; 8])),
        Some(Arc::new(common::FixedSummarizer {
            summary: "the captured summary".to_string(),
            ..Default::default()
        })),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );
    assert_eq!(
        capture.sound_memory_settings(),
        SoundMemorySettings::default(),
        "the builder default is OFF"
    );
    capture
        .enqueue(&capture_params(
            "session-off",
            "cap-off-1",
            Some("raw that must not be stored"),
        ))
        .await
        .expect("enqueue");
    assert_eq!(capture.drain_once().await.expect("drain").done, 1);

    clock.set(T0 + 40 * DAY_MILLIS);
    let deps = ConsolidationDeps {
        summarizer: Arc::new(StaticMilestoneSummarizer),
        embedder: Arc::new(CommonFakeEmbedder::with_vector(vec![0.1; 8])),
        sink: Arc::new(AcceptingSink),
        batch_writer: Arc::new(StoreBatchWriter::new(store.clone())),
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe: Arc::new(SystemProcessProbe),
        cross_tier: cross_tier_task(false, None, TTL_DAYS).expect("noop wiring"),
    };
    let report = Consolidator::new(
        store.clone(),
        ConsolidationPolicy::new(30).expect("policy"),
        deps,
    )
    .run(E2E_PROJECT, &promotion_scope())
    .await
    .expect("consolidation still works");
    assert_eq!(report.sessions_consolidated, 1);
    assert_eq!(
        report.cross_tier,
        mem_episodic::cross_tier::CrossTierReport::default()
    );

    assert!(
        store
            .raw_context_for_session(E2E_PROJECT, "session-off", E2E_AGENT)
            .await
            .expect("raw")
            .is_none(),
        "flags off write no retention row"
    );
    assert!(
        store
            .segments_for_session(E2E_PROJECT, "session-off", E2E_AGENT)
            .await
            .expect("segments")
            .is_empty()
    );
    assert!(
        !dir.path().join("facts.db").exists(),
        "flags off create no facts.db"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_promotion_is_deferred_until_the_ttl_elapses() {
    // The PINNED M15 fixture: age_days = 1, ttl = 7, created_at ==
    // updated_at == T0. Eligibility keys on created_at; the TTL keys on
    // updated_at.
    let e2e = E2e::new("w71-e2e-ttl").await;
    e2e.capture("session-ttl", "cap-ttl-1", "the raw context")
        .await;

    // T0 + 3 days: eligible by age (created_at < T0+2d), NOT due by TTL
    // (updated_at age 3d < 7d).
    e2e.clock.set(T0 + 3 * DAY_MILLIS);
    let report = e2e.consolidate(1, true).await;
    assert_eq!(report.agents_considered, 1);
    assert_eq!(
        report.agents_skipped,
        vec![mem_episodic::consolidation::SkippedAgent {
            agent_id: E2E_AGENT.to_string(),
            reason: mem_episodic::consolidation::SkipReason::PromotionNotDue,
        }]
    );
    assert_eq!(report.sessions_consolidated, 0);
    assert_eq!(report.milestones_written, 0);
    assert_eq!(
        report.cross_tier,
        mem_episodic::cross_tier::CrossTierReport::default()
    );
    assert_eq!(e2e.facts.count(E2E_PROJECT).await.expect("facts"), 0);
    assert_eq!(
        e2e.store
            .count_sessions(Some(E2E_PROJECT))
            .await
            .expect("count"),
        1,
        "the session row is still present"
    );
    assert!(
        e2e.store
            .raw_context_for_session(E2E_PROJECT, "session-ttl", E2E_AGENT)
            .await
            .expect("raw")
            .is_some(),
        "the raw context is still present"
    );

    // T0 + 8 days: eligible and due (age 8d ≥ 7d) → promoted.
    e2e.clock.set(T0 + 8 * DAY_MILLIS);
    let report = e2e.consolidate(1, true).await;
    assert_eq!(report.sessions_consolidated, 1);
    assert_eq!(report.cross_tier.promoted, 2);
    let facts = e2e.facts.list(E2E_PROJECT, 10).await.expect("facts");
    assert_eq!(facts.len(), 2);
    let milestone_id = facts[0].source_ref.clone().expect("source_ref");
    assert!(
        e2e.store
            .list_milestones(E2E_PROJECT, 10)
            .await
            .expect("milestones")
            .iter()
            .any(|milestone| milestone.id == milestone_id),
        "source_ref stays intact"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_after_the_full_pipeline_erases_the_session_and_its_facts() {
    let e2e = E2e::new("w71-e2e-purge").await;
    let raw = "the raw context of session e2e\n\nsecond paragraph";
    e2e.capture("session-e2e", "cap-e2e-1", raw).await;
    let segment_count = e2e
        .store
        .segments_for_session(E2E_PROJECT, "session-e2e", E2E_AGENT)
        .await
        .expect("segments")
        .len() as u64;
    assert!(segment_count >= 1);
    e2e.clock.set(T0 + 40 * DAY_MILLIS);
    e2e.consolidate(30, true).await;

    // Another project's identical session id must stay untouched.
    let other = e2e.store.clone();
    let embedder = CommonFakeEmbedder::with_vector(vec![0.1; 8]);
    other
        .insert_session(
            mem_episodic::record::NewAgentSession {
                project_id: "other-project".to_string(),
                session_id: "session-e2e".to_string(),
                agent_id: E2E_AGENT.to_string(),
                summary: "other".to_string(),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("other session");

    let report = mem_bridge::purge::purge_session(
        &e2e.store,
        Some(&e2e.facts),
        &e2e.purge_request("session-e2e", E2E_AGENT),
    )
    .await
    .expect("purge");
    assert_eq!(report.episodic.session_rows, 0, "the row was already gone");
    assert_eq!(report.episodic.raw_context, 1);
    assert_eq!(report.episodic.segments, segment_count);
    assert_eq!(report.episodic.milestones, 1);
    assert_eq!(report.facts.facts, 2);
    assert_eq!(report.facts.fact_sources, 2);
    assert!(!report.not_erased().is_empty());

    assert!(
        e2e.store
            .raw_context_for_session(E2E_PROJECT, "session-e2e", E2E_AGENT)
            .await
            .expect("raw")
            .is_none()
    );
    assert!(
        e2e.store
            .list_milestones(E2E_PROJECT, 10)
            .await
            .expect("milestones")
            .is_empty()
    );
    assert!(
        e2e.facts
            .list(E2E_PROJECT, 10)
            .await
            .expect("facts")
            .is_empty()
    );

    // The other project is untouched.
    assert_eq!(
        e2e.store
            .count_sessions(Some("other-project"))
            .await
            .expect("count"),
        1
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purged_milestone_summary_is_only_restorable_by_explicit_recapture() {
    // Two sessions of the same agent consolidate into ONE milestone; both
    // decide the same two things, so the facts are multi-source.
    let e2e = E2e::new("w71-e2e-recapture").await;
    e2e.capture("session-a", "cap-a-1", "raw of session a")
        .await;
    e2e.capture("session-b", "cap-b-1", "raw of session b")
        .await;
    e2e.clock.set(T0 + 40 * DAY_MILLIS);
    let first = e2e.consolidate(30, true).await;
    assert_eq!(first.sessions_consolidated, 2);
    assert_eq!(
        e2e.store
            .list_milestones(E2E_PROJECT, 10)
            .await
            .expect("milestones")
            .len(),
        1
    );
    assert_eq!(
        provenance_sessions(&e2e.facts, E2E_PROJECT).await,
        vec![
            "session-a".to_string(),
            "session-a".to_string(),
            "session-b".to_string(),
            "session-b".to_string()
        ]
    );

    // Purge A: the multi-source facts and the shared milestone go with it.
    mem_bridge::purge::purge_session(
        &e2e.store,
        Some(&e2e.facts),
        &e2e.purge_request("session-a", E2E_AGENT),
    )
    .await
    .expect("purge a");
    assert!(
        e2e.facts
            .list(E2E_PROJECT, 10)
            .await
            .expect("facts")
            .is_empty()
    );
    assert!(
        e2e.store
            .list_milestones(E2E_PROJECT, 10)
            .await
            .expect("milestones")
            .is_empty()
    );

    // Nothing self-heals: B has no session row (consolidation consumed it),
    // so another run promotes nothing.
    e2e.clock.set(T0 + 50 * DAY_MILLIS);
    let nothing = e2e.consolidate(1, true).await;
    assert_eq!(nothing.agents_considered, 0);
    assert_eq!(nothing.cross_tier.promoted, 0);
    assert!(
        e2e.facts
            .list(E2E_PROJECT, 10)
            .await
            .expect("facts")
            .is_empty()
    );

    // B's raw context survived the purge of A; an EXPLICIT re-capture
    // restores the session, and a later consolidation promotes the facts
    // again with provenance naming ONLY B.
    assert!(
        e2e.store
            .raw_context_for_session(E2E_PROJECT, "session-b", E2E_AGENT)
            .await
            .expect("raw")
            .is_some()
    );
    e2e.capture("session-b", "cap-b-2", "raw of session b")
        .await;
    e2e.clock.set(T0 + 80 * DAY_MILLIS);
    let restored = e2e.consolidate(1, true).await;
    assert_eq!(restored.cross_tier.promoted, 2);
    assert_eq!(
        provenance_sessions(&e2e.facts, E2E_PROJECT).await,
        vec!["session-b".to_string(), "session-b".to_string()],
        "the restored facts name only the surviving session"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_after_a_purge_resurrects_and_a_second_purge_erases_again() {
    let e2e = E2e::new("w71-e2e-resurrect").await;
    e2e.capture("session-r", "cap-r-1", "the first raw context")
        .await;

    let first = mem_bridge::purge::purge_session(
        &e2e.store,
        Some(&e2e.facts),
        &e2e.purge_request("session-r", E2E_AGENT),
    )
    .await
    .expect("first purge");
    assert_eq!(first.episodic.raw_context, 1);
    assert_eq!(first.episodic.session_rows, 1);

    // No tombstone: the same triple is a normal write again.
    e2e.capture("session-r", "cap-r-2", "the second raw context")
        .await;
    assert_eq!(
        e2e.store
            .raw_context_for_session(E2E_PROJECT, "session-r", E2E_AGENT)
            .await
            .expect("raw")
            .expect("present")
            .raw_context,
        "the second raw context"
    );
    assert_eq!(
        e2e.store
            .count_sessions(Some(E2E_PROJECT))
            .await
            .expect("count"),
        1
    );

    let second = mem_bridge::purge::purge_session(
        &e2e.store,
        Some(&e2e.facts),
        &e2e.purge_request("session-r", E2E_AGENT),
    )
    .await
    .expect("second purge");
    assert_eq!(second.episodic, first.episodic, "the same report shape");
    assert_eq!(second.facts_leg, first.facts_leg);
    assert_eq!(second.not_erased(), first.not_erased());
    assert!(
        e2e.store
            .raw_context_for_session(E2E_PROJECT, "session-r", E2E_AGENT)
            .await
            .expect("raw")
            .is_none()
    );
}
