//! W7.1 — purge orchestration, the CLI contract and the erasure barriers.
//!
//! Covers: the two-store merge and leg order, the mandatory `not_erased`
//! accessor, the M22 facts-leg skip, the same-root refusals, interruption
//! convergence, the consolidation locks, the consolidator race invariant,
//! the lexical call-site invariant (M21/M26), and the process-level exit
//! codes (0/64/74/75).

mod common;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use common::{FakeEmbedder, TestDir};
use mem_embed::embedder::BoxFuture;
use mem_episodic::consolidation::{
    ConsolidationDeps, ConsolidationPolicy, Consolidator, StoreBatchWriter,
};
use mem_episodic::cross_tier::{CrossTierContext, CrossTierReport, CrossTierTask};
use mem_episodic::error::EpisodicError;
use mem_episodic::inbox::{CaptureInbox, InboxNewEntry};
use mem_episodic::milestone::{Contributor, encode_contributors};
use mem_episodic::promotion::{PermanentMemorySink, PromoteOutcome, PromotionRecord};
use mem_episodic::purge::SessionScope;
use mem_episodic::record::{AgentSession, NewAgentSession};
use mem_episodic::store::EpisodicStore;
use mem_episodic::summarizer::MilestoneSummarizer;
use mem_facts::store::{FactInput, FactSource, FactsStore};
use mem_store::config::StoreConfig;
use mem_store::db::{BatchMode, Statement, Value, query_one};
use mem_store::episodic_sql as sql;
use mem_store::operation_lock::acquire_turso_operation_lock;
use mem_store::platform::SystemProcessProbe;

const PROJECT: &str = "project-a";
const SESSION: &str = "session-a";
const AGENT: &str = "agent-a";
const MILESTONE: &str = "milestone-1";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn store_config(dir: &TestDir) -> StoreConfig {
    dir.store_config()
}

async fn episodic_store(dir: &TestDir) -> Arc<EpisodicStore> {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        store_config(dir),
    ));
    store.ready().await.expect("episodic ready");
    store
}

async fn insert_session(store: &Arc<EpisodicStore>) -> String {
    let embedder = FakeEmbedder::with_vector(vec![0.1; 8]);
    store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT.to_string(),
                session_id: SESSION.to_string(),
                agent_id: AGENT.to_string(),
                summary: "the summary".to_string(),
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert session")
}

async fn insert_milestone(store: &Arc<EpisodicStore>, row_id: &str) {
    let contributors = vec![Contributor {
        session_row_id: row_id.to_string(),
        session_id: SESSION.to_string(),
        parent_session_id: None,
        agent_id: AGENT.to_string(),
        agent_name: AGENT.to_string(),
        created_at: 1,
        summary: "retained source text".to_string(),
    }];
    let json = encode_contributors(&contributors).expect("contributors");
    let db = store.ready().await.expect("ready");
    db.batch(
        vec![Statement::new(
            sql::INSERT_MILESTONE,
            vec![
                Value::Text(MILESTONE.to_string()),
                Value::Text(PROJECT.to_string()),
                Value::Text(AGENT.to_string()),
                Value::Text("the milestone summary".to_string()),
                Value::Text(json),
                Value::Integer(1),
                Value::Integer(1),
                Value::Integer(1),
                Value::Text("[0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1]".to_string()),
                Value::Integer(1),
            ],
        )],
        BatchMode::Write,
    )
    .await
    .expect("insert milestone");
}

async fn insert_fact(facts: &FactsStore, content: &str) {
    facts
        .insert(FactInput {
            project_id: PROJECT.to_string(),
            tier: mem_core::tier::MemoryTier::Semantic,
            content: content.to_string(),
            key: None,
            source_ref: Some(MILESTONE.to_string()),
            confidence: None,
            metadata: None,
            embedding: None,
            model_id: None,
            sources: vec![FactSource {
                project_id: PROJECT.to_string(),
                session_id: SESSION.to_string(),
                agent_id: AGENT.to_string(),
                milestone_id: MILESTONE.to_string(),
            }],
        })
        .await
        .expect("insert fact");
}

struct Seeded {
    dir: TestDir,
    store: Arc<EpisodicStore>,
    facts: Option<Arc<FactsStore>>,
}

async fn seed(dir: TestDir, with_facts: bool) -> Seeded {
    let store = episodic_store(&dir).await;
    let row_id = insert_session(&store).await;
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, "raw bytes that must be erased")
        .await
        .expect("raw");
    CaptureInbox::new(store.clone())
        .enqueue(InboxNewEntry {
            capture_id: "cap-1".to_string(),
            project_id: PROJECT.to_string(),
            session_id: SESSION.to_string(),
            agent_id: AGENT.to_string(),
            payload: mem_store::serde_json::json!({ "summary": "queued" }),
        })
        .await
        .expect("enqueue");
    insert_milestone(&store, &row_id).await;

    let facts = if with_facts {
        let facts = Arc::new(FactsStore::new(store_config(&dir)));
        facts.ready().await.expect("facts ready");
        insert_fact(&facts, "the fact that must be erased").await;
        Some(facts)
    } else {
        None
    };
    Seeded { dir, store, facts }
}

fn request() -> mem_bridge::purge::PurgeRequest {
    mem_bridge::purge::PurgeRequest {
        project_id: PROJECT.to_string(),
        session_id: SESSION.to_string(),
        agent_id: AGENT.to_string(),
    }
}

// ---------------------------------------------------------------------------
// In-process orchestration
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_report_merges_the_two_stores() {
    let seeded = seed(TestDir::new("w71-purge-merge"), true).await;
    let events: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let report = mem_bridge::purge::purge_session_observed(
        &seeded.store,
        seeded.facts.as_deref(),
        &request(),
        &|leg| events.lock().expect("events").push(leg),
    )
    .await
    .expect("purge");

    assert_eq!(report.episodic.session_rows, 1);
    assert_eq!(report.episodic.raw_context, 1);
    assert_eq!(report.episodic.segments, 1);
    assert_eq!(report.episodic.inbox_rows, 1);
    assert_eq!(report.episodic.milestones_scrubbed, 1);
    assert_eq!(report.facts.facts, 1);
    assert_eq!(report.facts.fact_sources, 1);
    assert_eq!(report.facts_leg, mem_bridge::purge::FactsLeg::Ran);
    assert_eq!(
        events.lock().expect("events").clone(),
        vec!["facts", "episodic"],
        "the facts leg runs before the episodic leg"
    );

    // The stores are really empty.
    assert_eq!(
        seeded
            .store
            .count_sessions(Some(PROJECT))
            .await
            .expect("count"),
        0
    );
    assert_eq!(
        seeded
            .facts
            .as_ref()
            .expect("facts")
            .count(PROJECT)
            .await
            .expect("facts count"),
        0
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_keeps_a_fact_sourced_only_by_a_surviving_milestone() {
    // D12's discriminator: the milestones SURVIVE (as a scrub), so their
    // promoted facts must survive too. The fact's only provenance row names
    // the purged milestone but a DIFFERENT triple (an empty session/agent is
    // not a valid provenance row), so only the milestone arm could erase it.
    let seeded = seed(TestDir::new("w71-purge-milestone-fact"), false).await;
    let facts = Arc::new(FactsStore::new(store_config(&seeded.dir)));
    facts.ready().await.expect("facts ready");
    facts
        .insert(FactInput {
            project_id: PROJECT.to_string(),
            tier: mem_core::tier::MemoryTier::Semantic,
            content: "knowledge a co-contributor's milestone produced".to_string(),
            key: None,
            source_ref: Some(MILESTONE.to_string()),
            confidence: None,
            metadata: None,
            embedding: None,
            model_id: None,
            sources: vec![FactSource {
                project_id: PROJECT.to_string(),
                session_id: "session-other".to_string(),
                agent_id: AGENT.to_string(),
                milestone_id: MILESTONE.to_string(),
            }],
        })
        .await
        .expect("insert milestone-sourced fact");

    let report = mem_bridge::purge::purge_session(&seeded.store, Some(&facts), &request())
        .await
        .expect("purge");
    assert_eq!(
        report.facts.facts, 0,
        "a fact sourced only through the surviving milestone is kept"
    );
    assert_eq!(report.facts.fact_sources, 0);
    assert_eq!(report.facts_leg, mem_bridge::purge::FactsLeg::Ran);
    assert_eq!(
        facts.count(PROJECT).await.expect("facts count"),
        1,
        "the co-contributor's durable knowledge survives the purge"
    );

    // The milestone that carries it survived as a scrub, which is WHY the
    // fact is kept.
    let milestones = seeded
        .store
        .list_milestones(PROJECT, 10)
        .await
        .expect("list");
    assert_eq!(milestones.len(), 1, "the milestone row survives");
    assert_eq!(
        milestones[0].contributors[0].summary, "",
        "the purged contributor's text was scrubbed"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_report_always_names_the_surviving_tier() {
    // The accessor is the subject: the CLI JSON is built FROM it, so the two
    // cannot drift. `NOT_ERASED` is the single source in mem-bridge.
    let report = mem_bridge::purge::PurgeReport::default();
    let not_erased = report.not_erased();
    assert!(
        !not_erased.is_empty(),
        "not_erased is mandatory and non-empty"
    );
    assert_eq!(not_erased.len(), 2, "exactly two surviving tiers");
    assert_eq!(not_erased[0].tier, "memories-shard");
    assert_eq!(not_erased[1].tier, "milestones-recap");
    assert!(!not_erased[0].reason.is_empty(), "the reason is non-empty");
    assert!(!not_erased[1].reason.is_empty(), "the reason is non-empty");
    assert_eq!(
        mem_bridge::purge::NOT_ERASED,
        [not_erased[0], not_erased[1]],
        "the accessor returns the constant"
    );

    // And a real purge's accessor reports the same thing.
    let seeded = seed(TestDir::new("w71-purge-not-erased"), false).await;
    let report = mem_bridge::purge::purge_session(&seeded.store, None, &request())
        .await
        .expect("purge");
    assert_eq!(report.not_erased()[0].tier, "memories-shard");
    assert_eq!(report.not_erased()[1].tier, "milestones-recap");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_skips_the_facts_leg_when_there_is_no_facts_db() {
    let seeded = seed(TestDir::new("w71-purge-no-facts-db"), false).await;
    let facts_path = seeded.dir.path().join("facts.db");
    assert!(!facts_path.exists(), "the fixture has no facts tier");

    // The store is passed unconditionally (the CLI always does): the leg is
    // decided by the locked existence probe, not by the caller.
    let unopened = FactsStore::new(store_config(&seeded.dir));
    let report = mem_bridge::purge::purge_session(&seeded.store, Some(&unopened), &request())
        .await
        .expect("purge");
    assert_eq!(
        report.facts_leg,
        mem_bridge::purge::FactsLeg::SkippedNoDatabase
    );
    assert!(report.facts.is_empty());
    assert_eq!(
        report.episodic.raw_context, 1,
        "the episodic leg still erased"
    );
    assert!(!facts_path.exists(), "a purge never creates facts.db");
    assert!(
        !unopened.db_path().exists(),
        "the skipped leg never opened the store"
    );

    // An EMPTY but EXISTING facts.db reports Ran — "no tier" and "empty tier"
    // are different operator facts.
    let dir = TestDir::new("w71-purge-empty-facts-db");
    let facts = FactsStore::new(store_config(&dir));
    facts.ready().await.expect("create an empty facts.db");
    assert!(dir.path().join("facts.db").exists());
    let store = episodic_store(&dir).await;
    let report = mem_bridge::purge::purge_session(&store, Some(&facts), &request())
        .await
        .expect("purge");
    assert_eq!(report.facts_leg, mem_bridge::purge::FactsLeg::Ran);
    assert!(report.facts.is_empty());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_interrupted_purge_converges_on_retry() {
    let seeded = seed(TestDir::new("w71-purge-interrupted"), true).await;
    let facts = seeded.facts.as_ref().expect("facts");
    let scope = SessionScope::new(PROJECT, SESSION, AGENT).expect("scope");

    // The facts leg alone: exactly the crash-between-the-batches state.
    let milestone_ids = seeded
        .store
        .milestone_ids_for_session(&scope)
        .await
        .expect("pre-scan");
    let first_facts = facts
        .purge_sources(PROJECT, SESSION, AGENT, &milestone_ids)
        .await
        .expect("facts leg");
    assert_eq!(first_facts.facts, 1);

    // The full orchestrator finishes: episodic counts, facts zeros.
    let report = mem_bridge::purge::purge_session(&seeded.store, Some(facts), &request())
        .await
        .expect("full purge");
    assert_eq!(report.episodic.raw_context, 1);
    assert_eq!(report.episodic.milestones_scrubbed, 1);
    assert!(report.facts.is_empty(), "the facts were already gone");

    // A third call is a total no-op.
    let third = mem_bridge::purge::purge_session(&seeded.store, Some(facts), &request())
        .await
        .expect("third purge");
    assert!(third.is_empty(), "{third:?}");
    assert_eq!(third.facts_leg, mem_bridge::purge::FactsLeg::Ran);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_takes_the_consolidation_locks_before_the_pre_scan() {
    let seeded = seed(TestDir::new("w71-purge-locks"), false).await;
    let probe: Arc<dyn mem_store::platform::ProcessProbe> = Arc::new(SystemProcessProbe);
    let held =
        acquire_turso_operation_lock(seeded.store.storage_path(), probe, "episodic-consolidation")
            .expect("hold the consolidation lock");

    let error = mem_bridge::purge::purge_session(&seeded.store, None, &request())
        .await
        .expect_err("busy");
    assert!(
        matches!(error, mem_bridge::purge::PurgeError::Busy),
        "{error:?}"
    );
    assert!(
        seeded
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "nothing was deleted while busy"
    );

    held.release();
    let report = mem_bridge::purge::purge_session(&seeded.store, None, &request())
        .await
        .expect("after release the same call erases");
    assert_eq!(report.episodic.raw_context, 1);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_refuses_a_facts_store_rooted_elsewhere() {
    let seeded = seed(TestDir::new("w7-follow-f8-mismatch"), true).await;
    let foreign_dir = TestDir::new("w7-follow-f8-foreign");
    // Constructed, never readied: no facts.db exists in the other root.
    let foreign = FactsStore::new(store_config(&foreign_dir));

    let error = mem_bridge::purge::purge_session(&seeded.store, Some(&foreign), &request())
        .await
        .expect_err("a facts store rooted outside the episodic root must be refused");

    assert!(
        matches!(error, mem_bridge::purge::PurgeError::Failed(_)),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("the same storage root"),
        "{error}"
    );
    assert!(
        !foreign_dir.path().join("facts.db").exists(),
        "a refused purge never creates a database in the other root"
    );
    assert!(
        seeded
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "a refused purge erases nothing"
    );
    assert_eq!(
        seeded
            .store
            .count_sessions(Some(PROJECT))
            .await
            .expect("count"),
        1
    );
    assert_eq!(
        seeded
            .facts
            .as_ref()
            .expect("facts")
            .count(PROJECT)
            .await
            .expect("facts count"),
        1,
        "the facts tier still holds its row"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_refuses_a_missing_facts_store_when_the_database_exists() {
    let seeded = seed(TestDir::new("w7-follow-f8-none"), true).await;
    let scope = SessionScope::new(PROJECT, SESSION, AGENT).expect("scope");

    let error = mem_bridge::purge::purge_session(&seeded.store, None, &request())
        .await
        .expect_err("an existing facts.db with no FactsStore must be refused");

    assert!(
        matches!(error, mem_bridge::purge::PurgeError::Failed(_)),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("the same storage root"),
        "{error}"
    );
    assert!(
        seeded.dir.path().join("facts.db").exists(),
        "the refused call created and deleted nothing"
    );
    assert_eq!(
        seeded
            .facts
            .as_ref()
            .expect("facts")
            .count(PROJECT)
            .await
            .expect("facts count"),
        1,
        "the facts tier still holds its row"
    );
    assert!(
        seeded
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "a refused purge erases nothing"
    );
    assert_eq!(
        seeded
            .store
            .count_sessions(Some(PROJECT))
            .await
            .expect("count"),
        1
    );
    assert_eq!(
        seeded
            .store
            .milestone_ids_for_session(&scope)
            .await
            .expect("milestones")
            .len(),
        1,
        "the pre-scan is read-only: the milestone survives the refusal"
    );
}

// ---------------------------------------------------------------------------
// The race invariant
// ---------------------------------------------------------------------------

/// A real store clock the fixture can move.
struct FixedClock(Arc<AtomicI64>);

impl FixedClock {
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

/// A summarizer that HOLDS the consolidator on both locks until the test
/// releases it, and signals `started` from inside the hold.
///
/// The signal is what makes the busy outcome deterministic: it fires only
/// while the consolidator owns the gate and the operation lock.
struct BlockingSummarizer {
    started: Arc<tokio::sync::Notify>,
    release: Arc<AtomicBool>,
}

impl BlockingSummarizer {
    fn new() -> Self {
        Self {
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl MilestoneSummarizer for BlockingSummarizer {
    fn summarize<'a>(
        &'a self,
        _transcript: &'a str,
    ) -> BoxFuture<'a, Result<String, EpisodicError>> {
        Box::pin(async move {
            self.started.notify_one();
            while !self.release.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            Ok("a merged milestone summary".to_string())
        })
    }
}

/// A REAL cross-tier task: it writes one fact whose `source_ref` is the
/// milestone and whose provenance names every consolidated session — the
/// facts half of AC-29's invariant, produced under the consolidator's locks.
struct FactsWritingTask {
    facts: Arc<FactsStore>,
}

impl CrossTierTask for FactsWritingTask {
    fn promotion_due(&self, _rows: &[AgentSession], _now: i64) -> bool {
        true
    }

    fn run<'a>(
        &'a self,
        context: &'a CrossTierContext<'a>,
    ) -> BoxFuture<'a, Result<CrossTierReport, EpisodicError>> {
        Box::pin(async move {
            let sources = context
                .sessions
                .iter()
                .map(|session| FactSource {
                    project_id: context.project_id.to_string(),
                    session_id: session.session_id.clone(),
                    agent_id: session.agent_id.clone(),
                    milestone_id: context.milestone_id.to_string(),
                })
                .collect();
            self.facts
                .insert(FactInput {
                    project_id: context.project_id.to_string(),
                    tier: mem_core::tier::MemoryTier::Semantic,
                    content: "the fact the race must erase".to_string(),
                    key: Some("race fact".to_string()),
                    source_ref: Some(context.milestone_id.to_string()),
                    confidence: None,
                    metadata: None,
                    embedding: None,
                    model_id: None,
                    sources,
                })
                .await
                .map_err(|error| EpisodicError::CrossTier {
                    message: error.to_string(),
                })?;
            Ok(CrossTierReport {
                promoted: 1,
                already_present: 0,
                skipped: 0,
            })
        })
    }
}

/// A sink that accepts without touching tier-1 storage.
struct NoopSink;

impl PermanentMemorySink for NoopSink {
    fn promote<'a>(
        &'a self,
        _scope_hash: &'a str,
        _record: &'a PromotionRecord,
    ) -> BoxFuture<'a, Result<PromoteOutcome, EpisodicError>> {
        Box::pin(async { Ok(PromoteOutcome::Inserted) })
    }
}

async fn assert_nothing_left(store: &Arc<EpisodicStore>, facts: Option<&Arc<FactsStore>>) {
    let scope = SessionScope::new(PROJECT, SESSION, AGENT).expect("scope");
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_none(),
        "no raw context survives a successful purge"
    );
    assert!(
        store
            .segments_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("segments")
            .is_empty()
    );
    assert_eq!(
        store.count_sessions(Some(PROJECT)).await.expect("count"),
        0,
        "no session row survives"
    );
    let milestone_ids = store
        .milestone_ids_for_session(&scope)
        .await
        .expect("milestones");
    assert!(
        !milestone_ids.is_empty(),
        "the milestone's attribution survives (a scrub, never a delete)"
    );
    for milestone in store
        .list_milestones(PROJECT, 100)
        .await
        .expect("list milestones")
        .iter()
        .filter(|milestone| milestone_ids.contains(&milestone.id))
    {
        for contributor in milestone.contributors.iter().filter(|contributor| {
            contributor.session_id == SESSION && contributor.agent_id == AGENT
        }) {
            assert_eq!(
                contributor.summary, "",
                "the purged contributor's own text is erased"
            );
        }
    }
    if let Some(facts) = facts {
        assert_eq!(
            facts.count(PROJECT).await.expect("facts count"),
            0,
            "no fact row survives a successful purge"
        );
        let db = facts.ready().await.expect("facts ready");
        let row = query_one(
            db.as_ref(),
            "SELECT COUNT(*) AS count FROM fact_provenance WHERE project_id = ? AND session_id = ? AND agent_id = ?",
            vec![
                Value::Text(PROJECT.to_string()),
                Value::Text(SESSION.to_string()),
                Value::Text(AGENT.to_string()),
            ],
        )
        .await
        .expect("provenance")
        .expect("row");
        assert_eq!(
            row.i64("count"),
            Some(0),
            "no fact provenance naming the triple survives"
        );
    }
    let inbox = CaptureInbox::new(store.clone())
        .queued_count()
        .await
        .expect("queued");
    assert_eq!(inbox, 0, "no inbox payload survives");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_raced_against_a_consolidator_leaves_nothing_behind() {
    let dir = TestDir::new("w71-purge-race");
    let clock = FixedClock::new(1_800_000_000_000);
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), store_config(&dir)).with_clock(clock.clock()),
    );
    store.ready().await.expect("ready");

    // A session old enough for the consolidator, with raw context and a
    // queued inbox payload — every episodic half of the invariant.
    clock.set(1_800_000_000_000 - 40 * 86_400_000);
    insert_session(&store).await;
    store
        .insert_raw_context(PROJECT, SESSION, AGENT, "raw bytes that must be erased")
        .await
        .expect("raw");
    CaptureInbox::new(store.clone())
        .enqueue(InboxNewEntry {
            capture_id: "cap-race".to_string(),
            project_id: PROJECT.to_string(),
            session_id: SESSION.to_string(),
            agent_id: AGENT.to_string(),
            payload: mem_store::serde_json::json!({ "summary": "queued" }),
        })
        .await
        .expect("enqueue");
    clock.set(1_800_000_000_000);

    // A REAL facts.db: the race invariant covers the facts tier, not just
    // the episodic one.
    let facts = Arc::new(FactsStore::new(store_config(&dir)));
    facts.ready().await.expect("facts ready");

    let summarizer = Arc::new(BlockingSummarizer::new());
    let deps = ConsolidationDeps {
        summarizer: summarizer.clone(),
        embedder: Arc::new(FakeEmbedder::with_vector(vec![0.1; 8])),
        sink: Arc::new(NoopSink),
        batch_writer: Arc::new(StoreBatchWriter::new(store.clone())),
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: 200,
        probe: Arc::new(SystemProcessProbe),
        cross_tier: Arc::new(FactsWritingTask {
            facts: facts.clone(),
        }) as Arc<dyn CrossTierTask>,
    };
    let consolidator = Arc::new(Consolidator::new(
        store.clone(),
        ConsolidationPolicy::new(30).expect("policy"),
        deps,
    ));
    let promotion_scope = || mem_episodic::consolidation::PromotionScope {
        container_tag: "opencode_project_0123456789abcdef".to_string(),
        scope_hash: "0123456789abcdef".to_string(),
    };

    // ---- Outcome 1: BUSY. The consolidator is inside the summarizer, i.e.
    // it owns both locks, so the purge must refuse and delete nothing.
    let runner = {
        let consolidator = consolidator.clone();
        let scope = promotion_scope();
        tokio::spawn(async move { consolidator.run(PROJECT, &scope).await })
    };
    // Bounded on purpose: the signal fires from INSIDE the summarizer, so a
    // fixture regression that makes the consolidator skip or fail before the
    // summarization phase must FAIL here with a diagnosis, not hang the
    // suite (docs/agents/testing.md: bounded waits, never unbounded ones).
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        summarizer.started.notified(),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the consolidator never signalled `summarizer.started` within 10s: it skipped \
             or failed before the summarization phase, so the busy outcome is unreachable"
        )
    });
    let busy = mem_bridge::purge::purge_session(&store, Some(&facts), &request())
        .await
        .expect_err("the consolidator holds the gate");
    assert!(
        matches!(busy, mem_bridge::purge::PurgeError::Busy),
        "{busy:?}"
    );
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "a busy purge deletes nothing"
    );
    assert_eq!(
        store.count_sessions(Some(PROJECT)).await.expect("count"),
        1,
        "a busy purge deletes no session row"
    );

    // Release the consolidator: under the same locks it mints the milestone
    // AND the fact whose provenance names the triple.
    summarizer.release.store(true, Ordering::SeqCst);
    let consolidation = runner.await.expect("join").expect("consolidation");
    assert!(!consolidation.busy, "the consolidator ran to completion");
    assert_eq!(consolidation.sessions_consolidated, 1);
    assert_eq!(
        facts.count(PROJECT).await.expect("facts count"),
        1,
        "the race fixture really has a fact row"
    );
    let scope = SessionScope::new(PROJECT, SESSION, AGENT).expect("scope");
    assert_eq!(
        store
            .milestone_ids_for_session(&scope)
            .await
            .expect("milestones")
            .len(),
        1,
        "the consolidation minted one milestone naming the triple"
    );

    // ---- Outcome 2: SUCCESS, with the facts half. A new session row makes a
    // second consolidation possible; the purge wins the gate while that
    // consolidator races and loses it. The observed seam fires from INSIDE
    // the locked region, so the ordering is coordinated, never timed.
    insert_session(&store).await;
    let racer_start = Arc::new(tokio::sync::Notify::new());
    let racer = {
        let consolidator = consolidator.clone();
        let start = racer_start.clone();
        let scope = promotion_scope();
        tokio::spawn(async move {
            start.notified().await;
            consolidator.run(PROJECT, &scope).await
        })
    };
    let report =
        mem_bridge::purge::purge_session_observed(&store, Some(&facts), &request(), &|leg| {
            if leg == "facts" {
                racer_start.notify_one();
            }
        })
        .await
        .expect("the purge wins the gate");

    // A success must cover BOTH tiers: the fact row and its provenance are
    // gone, and the episodic counts are the exact expected values.
    assert_eq!(report.facts_leg, mem_bridge::purge::FactsLeg::Ran);
    assert_eq!(report.facts.facts, 1, "the fact row was erased");
    assert_eq!(report.facts.fact_sources, 1, "the provenance was erased");
    assert_eq!(report.episodic.raw_context, 1);
    assert_eq!(report.episodic.segments, 1);
    assert_eq!(report.episodic.session_rows, 1);
    assert_eq!(report.episodic.inbox_rows, 1);
    assert_eq!(report.episodic.milestones_scrubbed, 1);
    assert_nothing_left(&store, Some(&facts)).await;

    // The racing consolidator either lost the gate (busy) or ran AFTER the
    // purge released it; in both shapes it finds nothing to claim, so a
    // successful purge cannot leave a residue behind it either way.
    let racing = racer.await.expect("join").expect("the racing consolidator");
    assert_eq!(
        racing.agents_considered, 0,
        "the purged triple is not reclaimable by a racing consolidation"
    );
    assert_eq!(racing.sessions_consolidated, 0);
    assert_eq!(racing.milestones_written, 0);
    assert_eq!(racing.cross_tier.promoted, 0);
    assert_nothing_left(&store, Some(&facts)).await;

    // A settling purge after both racers finished is a no-op that still
    // satisfies the invariant.
    let settling = mem_bridge::purge::purge_session(&store, Some(&facts), &request())
        .await
        .expect("the settling purge");
    assert!(settling.is_empty(), "{settling:?}");
    assert_nothing_left(&store, Some(&facts)).await;
}

// ---------------------------------------------------------------------------
// The lexical call-site barrier (M21/M26)
// ---------------------------------------------------------------------------

/// A LEXICAL TRIPWIRE, not a proof.
///
/// It walks every `.rs` under the three crates' `src/**` trees and fails when
/// a CALL form (`.purge_session(`, `::purge_session(`, `.purge_sources(`,
/// `::purge_sources(`) appears outside the three-file allowlist, or when a
/// DEFINITION (`fn purge_session` / `fn purge_sources`) appears outside it
/// (the bridge's orchestrator necessarily defines one).
///
/// Known evasions, stated so nobody mistakes this for absence: bare method
/// references (`.purge_session` without the call parenthesis), `#[path]`
/// modules declared outside `src/`, and macro-generated calls.
#[test]
fn test_purge_is_never_reachable_from_capture_or_consolidation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let allowlist = [
        "crates/mem-episodic/src/purge.rs",
        "crates/mem-facts/src/purge.rs",
        "crates/mem-bridge/src/purge.rs",
    ];
    let mut files = Vec::new();
    for crate_dir in ["mem-episodic", "mem-facts", "mem-bridge"] {
        collect_rs(&root.join("crates").join(crate_dir).join("src"), &mut files);
    }
    assert!(!files.is_empty(), "the scan found no sources");

    for path in &files {
        let relative = path
            .strip_prefix(&root)
            .expect("path under the repository root")
            .to_string_lossy()
            .replace('\\', "/");
        let allowed = allowlist.contains(&relative.as_str());
        let source = std::fs::read_to_string(path).expect("read source");
        for (line_number, line) in source.lines().enumerate() {
            let calls = [
                ".purge_session(",
                "::purge_session(",
                ".purge_sources(",
                "::purge_sources(",
            ]
            .iter()
            .any(|needle| line.contains(needle));
            let defines = line.contains("fn purge_session") || line.contains("fn purge_sources");
            assert!(
                !(calls || defines) || allowed,
                "{relative}:{} carries a purge call/definition outside the allowlist: {line}",
                line_number + 1
            );
        }
    }

    // The four non-purge modules the barrier exists to protect.
    for relative in [
        "crates/mem-episodic/src/store.rs",
        "crates/mem-episodic/src/consolidation.rs",
        "crates/mem-bridge/src/capture.rs",
        "crates/mem-bridge/src/composition.rs",
    ] {
        let source = std::fs::read_to_string(root.join(relative)).expect("read source");
        for needle in [
            "fn purge_session",
            "fn purge_sources",
            ".purge_session(",
            "::purge_session(",
            ".purge_sources(",
            "::purge_sources(",
        ] {
            assert!(
                !source.contains(needle),
                "{relative} must not carry {needle}"
            );
        }
    }
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Process-level CLI tests
// ---------------------------------------------------------------------------

/// Where the root binary lives, given the running test executable and the
/// `CARGO_TARGET_DIR` value (if any).
///
/// An integration test's `current_exe()` is `<target>/<profile>/deps/<test>`,
/// while the root package's binary is written to `<target>/<profile>/`. The
/// derivation therefore strips exactly one `deps` segment; any other parent is
/// taken as the profile directory itself. With no parent at all (a root path —
/// unreachable for a real test binary) the bare file name is returned, which
/// resolves against the working directory and keeps the function total.
///
/// Pure: no environment reads, no filesystem access, no allocation beyond the
/// returned path — the caller owns both inputs.
fn resolve_binary_path(current_exe: &Path, target_dir: Option<&OsStr>) -> PathBuf {
    let name = format!("opencode-agents-memory{}", std::env::consts::EXE_SUFFIX);
    if let Some(target_dir) = target_dir {
        return Path::new(target_dir).join("debug").join(name);
    }
    let profile_dir = current_exe.parent().map(|parent| {
        if parent.file_name() == Some(OsStr::new("deps")) {
            parent.parent().unwrap_or(parent)
        } else {
            parent
        }
    });
    match profile_dir {
        Some(profile_dir) => profile_dir.join(name),
        None => PathBuf::from(name),
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root is 2 levels above crate manifest")
        .to_path_buf()
}

fn resolve_target_dir(target_dir: Option<&OsStr>, workspace_root: &Path) -> Option<PathBuf> {
    target_dir.map(|td| {
        let path = Path::new(td);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace_root.join(path)
        }
    })
}

fn build_root_binary_cmd(_manifest_dir: &Path, target_dir_env: Option<&OsStr>) -> Command {
    let ws_root = workspace_root();
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(&ws_root);
    cmd.args(["build", "-q", "-p", "opencode-agents-memory"]);
    if let Some(target_dir) = resolve_target_dir(target_dir_env, &ws_root) {
        cmd.env("CARGO_TARGET_DIR", target_dir);
    } else {
        cmd.env_remove("CARGO_TARGET_DIR");
    }
    cmd
}

/// Serializes every nested `cargo build` in this test binary. Cargo's target-dir
/// lock is keyed by the path as spelled, not the resolved real path, so two
/// builds naming the same directory through different spellings (one via the
/// `issue-125-probe` symlink, one direct) race each other's dep-graph files.
static BUILD_LOCK: Mutex<()> = Mutex::new(());

fn lock_builds() -> std::sync::MutexGuard<'static, ()> {
    BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Removes `path` whatever it currently is: a real directory (left behind by an
/// earlier run), a symlink to one, or a plain file. `symlink_metadata` reports
/// the link itself, so a symlink-to-directory still takes the `remove_file`
/// branch.
fn remove_probe_path(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

fn binary_path() -> PathBuf {
    use std::sync::OnceLock;
    static BUILT: OnceLock<()> = OnceLock::new();
    let exe = std::env::current_exe().expect("current exe");
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ws_root = workspace_root();
    let target_dir_var = std::env::var_os("CARGO_TARGET_DIR");
    let target_dir = resolve_target_dir(target_dir_var.as_deref(), &ws_root);
    let binary = resolve_binary_path(&exe, target_dir.as_deref().map(Path::as_os_str));
    // Always (re)build once per test process: `cargo test -p mem-bridge` does
    // not build the ROOT binary, and a stale one would silently test the old
    // argv surface. The build is incremental and serialized by cargo's own
    // target-dir lock.
    BUILT.get_or_init(|| {
        let _build_lock = lock_builds();
        let status = build_root_binary_cmd(manifest_dir, target_dir_var.as_deref())
            .status()
            .expect("build the root binary");
        assert!(status.success(), "cargo build failed");
    });
    assert!(
        binary.exists(),
        "the root binary is missing at {}",
        binary.display()
    );
    binary
}

// binary_path resolution (issue 57)

fn root_binary_name() -> String {
    format!("opencode-agents-memory{}", std::env::consts::EXE_SUFFIX)
}

#[test]
fn test_resolve_binary_path_prefers_the_profile_dir_when_the_exe_sits_in_deps() {
    let root = scratch_home("resolve-deps");
    let resolved = resolve_binary_path(
        &root.join("debug").join("deps").join("purge_cli-abcd1234"),
        None,
    );
    assert_eq!(resolved, root.join("debug").join(root_binary_name()));
}

#[test]
fn test_resolve_binary_path_uses_cargo_target_dir_when_it_is_set() {
    let root = scratch_home("resolve-target-dir");
    let elsewhere = root.join("elsewhere");
    let resolved = resolve_binary_path(
        &root.join("debug").join("deps").join("purge_cli-abcd1234"),
        Some(elsewhere.as_os_str()),
    );
    assert_eq!(resolved, elsewhere.join("debug").join(root_binary_name()));
}

#[test]
fn test_resolve_binary_path_treats_a_non_deps_parent_as_the_profile_dir() {
    let root = scratch_home("resolve-flat");
    let resolved = resolve_binary_path(&root.join("debug").join("purge_cli-abcd1234"), None);
    assert_eq!(resolved, root.join("debug").join(root_binary_name()));
}

#[test]
fn test_resolve_binary_path_ignores_dwo_sidecars_in_deps() {
    let root = scratch_home("resolve-dwo");
    let debug = root.join("debug");
    let deps = debug.join("deps");
    std::fs::create_dir_all(&deps).expect("deps dir");
    std::fs::write(
        deps.join("opencode_agents_memory-318052b41797682c.dwo"),
        b"",
    )
    .expect("dwo decoy");
    std::fs::write(deps.join("opencode_agents_memory-318052b41797682c"), b"")
        .expect("underscored decoy");
    std::fs::write(debug.join(root_binary_name()), b"").expect("binary placeholder");
    let resolved = resolve_binary_path(&deps.join("purge_cli-abcd1234"), None);
    assert_eq!(resolved, debug.join(root_binary_name()));
    assert!(
        resolved.exists(),
        "the resolver must land on the profile-dir binary, not a deps/ sidecar"
    );
}

#[test]
fn test_resolve_binary_path_falls_back_to_a_bare_name_when_the_exe_has_no_parent() {
    let resolved = resolve_binary_path(Path::new("/"), None);
    assert_eq!(resolved, PathBuf::from(root_binary_name()));
}

#[test]
fn test_resolve_target_dir_normalizes_relative_to_workspace_root() {
    let ws = Path::new("/workspace");
    let relative = OsStr::new(".tmp/scratch/target-w1");
    let resolved = resolve_target_dir(Some(relative), ws);
    assert_eq!(resolved, Some(ws.join(".tmp/scratch/target-w1")));
}

#[test]
fn test_resolve_target_dir_preserves_absolute_path() {
    let ws = Path::new("/workspace");
    let absolute = OsStr::new("/opt/target");
    let resolved = resolve_target_dir(Some(absolute), ws);
    assert_eq!(resolved, Some(PathBuf::from("/opt/target")));
}

#[test]
fn test_resolve_target_dir_returns_none_when_unset() {
    let ws = Path::new("/workspace");
    let resolved = resolve_target_dir(None, ws);
    assert_eq!(resolved, None);
}

#[test]
fn test_build_root_binary_cmd_sets_absolute_target_dir_when_relative() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ws_root = workspace_root();
    let relative = OsStr::new(".tmp/scratch/target-w1");
    let cmd = build_root_binary_cmd(manifest_dir, Some(relative));
    assert_eq!(cmd.get_current_dir(), Some(ws_root.as_path()));
    let env_target = cmd
        .get_envs()
        .find(|(k, _)| *k == OsStr::new("CARGO_TARGET_DIR"))
        .and_then(|(_, v)| v);
    assert_eq!(
        env_target,
        Some(ws_root.join(".tmp/scratch/target-w1").as_os_str())
    );
}

#[test]
fn test_build_root_binary_cmd_unsets_target_dir_when_none() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ws_root = workspace_root();
    let cmd = build_root_binary_cmd(manifest_dir, None);
    assert_eq!(cmd.get_current_dir(), Some(ws_root.as_path()));
    let env_entry = cmd
        .get_envs()
        .find(|(k, _)| *k == OsStr::new("CARGO_TARGET_DIR"));
    assert_eq!(env_entry, Some((OsStr::new("CARGO_TARGET_DIR"), None)));
}

#[test]
fn test_relative_cargo_target_dir_does_not_leak_into_crate_tree() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let leaked_tmp = manifest_dir.join(".tmp");
    if leaked_tmp.exists() {
        let _ = std::fs::remove_dir_all(&leaked_tmp);
    }

    let ws_root = workspace_root();
    let exe = std::env::current_exe().expect("current exe");
    let active_target = exe
        .parent()
        .and_then(|p| {
            if p.file_name() == Some(OsStr::new("deps")) {
                p.parent()
            } else {
                Some(p)
            }
        })
        .and_then(|p| p.parent())
        .expect("active target directory");

    let scratch_dir = ws_root.join(".tmp").join("scratch");
    std::fs::create_dir_all(&scratch_dir).expect("create scratch dir");
    let probe_symlink = scratch_dir.join("issue-125-probe");
    remove_probe_path(&probe_symlink);
    std::os::unix::fs::symlink(active_target, &probe_symlink).expect("create probe symlink");

    struct SymlinkGuard(PathBuf);
    impl Drop for SymlinkGuard {
        fn drop(&mut self) {
            remove_probe_path(&self.0);
        }
    }
    let _guard = SymlinkGuard(probe_symlink);

    let mut cmd = build_root_binary_cmd(
        manifest_dir,
        Some(OsStr::new(".tmp/scratch/issue-125-probe")),
    );
    let status = {
        let _build_lock = lock_builds();
        cmd.status().expect("cargo build")
    };
    assert!(status.success(), "cargo build failed");

    assert!(
        !leaked_tmp.exists(),
        "cargo build leaked directory into crates/mem-bridge/.tmp"
    );
}

fn scratch_home(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let home = std::env::temp_dir().join(format!(
        "mem-bridge-w71-{label}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("create home");
    home
}

fn broken_config_home(label: &str) -> PathBuf {
    let home = scratch_home(label);
    let config_dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(config_dir.join("opencode-mem.jsonc"), "{ not json at all")
        .expect("broken config");
    home
}

fn run_child(home: &Path, args: &[String]) -> std::process::Output {
    Command::new(binary_path())
        .args(args)
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .expect("spawn the binary")
}

fn purge_args(root: &Path, project: &str, session: &str, agent: &str) -> Vec<String> {
    vec![
        "--mode=purge-session".to_string(),
        format!("--storage-root={}", root.display()),
        "--project".to_string(),
        project.to_string(),
        "--session".to_string(),
        session.to_string(),
        "--agent".to_string(),
        agent.to_string(),
        format!("--confirm={project}:{session}:{agent}"),
    ]
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_session_process_exits_zero_and_reports_counts() {
    let seeded = seed(TestDir::new("w71-purge-cli-zero"), true).await;
    // Post-drop (issue 43 W6a) every open is legacy, so the seeding process owns
    // the database file's lock: it must release before the CLI child can open the
    // same root (the guard's restored safety property).
    let Seeded { dir, store, facts } = seeded;
    drop(store);
    drop(facts);
    let home = scratch_home("zero");
    let args = purge_args(dir.path(), PROJECT, SESSION, AGENT);
    let output = run_child(&home, &args);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line: {stdout}");
    let event: serde_json::Value = serde_json::from_str(lines[0]).expect("JSON line");
    assert_eq!(event["event"], "purge.completed");
    assert_eq!(event["outcome"], "erased");
    assert_eq!(event["project_id"], PROJECT);
    assert_eq!(event["counts"]["session_rows"], 1);
    assert_eq!(event["counts"]["raw_context"], 1);
    assert_eq!(event["counts"]["milestones_scrubbed"], 1);
    assert_eq!(event["counts"]["facts"], 1);
    assert_eq!(event["counts"]["fact_sources"], 1);
    assert_eq!(event["facts_leg"], "ran");
    let not_erased = event["not_erased"]
        .as_array()
        .expect("not_erased is an array");
    assert!(!not_erased.is_empty(), "not_erased is mandatory");
    assert_eq!(not_erased.len(), 2, "exactly two surviving tiers");
    assert_eq!(not_erased[0]["tier"], "memories-shard");
    assert_eq!(not_erased[1]["tier"], "milestones-recap");
    for tier in not_erased {
        assert!(
            tier["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "every reason is non-empty"
        );
    }

    // Rows really gone.
    let store = episodic_store(&dir).await;
    assert_eq!(store.count_sessions(Some(PROJECT)).await.expect("count"), 0);
    assert!(
        store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_session_honours_storage_root() {
    let a = seed(TestDir::new("w71-purge-root-a"), true).await;
    let b = seed(TestDir::new("w71-purge-root-b"), true).await;
    // Both seeding processes must release their file locks before the CLI child
    // opens root B (post-drop every open is legacy: one owner per file).
    let (a_dir, b_dir) = {
        let Seeded {
            dir: a_dir,
            store,
            facts,
        } = a;
        drop(store);
        drop(facts);
        let Seeded {
            dir: b_dir,
            store,
            facts,
        } = b;
        drop(store);
        drop(facts);
        (a_dir, b_dir)
    };
    let a_episodic = std::fs::read(a_dir.path().join("episodic.db")).expect("read a");
    let a_facts = std::fs::read(a_dir.path().join("facts.db")).expect("read a facts");

    let home = scratch_home("root-b");
    let args = purge_args(b_dir.path(), PROJECT, SESSION, AGENT);
    let output = run_child(&home, &args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // B was erased; A is byte-identical and still holds its rows.
    assert_eq!(
        std::fs::read(a_dir.path().join("episodic.db")).expect("read a"),
        a_episodic
    );
    assert_eq!(
        std::fs::read(a_dir.path().join("facts.db")).expect("read a facts"),
        a_facts
    );
    let a_store = episodic_store(&a_dir).await;
    assert!(
        a_store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some()
    );
    let b_store = episodic_store(&b_dir).await;
    assert!(
        b_store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_none()
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_session_refuses_without_a_matching_confirmation() {
    let home = broken_config_home("confirmation");
    let root = scratch_home("confirmation-root");
    let base = |confirm: Option<&str>| {
        let mut args = vec![
            "--mode=purge-session".to_string(),
            format!("--storage-root={}", root.display()),
            "--project=project-a".to_string(),
            "--session=session-a".to_string(),
            "--agent=agent-a".to_string(),
        ];
        if let Some(confirm) = confirm {
            args.push(format!("--confirm={confirm}"));
        }
        args
    };

    let output = run_child(&home, &base(None));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(64), "{stderr}");
    assert!(stderr.contains("purge.confirmation_required"), "{stderr}");

    for wrong in ["project-a:session-a", "wrong:wrong:wrong"] {
        let output = run_child(&home, &base(Some(wrong)));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(64), "{stderr}");
        assert!(stderr.contains("purge.confirmation_mismatch"), "{stderr}");
    }
    // The refusal precedes config load (the home holds a broken config) and
    // opens no store.
    assert!(!root.join("episodic.db").exists());
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_session_rejects_an_invalid_scope_with_exit_64() {
    let home = scratch_home("invalid-scope");
    let root = scratch_home("invalid-scope-root");
    let args = vec![
        "--mode=purge-session".to_string(),
        format!("--storage-root={}", root.display()),
        "--project=   ".to_string(),
        "--session=session-a".to_string(),
        "--agent=agent-a".to_string(),
        "--confirm=   :session-a:agent-a".to_string(),
    ];
    let output = run_child(&home, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(64), "{stderr}");
    assert!(stderr.contains("purge.invalid_scope"), "{stderr}");
    assert!(
        !root.join("episodic.db").exists(),
        "nothing is opened before scope validation"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_session_reports_a_store_failure_with_exit_74() {
    let home = scratch_home("store-failure");
    let file = scratch_home("store-failure-root").join("not-a-directory");
    std::fs::write(&file, "a regular file, not a storage root").expect("write file");
    let args = purge_args(&file, PROJECT, SESSION, AGENT);
    let output = run_child(&home, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(74), "{stderr}");
    assert!(stderr.contains("purge.failed"), "{stderr}");
    assert!(!stderr.contains("raw bytes"), "no content is reported");
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_session_reports_busy_with_exit_75() {
    let seeded = seed(TestDir::new("w71-purge-cli-busy"), false).await;
    let probe: Arc<dyn mem_store::platform::ProcessProbe> = Arc::new(SystemProcessProbe);
    let held =
        acquire_turso_operation_lock(seeded.store.storage_path(), probe, "episodic-consolidation")
            .expect("hold the lock");

    let home = scratch_home("busy");
    let args = purge_args(seeded.dir.path(), PROJECT, SESSION, AGENT);
    let output = run_child(&home, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(75), "{stderr}");
    assert!(stderr.contains("purge.busy"), "{stderr}");
    assert!(output.stdout.is_empty(), "no success line while busy");
    assert!(
        seeded
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "nothing was deleted"
    );
    held.release();
}

/// Issue 43 W6a / finding D1: with the multiprocess flag dropped, a live engine
/// holds the `.db` file itself, so `store.ready()` — not the operation lock — is
/// where the busy condition surfaces. The CLI must map that wrapped lock refusal
/// to exit **75** with the actionable wording, and delete nothing.
///
/// Inverted world (pre-fix): the `ready()` error is mapped to `Failed`, so the CLI
/// exits 74 `purge.failed` and this test fails on the exit code.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_busy_names_the_engine_holding_the_storage_root() {
    // `seeded.store` is a live engine-shaped handle on `episodic.db`: post-drop
    // every open is legacy, so it owns the database's file lock.
    let seeded = seed(TestDir::new("w6a-purge-engine-held"), false).await;

    let home = scratch_home("engine-held");
    let args = purge_args(seeded.dir.path(), PROJECT, SESSION, AGENT);
    let output = run_child(&home, &args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(75), "{stderr}");
    assert!(stderr.contains("purge.busy"), "{stderr}");
    assert!(
        stderr.contains("holds this storage root"),
        "the busy line must name the cause and the remedy: {stderr}"
    );
    assert!(output.stdout.is_empty(), "no success line while busy");
    assert!(
        seeded
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw")
            .is_some(),
        "nothing was read or deleted while the engine holds the root"
    );
}

/// ADR-0012 D-11: the purge erasure leg is not shipped yet — W8b owns it — so
/// the purge must be proved unable to reach the vault's directories. The fixture
/// is synthetic opaque bytes on purpose: the property under test is that purge
/// reaches no path under `{storage_root}/vault/`, which must hold for any
/// content there. `mem-bridge` does declare a `mem-vault` edge (`Cargo.toml:14`,
/// used by `vault_feeder.rs`), so a real store could be constructed; a real
/// vault is not used because it would couple this purge oracle to key custody
/// without widening the purge path it proves. When W8b lands the erasure leg,
/// this oracle must be re-scoped: erasure is then supposed to delete the scopes
/// named in `PurgeRequest.vault_scopes`, and this claim must narrow to the
/// scopes the request does not name.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_purge_does_not_touch_the_vault_directories() {
    let seeded = seed(TestDir::new("w72-purge-vault"), true).await;
    // Post-drop (issue 43 W6a) the seeding process owns the database file's lock,
    // so it must release before the CLI child opens the root.
    let Seeded { dir, store, facts } = seeded;
    drop(store);
    drop(facts);
    let scope = "0123456789abcdef";
    let vault_dir = dir.path().join("vault").join(scope);
    std::fs::create_dir_all(&vault_dir).expect("create the vault-shaped directory");
    let db_bytes = b"\x00\x01synthetic encrypted vault page\xff\xfe".to_vec();
    std::fs::write(vault_dir.join("vault.db"), &db_bytes).expect("write the vault-shaped database");
    std::fs::write(vault_dir.join(".vault-encrypted-v1"), "cipher=aes256gcm\n")
        .expect("write the marker");

    let listing = |dir: &Path| -> Vec<(String, Vec<u8>)> {
        let mut entries: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .expect("list the vault-shaped directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap_or_default(),
                )
            })
            .collect();
        entries.sort();
        entries
    };
    let before = listing(&vault_dir);

    let home = scratch_home("vault-untouched");
    let args = purge_args(dir.path(), PROJECT, SESSION, AGENT);
    let output = run_child(&home, &args);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stdout line: {stdout}");
    assert!(
        !stdout.contains("vault"),
        "the report must not name a tier that holds nothing: {stdout}"
    );
    let event: serde_json::Value = serde_json::from_str(lines[0]).expect("JSON line");
    assert_eq!(event["event"], "purge.completed");
    assert_eq!(event["outcome"], "erased");
    assert_eq!(
        event["counts"]["session_rows"], 1,
        "the purge really ran: {stdout}"
    );
    let not_erased = event["not_erased"]
        .as_array()
        .expect("not_erased is an array");
    assert_eq!(not_erased.len(), 2, "exactly two surviving tiers");
    assert_eq!(not_erased[0]["tier"], "memories-shard");
    assert_eq!(not_erased[1]["tier"], "milestones-recap");

    assert_eq!(
        std::fs::read(vault_dir.join("vault.db")).expect("read back the vault-shaped file"),
        db_bytes,
        "the purge leaves the vault's bytes byte-identical"
    );
    assert_eq!(
        listing(&vault_dir),
        before,
        "the purge leaves the vault's directory listing unchanged"
    );
}
