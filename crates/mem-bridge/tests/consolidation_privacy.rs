//! Issue 52 — D-52-7's consequence pin: the privacy wrap is capture-side only.
//!
//! The consolidation summarizer keeps the UNWRAPPED provider. Wrapping it
//! would turn a legacy marker-bearing memory into a silent, permanent skip:
//! `PrivacyFilterProvider::complete` errors when every user message was
//! dropped, `ProviderSummarizer::summarize` maps that to an `Err`, and
//! `Consolidator` records `SkipReason::SummarizerError` and still returns
//! `Ok`. The pair below brackets the decision from both sides.
//!
//! Every fake is authored in this file (no cross-crate `mod common;`): the
//! mem-episodic fixture module is compiled into mem-episodic's own test
//! binaries only. The shapes are copied from
//! `crates/mem-bridge/tests/consolidation_scheduling.rs` (the same-crate
//! precedent) and the `AiProvider` impl is written in the **desugared** form,
//! so no `async-trait` dependency is added.

mod common;

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{FakeEmbedder, TestDir};
use mem_ai::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};
use mem_ai::provider::{AiProvider, BoxFuture, CompletionStream, ProviderError};
use mem_episodic::consolidation::{
    ConsolidationDeps, ConsolidationPolicy, ConsolidationReport, Consolidator, DEFAULT_BATCH_LIMIT,
    PromotionScope, SkippedAgent, StoreBatchWriter,
};
use mem_episodic::error::EpisodicError;
use mem_episodic::promotion::{PermanentMemorySink, PromoteOutcome, PromotionRecord};
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::{Value, json};

const NOW: i64 = 1_000_000_000_000;
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
const PROJECT: &str = "/p";
const AGENT: &str = "agent-1";
const MARKER_SUMMARY: &str =
    "did work\n<system-reminder>\n[BACKGROUND TASK RESULT READY]\n</system-reminder>";

/// A clock the test can move (unix millis) — the age threshold is the trigger,
/// and no wall clock may be involved.
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

/// Records the promoted facts instead of writing shards.
struct RecordingSink {
    records: Mutex<Vec<PromotionRecord>>,
}

impl PermanentMemorySink for RecordingSink {
    fn promote<'a>(
        &'a self,
        _scope_hash: &'a str,
        record: &'a PromotionRecord,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<PromoteOutcome, EpisodicError>> {
        self.records
            .lock()
            .expect("records lock")
            .push(record.clone());
        Box::pin(async { Ok(PromoteOutcome::Inserted) })
    }
}

/// Counts `complete` calls and records each request.
#[derive(Default)]
struct CountingProvider {
    calls: AtomicUsize,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl CountingProvider {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<CompletionRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl AiProvider for CountingProvider {
    fn name(&self) -> &str {
        "counting-provider"
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().expect("requests lock").push(request);
        Box::pin(mem_episodic::futures::stream::iter(vec![
            Ok::<Value, ProviderError>(json!({ "text": "merged milestone" })),
            Ok(json!({ "done": true })),
        ]))
    }

    fn embed<'life0, 'async_trait>(
        &'life0 self,
        _request: EmbedRequest,
    ) -> BoxFuture<'async_trait, Result<Vec<Vec<f32>>, ProviderError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn list_models<'life0, 'async_trait>(
        &'life0 self,
    ) -> BoxFuture<'async_trait, Result<Vec<ModelInfo>, ProviderError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn count_tokens<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _model: &'life1 str,
        _text: &'life2 str,
    ) -> BoxFuture<'async_trait, Result<u32, ProviderError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(0) })
    }
}

/// One run over the same marker-bearing legacy row; `wrap` selects which
/// handle the summarizer gets (the wrong handle is 16b's mutation).
async fn run_consolidation(
    label: &str,
    wrap: bool,
) -> (
    ConsolidationReport,
    Arc<CountingProvider>,
    Arc<RecordingSink>,
) {
    let dir = TestDir::new(label);
    let clock = FixedClock::new(NOW - 3 * DAY_MILLIS);
    let store = Arc::new(
        EpisodicStore::new(dir.path().to_path_buf(), dir.store_config()).with_clock(clock.clock()),
    );
    store.ready().await.expect("store ready");
    // The in-crate write: `EpisodicStore::insert_session` takes `&dyn Embedder`.
    // NEVER `insert_at` — that is mem-episodic's `ConsolidationFixture` method
    // and its `mod common` is not importable from this crate.
    let insert_embedder = Arc::new(FakeEmbedder::with_vector(common::unit_vector(0)));
    store
        .insert_session(
            NewAgentSession {
                project_id: PROJECT.to_string(),
                session_id: "s-1".to_string(),
                agent_id: AGENT.to_string(),
                summary: MARKER_SUMMARY.to_string(),
                ..Default::default()
            },
            insert_embedder.as_ref(),
        )
        .await
        .expect("insert session");
    clock.set(NOW);

    let inner = Arc::new(CountingProvider::default());
    let summarizer: Arc<dyn mem_episodic::summarizer::MilestoneSummarizer> = if wrap {
        // The mistake 16b prices: exactly one `Arc::new`, because `wrap`
        // already returns `Arc<PrivacyFilterProvider>`.
        Arc::new(mem_episodic::summarizer::ProviderSummarizer::new(
            mem_ai::privacy::PrivacyFilterLayer::default().wrap(inner.clone()),
            "test-model",
        ))
    } else {
        // The composition's actual configuration (D-52-7).
        Arc::new(mem_episodic::summarizer::ProviderSummarizer::new(
            inner.clone(),
            "test-model",
        ))
    };
    let sink = Arc::new(RecordingSink {
        records: Mutex::new(Vec::new()),
    });
    let deps = ConsolidationDeps {
        summarizer,
        embedder: Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        sink: sink.clone(),
        // The REAL batch writer: a fake would leave `sessions_consolidated`
        // true while nothing was written, making 16c's "the batch committed"
        // claim vacuous.
        batch_writer: Arc::new(StoreBatchWriter::new(store.clone())),
        prefix_options: mem_embed::prefix::PrefixOptions::default(),
        batch_limit: DEFAULT_BATCH_LIMIT,
        probe: Arc::new(mem_store::platform::SystemProcessProbe),
        cross_tier: Arc::new(mem_episodic::cross_tier::NoopCrossTierTask),
    };
    let consolidator = Consolidator::new(
        store.clone(),
        ConsolidationPolicy::new(1).expect("policy"),
        deps,
    );
    let scope = PromotionScope {
        container_tag: "opencode_project_0123456789abcdef".to_string(),
        scope_hash: "0123456789abcdef".to_string(),
    };
    let report = consolidator
        .run(PROJECT, &scope)
        .await
        .expect("consolidation");
    (report, inner, sink)
}

// ---------------------------------------------------------------------------
// Step 15 — test 16b: the wrapped handle silently skips the memory
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_privacy_wrapped_provider_silently_skips_a_marker_bearing_memory() {
    let (report, inner, sink) = run_consolidation("i52-consolidation-wrapped", true).await;

    assert_eq!(report.sessions_consolidated, 0);
    assert_eq!(report.milestones_written, 0);
    assert_eq!(
        report.agents_skipped,
        vec![SkippedAgent {
            agent_id: AGENT.to_string(),
            reason: mem_episodic::consolidation::SkipReason::SummarizerError,
        }],
        "the layer's all-dropped error becomes a SummarizerError skip"
    );
    assert_eq!(inner.calls(), 0, "the inner provider is never reached");
    assert!(sink.records.lock().expect("records lock").is_empty());
}

// ---------------------------------------------------------------------------
// Step 15 — test 16c: the unwrapped handle consolidates it
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_an_unwrapped_provider_consolidates_the_same_marker_bearing_memory() {
    let (report, inner, _sink) = run_consolidation("i52-consolidation-unwrapped", false).await;

    assert_eq!(report.sessions_consolidated, 1);
    assert_eq!(report.milestones_written, 1);
    assert_eq!(inner.calls(), 1, "the inner provider is reached once");
    let requests = inner.requests();
    assert!(
        requests[0].messages[0]
            .content
            .contains("<system-reminder>"),
        "the unwrapped summarizer sees the marker-bearing transcript"
    );
}
