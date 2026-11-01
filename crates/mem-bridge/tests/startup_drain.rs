//! Step 7's startup-replay ordering at the ENGINE seam (the drain-vs-handshake
//! defect's other half).
//!
//! `tests/engine_startup_drain.rs` pins the process-level contract: the drain
//! never gates `hello`, and shutdown stays inside the D-45 budget while a
//! drain is blocked. This file pins the ordering the old AWAITED drain provided
//! and Step 7 still needs — with the real `Engine` scheduling path, a real
//! episodic store and the shared fake seams:
//!
//! * (b) with a FAST drain, the FIRST `inject` waits (inside the named
//!   `INJECT_BARRIER_BUDGET`) and therefore observes the fact the drain wrote;
//! * (c) a later `inject` neither re-waits nor re-runs the completed drain;
//! * (e) a BLOCKED drain never holds the serial stdio loop past that budget:
//!   the first `inject` proceeds, warns `engine.inject_barrier_timeout` once,
//!   and the second `inject` does not re-wait; and
//! * (f) a DEAD drain (the task panicked) fails open — `inject` answers — and
//!   names itself `engine.startup_drain_failed` instead of dying silently.
//!
//! The ordering guarantee is therefore BOUNDED, not absolute: (b) is what a
//! fast replay buys, (e) is the deliberate trade — a possibly-stale first read
//! instead of a wedged engine (`dequeue` never deletes, so nothing is lost).
//!
//! The summarizer is a gate: its call is counted, then it holds the drain in
//! flight until the test releases it. That makes "the first inject is waiting"
//! an observation, not a timing guess.
//!
//! The named events are asserted from a CHILD of this test binary with its
//! stderr redirected to a file: `structured_stderr` writes straight to the
//! descriptor and libtest's per-test capture never sees it
//! (`docs/agents/testing.md`: stdout/stderr are redirected to files, never
//! pipes).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{
    FakeEmbedder, FixedScopeResolver, SystemProcessProbe, TestDir, shard_manager, unit_vector,
};
use mem_bridge::capture::{
    CaptureExtension, CaptureParams, CaptureService, CaptureSummaryOutput, CaptureWriterDeps,
    DefaultTier1Writer,
};
use mem_bridge::capture_prompt::CaptureSummarizer;
use mem_bridge::engine::{
    Engine, EngineConfig, EngineRuntime, INJECT_BARRIER_BUDGET, LaunchMode, ShutdownReport,
    serve_stdio,
};
use mem_bridge::inject::InjectService;
use mem_bridge::recall::RecallService;
use mem_embed::embedder::BoxFuture;
use mem_episodic::error::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::{Value, json};
use mem_store::tokio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

/// The fact ONLY the drain can write: it is the summarizer's output for the
/// queued capture, rendered by `inject` under `## Recent sessions`.
const DRAINED_SUMMARY: &str = "Drained the queued capture before the first inject read";

const PROJECT: &str = "/srv/startup-drain-project";
const SEEDED_CAPTURE: &str = "019a0000-0000-7f00-8000-00000000d5b1";
const SECOND_CAPTURE: &str = "019a0000-0000-7f00-8000-00000000d5b2";

const HELLO: &str = r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"startup-drain","min_protocol":"1.0.0","protocol":"1.0.0"}}"#;
const SHUTDOWN: &str = r#"{"id":"shutdown-1","jsonrpc":"2.0","method":"shutdown","params":{"session_id":"sess-drain"}}"#;

/// The wrapper's own deadline is 5 s; the handshake must spend none of it on
/// the drain, so this bound is an order of magnitude under it.
const HELLO_BOUND: Duration = Duration::from_secs(1);
const INJECT_BOUND: Duration = Duration::from_secs(5);
const IDLE_BOUND: Duration = Duration::from_millis(500);

/// The reviewed barrier budget, asserted equal to the production constant so a
/// silent widening cannot pass: 2 s is the fix, and a wider one re-opens R1.
const BARRIER_BUDGET: Duration = Duration::from_secs(2);

/// What a blocked barrier may cost before the inject answers: the budget plus
/// scheduling slack for the fixture's store reads.
const BLOCKED_INJECT_BOUND: Duration = Duration::from_secs(4);

/// A settled barrier never re-waits, so the second inject is an ordinary read.
const NO_REWAIT_BOUND: Duration = Duration::from_secs(1);

/// The wrapper's `REQUEST_TIMEOUT_MS` (`plugin/src/plugin.js:57`): the
/// consequence R1 measured. Every bound above stays far under it.
const WRAPPER_TIMEOUT: Duration = Duration::from_secs(15);

fn capture_params(capture_id: &str) -> CaptureParams {
    CaptureParams {
        session_id: "sess-drain".to_string(),
        capture_id: capture_id.to_string(),
        project_id: PROJECT.to_string(),
        agent_id: "agent-drain".to_string(),
        transcript: Some("drain the queued capture before inject reads".to_string()),
        ..CaptureParams::default()
    }
}

fn inject_request(id: &str) -> String {
    json!({
        "id": id,
        "jsonrpc": "2.0",
        "method": "inject",
        "params": { "session_id": "sess-drain", "project_id": PROJECT }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// The gated summarizer: the drain is in flight, provably
// ---------------------------------------------------------------------------

struct GatedSummarizer {
    calls: Arc<AtomicUsize>,
    release: watch::Sender<bool>,
}

impl CaptureSummarizer for GatedSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, EpisodicError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut released = self.release.subscribe();
        Box::pin(async move {
            // 0.5 s of provider latency would already be enough to hold the
            // handshake past the wrapper's deadline; this gate is unbounded
            // until the test releases it, so "still draining" is observed.
            let _ = released.wait_for(|open| *open).await;
            Ok(CaptureSummaryOutput::Capture {
                summary: DRAINED_SUMMARY.to_string(),
                memory_type: "memory".to_string(),
                tags: vec!["drain".to_string()],
                extension: CaptureExtension::default(),
            })
        })
    }
}

/// A summarizer that PANICS: `drain_once` unwinds, the spawned drain task dies,
/// the `watch` sender drops with it, and the first `inject`'s wait returns
/// `Err` — the dead-drain fail-open. Before the fix that branch was silent.
struct PanickingSummarizer;

impl CaptureSummarizer for PanickingSummarizer {
    fn summarize<'a>(
        &'a self,
        _context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, EpisodicError>> {
        Box::pin(async move {
            panic!("the fixture summarizer fails the startup drain (dead-drain fail-open case)");
        })
    }
}

// ---------------------------------------------------------------------------
// Fixture: real store + real capture drain + real inject, one gate
// ---------------------------------------------------------------------------

/// The engine and its runtime over a fresh temp root, with ONE capture queued
/// whose only route to a memory is the summarizer — so a drain is provably
/// what the first `inject` waits for. The engine is assembled FIRST, then the
/// drain is scheduled: the production order in `run_in_runtime`.
struct Parts {
    engine: Engine,
    capture: Arc<CaptureService>,
    dir: TestDir,
}

async fn parts(prefix: &str, summarizer: Option<Arc<dyn CaptureSummarizer>>) -> Parts {
    let dir = TestDir::new(prefix);
    let config = dir.store_config();
    let store = Arc::new(EpisodicStore::new(dir.path().to_path_buf(), config.clone()));
    store.ready().await.expect("fixture store ready");
    let shards = Arc::new(shard_manager(config));
    let embedder: Arc<dyn mem_embed::embedder::Embedder> =
        Arc::new(FakeEmbedder::with_vector(unit_vector(0)));
    let capture = Arc::new(CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(DefaultTier1Writer::new(CaptureWriterDeps {
            shards: shards.clone(),
            embedder: embedder.clone(),
        })),
        embedder.clone(),
        summarizer,
        Arc::new(FixedScopeResolver::default()),
        131_072,
    ));
    capture
        .enqueue(&capture_params(SEEDED_CAPTURE))
        .await
        .expect("queue the seeded capture");
    let recall = RecallService::new(store.clone(), embedder.clone());
    let inject = InjectService::new(
        store,
        shards,
        Arc::new(FixedScopeResolver::default()),
        Arc::new(AtomicBool::new(false)),
        embedder,
        Arc::new(SystemProcessProbe),
    );
    let runtime = EngineRuntime {
        capture: Arc::clone(&capture),
        inject: Arc::new(inject),
        recall: Arc::new(recall),
        skills: None,
        tools: None,
    };
    let mut engine = Engine::with_runtime(EngineConfig::default(), Arc::new(runtime));
    engine.schedule_startup_drain(Arc::clone(&capture));
    Parts {
        engine,
        capture,
        dir,
    }
}

/// The gated fixture: the drain can be held in flight and released on demand.
struct Fixture {
    capture: Arc<CaptureService>,
    calls: Arc<AtomicUsize>,
    release: watch::Sender<bool>,
    engine: Engine,
    _dir: TestDir,
}

async fn fixture(prefix: &str) -> Fixture {
    let calls = Arc::new(AtomicUsize::new(0));
    let (release, _) = watch::channel(false);
    let summarizer = Arc::new(GatedSummarizer {
        calls: Arc::clone(&calls),
        release: release.clone(),
    });
    let parts = parts(prefix, Some(summarizer)).await;
    Fixture {
        capture: parts.capture,
        calls,
        release,
        engine: parts.engine,
        _dir: parts.dir,
    }
}

/// The dead-drain fixture: the drain task panics instead of completing.
struct DeadFixture {
    engine: Engine,
    _dir: TestDir,
}

async fn dead_fixture(prefix: &str) -> DeadFixture {
    let parts = parts(prefix, Some(Arc::new(PanickingSummarizer))).await;
    DeadFixture {
        engine: parts.engine,
        _dir: parts.dir,
    }
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// One `serve_stdio` loop over an in-memory duplex pair, with the response
/// lines forwarded by a dedicated reader task so a bounded receive never
/// cancels a half-read line.
struct Wire {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: mpsc::UnboundedReceiver<String>,
    handle: tokio::task::JoinHandle<Result<ShutdownReport, String>>,
}

impl Wire {
    fn start(engine: Arc<Engine>) -> Self {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (client_read, writer) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server);
        let (sender, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(client_read);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let handle = tokio::spawn(async move {
            serve_stdio(&engine, server_read, server_write, LaunchMode::Stdio).await
        });
        Self {
            writer,
            lines,
            handle,
        }
    }

    async fn send(&mut self, line: &str) {
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("request write");
        self.writer
            .write_all(b"\n")
            .await
            .expect("NDJSON terminator");
        self.writer.flush().await.expect("request flush");
    }

    async fn recv_within(&mut self, bound: Duration) -> Option<Value> {
        let line = timeout(bound, self.lines.recv()).await.ok().flatten()?;
        Some(mem_store::serde_json::from_str(line.trim()).expect("a JSON-RPC response"))
    }

    /// Acks `shutdown` and joins the stdio loop (no parked task remains).
    async fn finish(mut self) {
        self.send(SHUTDOWN).await;
        assert!(
            self.recv_within(Duration::from_secs(2)).await.is_some(),
            "shutdown is acked"
        );
        let joined = timeout(Duration::from_secs(2), self.handle)
            .await
            .expect("the stdio loop ends after shutdown");
        joined
            .expect("the stdio task joins")
            .expect("serve_stdio ok");
    }
}

/// Polls `condition` for at most 2 s (the drain task runs as soon as the
/// engine's loop yields, so this is a scheduling wait, not a sleep budget).
async fn wait_until(condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the startup drain never reached the summarizer"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn block_of(response: &Value) -> Option<&str> {
    response
        .get("result")
        .and_then(|result| result.get("block"))
        .and_then(Value::as_str)
}

fn memory_count_of(response: &Value) -> Option<&Value> {
    response
        .get("result")
        .and_then(|result| result.get("memory_count"))
}

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

// ---------------------------------------------------------------------------
// The named-event wire: a child of this binary with stderr on a file
// ---------------------------------------------------------------------------

/// Runs one `#[ignore]`d case in a CHILD of this same test binary, its stderr
/// redirected to a FILE, and returns what the child wrote there.
///
/// `structured_stderr` writes to the raw descriptor, which libtest's per-test
/// capture never intercepts, so the house convention — stderr to files, never
/// pipes — is the only way to assert a named event from an in-process seam.
fn run_captured(inner: &str, dir: &TestDir) -> String {
    let log = dir.path().join("child.err");
    let stderr = std::fs::File::create(&log).expect("create the child stderr file");
    let status = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            inner,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr))
        .status()
        .expect("the child test binary runs");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.success(),
        "the child case `{inner}` must pass: {status}\n{text}"
    );
    text
}

fn assert_reviewed_budget() {
    assert_eq!(
        INJECT_BARRIER_BUDGET, BARRIER_BUDGET,
        "the barrier budget is the reviewed 2 s; a wider one re-opens the wrapper-timeout R1"
    );
    assert!(
        BLOCKED_INJECT_BOUND < WRAPPER_TIMEOUT,
        "the barrier plus its slack must stay well inside the wrapper's 15 s deadline"
    );
}

// ---------------------------------------------------------------------------
// (b) a FAST drain: the first inject waits (in budget) and sees its write
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_first_inject_waits_for_the_startup_drain_and_sees_its_write() {
    assert_reviewed_budget();
    let fixture = fixture("startup-drain-first-inject").await;
    let mut wire = Wire::start(Arc::new(fixture.engine));

    // The handshake answers while the drain is blocked: the drain never gates
    // it (the process-level row proves the longer timeouts; this one proves
    // the ordering underneath).
    wire.send(HELLO).await;
    let hello = wire
        .recv_within(HELLO_BOUND)
        .await
        .expect("`hello` must never wait for the startup drain");
    assert_eq!(
        hello
            .get("result")
            .and_then(|result| result.get("protocol"))
            .and_then(Value::as_str),
        Some("1.0.0"),
        "the handshake answered the protocol line"
    );

    wire.send(&inject_request("inject-1")).await;
    wait_until(|| fixture.calls.load(Ordering::SeqCst) == 1).await;

    // The drain is demonstrably still in flight (the summarizer is holding
    // it), so a reply now would mean `inject` read an un-drained store.
    let early = wire.recv_within(IDLE_BOUND).await;
    assert!(
        early.is_none(),
        "a FAST first inject must give the drain its budget; it answered while the drain was \
         still in flight: {early:?}"
    );

    fixture
        .release
        .send(true)
        .expect("release the drain's provider call");
    let inject = wire
        .recv_within(INJECT_BOUND)
        .await
        .expect("inject answers once the drain completed");
    let block = block_of(&inject).expect("an inject history block is rendered");
    assert!(
        block.contains(DRAINED_SUMMARY),
        "the first inject observed the fact the drain wrote: {block}"
    );
    assert!(
        block.contains("## Recent sessions"),
        "the drained episode renders under recent sessions: {block}"
    );
    assert_eq!(
        memory_count_of(&inject),
        Some(&json!(1)),
        "one included logical row from the drained capture"
    );

    wire.finish().await;
}

// ---------------------------------------------------------------------------
// (c) a later inject neither re-waits nor re-runs the completed drain
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_second_inject_does_not_re_wait_or_re_run_the_completed_drain() {
    let fixture = fixture("startup-drain-idempotent").await;
    let mut wire = Wire::start(Arc::new(fixture.engine));

    wire.send(HELLO).await;
    wire.recv_within(HELLO_BOUND)
        .await
        .expect("`hello` must never wait for the startup drain");

    wire.send(&inject_request("inject-1")).await;
    wait_until(|| fixture.calls.load(Ordering::SeqCst) == 1).await;
    let early = wire.recv_within(IDLE_BOUND).await;
    assert!(
        early.is_none(),
        "the FIRST inject must wait for the startup drain: {early:?}"
    );
    fixture
        .release
        .send(true)
        .expect("release the drain's provider call");
    let first = wire
        .recv_within(INJECT_BOUND)
        .await
        .expect("the first inject answers after the drain");
    assert!(
        block_of(&first)
            .expect("an inject history block is rendered")
            .contains(DRAINED_SUMMARY),
        "the first inject saw the drained fact: {:?}",
        block_of(&first)
    );

    // A capture queued AFTER the drain completed. A second inject must not
    // drain it: a re-run would consume it and call the summarizer again.
    fixture
        .capture
        .enqueue(&capture_params(SECOND_CAPTURE))
        .await
        .expect("queue the second capture");

    let started = Instant::now();
    wire.send(&inject_request("inject-2")).await;
    let second = wire
        .recv_within(HELLO_BOUND)
        .await
        .expect("a completed drain never re-waits an inject");
    assert!(
        started.elapsed() < HELLO_BOUND,
        "the second inject did not wait on the completed drain"
    );
    assert_eq!(
        fixture.calls.load(Ordering::SeqCst),
        1,
        "the second inject must not re-run the drain"
    );
    assert_eq!(
        memory_count_of(&second),
        Some(&json!(1)),
        "the second capture is still queued, not drained by an inject"
    );

    wire.finish().await;
}

// ---------------------------------------------------------------------------
// (e) a BLOCKED drain: bounded, warned once, and never re-waited
// ---------------------------------------------------------------------------

/// The child run (see [`run_captured`]): the barrier is bounded and one-shot.
#[test]
#[ignore = "run by test_blocked_startup_drain_is_bounded_and_names_the_timeout in a child process"]
fn inner_blocked_startup_drain_is_bounded() {
    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the child runtime");
    runtime.block_on(async {
        // The gate is NEVER released: the drain stays in flight for the whole
        // case, exactly the backlog R1 measured.
        let fixture = fixture("startup-drain-blocked").await;
        let mut wire = Wire::start(Arc::new(fixture.engine));

        wire.send(HELLO).await;
        wire.recv_within(HELLO_BOUND)
            .await
            .expect("`hello` must never wait for the startup drain");

        let started = Instant::now();
        wire.send(&inject_request("inject-1")).await;
        wait_until(|| fixture.calls.load(Ordering::SeqCst) == 1).await;
        let first = wire.recv_within(BLOCKED_INJECT_BOUND).await.unwrap_or_else(|| {
            panic!(
                "a blocked startup drain must never hold the serial stdio loop past the barrier \
                 budget ({BARRIER_BUDGET:?}); no inject reply within {BLOCKED_INJECT_BOUND:?} \
                 (the wrapper's deadline is {WRAPPER_TIMEOUT:?})"
            )
        });
        let elapsed = started.elapsed();
        assert!(
            elapsed >= BARRIER_BUDGET,
            "the barrier must actually give the drain its budget before proceeding; it \
             answered after {elapsed:?}"
        );
        assert!(
            elapsed < BLOCKED_INJECT_BOUND,
            "the barrier must be BOUNDED, not merely moved: {elapsed:?}"
        );
        assert!(
            elapsed < WRAPPER_TIMEOUT,
            "the first inject answers inside the wrapper's deadline: {elapsed:?}"
        );
        assert_eq!(
            memory_count_of(&first),
            Some(&json!(0)),
            "the trade the budget makes: the un-drained capture is not in this read"
        );
        assert!(
            block_of(&first).unwrap_or("").is_empty(),
            "the blocked drain wrote nothing, so the first read carries no session block: {:?}",
            block_of(&first)
        );

        // The second inject must not re-wait: the barrier settled on the first.
        let started = Instant::now();
        wire.send(&inject_request("inject-2")).await;
        let second = wire
            .recv_within(NO_REWAIT_BOUND)
            .await
            .expect("a settled barrier never re-waits a later inject");
        assert!(
            started.elapsed() < BARRIER_BUDGET,
            "the second inject did not re-wait the barrier: {:?}",
            started.elapsed()
        );
        assert_eq!(
            fixture.calls.load(Ordering::SeqCst),
            1,
            "the second inject must not re-run the drain"
        );
        assert_eq!(
            memory_count_of(&second),
            Some(&json!(0)),
            "the barrier's expiry is never repaired by a later inject"
        );

        wire.finish().await;
    });
}

#[test]
fn test_blocked_startup_drain_is_bounded_and_names_the_timeout() {
    assert_reviewed_budget();
    let dir = TestDir::new("startup-drain-blocked-log");
    let text = run_captured("inner_blocked_startup_drain_is_bounded", &dir);
    assert_eq!(
        occurrences(&text, "engine.inject_barrier_timeout"),
        1,
        "the expired barrier is named exactly once, never per request: {text}"
    );
    assert!(
        text.contains(&format!("\"budget_ms\":{}", BARRIER_BUDGET.as_millis())),
        "the warn carries the budget it spent: {text}"
    );
    assert!(
        !text.contains("engine.startup_drain_failed"),
        "a live-but-slow drain is not a dead one: {text}"
    );
}

// ---------------------------------------------------------------------------
// (f) a DEAD drain: fail-open, and named instead of silent
// ---------------------------------------------------------------------------

/// The child run (see [`run_captured`]): a panicked drain task never wedges.
#[test]
#[ignore = "run by test_dead_startup_drain_fails_open_and_names_the_failure in a child process"]
fn inner_dead_startup_drain_fails_open() {
    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the child runtime");
    runtime.block_on(async {
        let fixture = dead_fixture("startup-drain-dead").await;
        let mut wire = Wire::start(Arc::new(fixture.engine));

        wire.send(HELLO).await;
        wire.recv_within(HELLO_BOUND)
            .await
            .expect("`hello` must never wait for the startup drain");

        let started = Instant::now();
        wire.send(&inject_request("inject-1")).await;
        let inject = wire.recv_within(INJECT_BOUND).await.unwrap_or_else(|| {
            panic!(
                "a DEAD startup drain must fail open, never wedge: no inject reply within \
                 {INJECT_BOUND:?} (the wrapper's deadline is {WRAPPER_TIMEOUT:?})"
            )
        });
        assert!(
            started.elapsed() < INJECT_BOUND,
            "the fail-open costs no wait once the task is gone: {:?}",
            started.elapsed()
        );
        assert_eq!(
            memory_count_of(&inject),
            Some(&json!(0)),
            "a dead drain drained nothing; the durable rows stay for the next drain"
        );

        wire.finish().await;
    });
}

#[test]
fn test_dead_startup_drain_fails_open_and_names_the_failure() {
    let dir = TestDir::new("startup-drain-dead-log");
    let text = run_captured("inner_dead_startup_drain_fails_open", &dir);
    assert!(
        text.contains("engine.startup_drain_failed"),
        "the dead-drain fail-open must name itself; the review's F1 was this silence: {text}"
    );
    assert!(
        !text.contains("engine.inject_barrier_timeout"),
        "a dead drain resolves at once; it never spends the budget: {text}"
    );
}
