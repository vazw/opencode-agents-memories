//! W5.3 Step 12 / AC10 — the Q8 consolidation scheduler.
//!
//! Startup tick + periodic tick over a BOUNDED project scan (64 per tick),
//! a clean busy-lock skip (never a stolen lock), and prompt stop on engine
//! shutdown. The promoted facts carry the SAME `container_tag` the injector
//! reads with (`test_consolidation_promotion_record_tag_equals_inject_read_tag`).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;

use common::{FakeEmbedder, FixedScopeResolver, TestDir};
use mem_bridge::engine::{ConsolidationRunner, ConsolidationScheduler};
use mem_bridge::project_scope::ResolvedProjectScope;
use mem_episodic::error::EpisodicError;
use mem_episodic::record::{Decision, NewAgentSession};
use mem_episodic::store::EpisodicStore;
use mem_store::db::Value;

/// A runner that counts calls and optionally reports busy.
struct CountingRunner {
    calls: AtomicUsize,
    busy: bool,
}

impl ConsolidationRunner for CountingRunner {
    fn run<'a>(
        &'a self,
        _project_id: &'a str,
        _scope: &'a ResolvedProjectScope,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<bool, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let busy = self.busy;
        Box::pin(async move { Ok(busy) })
    }
}

/// A clock the test can move (unix millis).
struct FixedClock {
    millis: Arc<AtomicI64>,
}

impl FixedClock {
    fn new(millis: i64) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(millis)),
        }
    }
    fn set(&self, millis: i64) {
        self.millis.store(millis, Ordering::SeqCst);
    }
    fn clock(&self) -> mem_episodic::store::Clock {
        let millis = self.millis.clone();
        Arc::new(move || millis.load(Ordering::SeqCst))
    }
}

const NOW: i64 = 1_000_000_000_000;
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

async fn store(dir: &TestDir) -> Arc<EpisodicStore> {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    store
}

async fn insert_session(store: &EpisodicStore, project: &str, session: &str, agent: &str) {
    let embedder = FakeEmbedder::with_vector(common::unit_vector(0));
    store
        .insert_session(
            NewAgentSession {
                project_id: project.to_string(),
                session_id: session.to_string(),
                agent_id: agent.to_string(),
                summary: "did some work".to_string(),
                decisions: vec![Decision {
                    description: "use JWT for auth".to_string(),
                }],
                ..Default::default()
            },
            &embedder,
        )
        .await
        .expect("insert session");
}

fn scheduler(
    store: Arc<EpisodicStore>,
    runner: Arc<dyn ConsolidationRunner>,
    shutting_down: Arc<AtomicBool>,
    consolidation_active: Arc<AtomicBool>,
    interval: Duration,
) -> Arc<ConsolidationScheduler> {
    Arc::new(ConsolidationScheduler::new(
        store,
        Arc::new(FixedScopeResolver::default()),
        runner,
        consolidation_active,
        shutting_down,
        interval,
        64,
    ))
}

/// The startup tick visits every visible project once.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_startup_replay_runs() {
    let dir = TestDir::new("w53-sched-startup");
    let store = store(&dir).await;
    insert_session(&store, "/p/a", "s-a", "agent-1").await;
    insert_session(&store, "/p/b", "s-b", "agent-1").await;

    let runner = Arc::new(CountingRunner {
        calls: AtomicUsize::new(0),
        busy: false,
    });
    let scheduler = scheduler(
        store,
        runner.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(3600),
    );

    let report = scheduler.run_tick().await.expect("startup tick");
    assert_eq!(report.projects_scanned, 2, "two visible projects");
    assert_eq!(report.runs, 2, "one run per project");
    assert_eq!(report.failed, 0);
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "the consolidator ran twice"
    );
}

/// One tick scans at most 64 project ids (the 65th project waits).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_periodic_tick_runs_bounded_projects() {
    let dir = TestDir::new("w53-sched-bound");
    let store = store(&dir).await;
    for index in 0..70 {
        insert_session(
            &store,
            &format!("/p/{index:03}"),
            &format!("s-{index}"),
            "agent-1",
        )
        .await;
    }

    let runner = Arc::new(CountingRunner {
        calls: AtomicUsize::new(0),
        busy: false,
    });
    let scheduler = scheduler(
        store,
        runner.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(3600),
    );

    let report = scheduler.run_tick().await.expect("tick");
    assert_eq!(
        report.projects_scanned, 64,
        "CONSOLIDATION_MAX_PROJECTS bounds the scan"
    );
    assert_eq!(report.runs, 64, "at most one run per scanned id");
    assert_eq!(runner.calls.load(Ordering::SeqCst), 64);
}

/// A live lock is a CLEAN busy skip: no error, nothing stolen, the shared
/// flag cleared on return.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_busy_lock_skips_cleanly() {
    let dir = TestDir::new("w53-sched-busy");
    let store = store(&dir).await;
    insert_session(&store, "/p/a", "s-a", "agent-1").await;

    let runner = Arc::new(CountingRunner {
        calls: AtomicUsize::new(0),
        busy: true,
    });
    let flag = Arc::new(AtomicBool::new(false));
    let scheduler = scheduler(
        store,
        runner,
        Arc::new(AtomicBool::new(false)),
        flag.clone(),
        Duration::from_secs(3600),
    );

    let report = scheduler.run_tick().await.expect("busy is not an error");
    assert_eq!(
        report.busy, 1,
        "the busy skip is reported on the `busy` FIELD"
    );
    assert_eq!(report.runs, 0);
    assert_eq!(report.failed, 0, "a busy lock never counts as a failure");
    assert!(
        !flag.load(Ordering::SeqCst),
        "the RAII guard clears `consolidation_active` on every return"
    );
}

/// The periodic loop stops promptly when the engine shuts down.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_scheduler_stops_with_engine() {
    let dir = TestDir::new("w53-sched-stop");
    let store = store(&dir).await;
    let runner = Arc::new(CountingRunner {
        calls: AtomicUsize::new(0),
        busy: false,
    });
    let shutting_down = Arc::new(AtomicBool::new(false));
    let scheduler = scheduler(
        store,
        runner,
        shutting_down.clone(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(100),
    );

    let task = mem_store::tokio::spawn(scheduler.run_periodic());
    mem_store::tokio::time::sleep(Duration::from_millis(70)).await;
    shutting_down.store(true, Ordering::SeqCst);
    let stopped = mem_store::tokio::time::timeout(Duration::from_secs(3), task).await;
    assert!(
        stopped.is_ok(),
        "the scheduler task terminates after shutdown"
    );
    assert!(
        stopped.expect("join").is_ok(),
        "the periodic task ended cleanly"
    );
}

/// A runner that signals when it starts and finishes and yields while in
/// flight (so the test can observe the `consolidation_active` window).
struct SlowRunner {
    started: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl ConsolidationRunner for SlowRunner {
    fn run<'a>(
        &'a self,
        _project_id: &'a str,
        _scope: &'a ResolvedProjectScope,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<bool, String>> {
        let started = self.started.clone();
        let finished = self.finished.clone();
        Box::pin(async move {
            started.store(true, Ordering::SeqCst);
            for _ in 0..50 {
                mem_store::tokio::task::yield_now().await;
            }
            finished.store(true, Ordering::SeqCst);
            Ok(false)
        })
    }
}

/// D8: the startup tick is SCHEDULED, never awaited before the stdio loop —
/// an aged store must not delay the plugin's timeout-bounded `hello`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_startup_tick_is_scheduled_not_awaited() {
    use mem_bridge::engine::{Engine, EngineConfig, spawn_consolidation_lifecycle};

    let dir = TestDir::new("w53-sched-lifecycle");
    let store = store(&dir).await;
    insert_session(&store, "/p/a", "s-a", "agent-1").await;

    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let runner = Arc::new(SlowRunner {
        started: started.clone(),
        finished: finished.clone(),
    });
    let shutting_down = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicBool::new(false));
    let scheduler = scheduler(
        store,
        runner,
        shutting_down.clone(),
        active.clone(),
        Duration::from_secs(3600),
    );

    let engine = Engine::new(EngineConfig::default());
    spawn_consolidation_lifecycle(&engine, scheduler);

    // Deterministic on the current_thread test runtime: nothing has been
    // polled yet, so an inline `run_tick().await` would already have run.
    assert!(
        !started.load(Ordering::SeqCst),
        "the startup tick was NOT awaited inline"
    );
    assert_eq!(
        engine.live_tasks().load(Ordering::SeqCst),
        1,
        "one tracked lifecycle task"
    );

    mem_store::tokio::task::yield_now().await;
    assert!(
        started.load(Ordering::SeqCst),
        "the startup tick runs inside the spawned task"
    );
    assert!(
        active.load(Ordering::SeqCst),
        "consolidation_active is set while a run is in flight"
    );

    for _ in 0..10_000 {
        if finished.load(Ordering::SeqCst) {
            break;
        }
        mem_store::tokio::task::yield_now().await;
    }
    assert!(finished.load(Ordering::SeqCst), "the startup run completed");
    assert!(
        !active.load(Ordering::SeqCst),
        "the RAII guard cleared the flag"
    );
    assert_eq!(
        engine.live_tasks().load(Ordering::SeqCst),
        1,
        "the periodic loop shares the SAME tracked task (not a second spawn)"
    );

    shutting_down.store(true, Ordering::SeqCst);
    let mut remaining = 0;
    for _ in 0..600 {
        remaining = engine.live_tasks().load(Ordering::SeqCst);
        if remaining == 0 {
            break;
        }
        mem_store::tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(remaining, 0, "the lifecycle task ends on shutdown");
}

// ---------------------------------------------------------------------------
// R5 tag agreement: the promoted fact and the injector read the same tag
// ---------------------------------------------------------------------------

/// A sink that records the promoted facts instead of writing shards.
struct RecordingSink {
    records: std::sync::Mutex<Vec<mem_episodic::promotion::PromotionRecord>>,
}

impl mem_episodic::promotion::PermanentMemorySink for RecordingSink {
    fn promote<'a>(
        &'a self,
        _scope_hash: &'a str,
        record: &'a mem_episodic::promotion::PromotionRecord,
    ) -> mem_embed::embedder::BoxFuture<
        'a,
        Result<mem_episodic::promotion::PromoteOutcome, EpisodicError>,
    > {
        self.records.lock().expect("records").push(record.clone());
        Box::pin(async { Ok(mem_episodic::promotion::PromoteOutcome::Inserted) })
    }
}

struct FixedMilestoneSummarizer;

impl mem_episodic::summarizer::MilestoneSummarizer for FixedMilestoneSummarizer {
    fn summarize<'a>(
        &'a self,
        _transcript: &'a str,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<String, EpisodicError>> {
        Box::pin(async { Ok("merged milestone summary".to_string()) })
    }
}

/// `PromotionRecord.container_tag` is byte-identical to the tag the injector
/// derives for the same project (one scope family, never a second derivation).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_consolidation_promotion_record_tag_equals_inject_read_tag() {
    let dir = TestDir::new("w53-sched-tag");
    let clock = FixedClock::new(NOW - 3 * DAY_MILLIS);
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock.clock()),
    );
    store.ready().await.expect("store ready");
    insert_session(&store, "/p/tag", "s-tag", "agent-1").await;
    clock.set(NOW);

    let embedder: Arc<dyn mem_embed::embedder::Embedder> =
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(1)));
    let sink = Arc::new(RecordingSink {
        records: std::sync::Mutex::new(Vec::new()),
    });
    let deps = mem_episodic::consolidation::ConsolidationDeps {
        summarizer: Arc::new(FixedMilestoneSummarizer),
        embedder: embedder.clone(),
        sink: sink.clone(),
        batch_writer: Arc::new(mem_episodic::consolidation::StoreBatchWriter::new(
            store.clone(),
        )),
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: mem_episodic::consolidation::DEFAULT_BATCH_LIMIT,
        probe: Arc::new(mem_store::platform::SystemProcessProbe),
        cross_tier: Arc::new(mem_episodic::cross_tier::NoopCrossTierTask),
    };
    let policy = mem_episodic::consolidation::ConsolidationPolicy::new(1).expect("policy");
    let consolidator = mem_episodic::consolidation::Consolidator::new(store.clone(), policy, deps);
    let runner: Arc<dyn ConsolidationRunner> = Arc::new(
        mem_bridge::engine::DefaultConsolidationRunner::new(consolidator),
    );

    let scheduler = scheduler(
        store.clone(),
        runner,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(3600),
    );
    let report = scheduler.run_tick().await.expect("consolidation tick");
    assert_eq!(report.runs, 1, "the aged project consolidated");

    let records = sink.records.lock().expect("records").clone();
    assert!(!records.is_empty(), "at least one fact was promoted");
    let scope = <FixedScopeResolver as mem_bridge::scope_resolver::ProjectScopeResolver>::resolve(
        &FixedScopeResolver::default(),
        "/p/tag",
    )
    .expect("the injector's scope");
    for record in &records {
        assert_eq!(
            scope.container_tag.as_str(),
            record.container_tag.as_str(),
            "the promoted fact and the injector read the SAME container tag"
        );
    }
    let _ = Value::Null;
}
