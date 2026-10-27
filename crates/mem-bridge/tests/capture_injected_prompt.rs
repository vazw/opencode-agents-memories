//! Issue 52 — the injected-prompt filter on the capture (store) path.
//!
//! One vertical slice per step: each test drives the real `CaptureService`
//! over a real `EpisodicStore` and a real tier-1 writer, with a recording
//! summarizer so "did the provider see anything?" is observable.
//!
//! Fixture note per test (r4 item 6 / §9.2): tests 7, 8, 10, 11, 13 reproduce
//! the wrapper's payload shape (`transcript == metadata.autoCaptureContext.
//! userPrompt`, one `textResponses` entry). Test 7d is the exception — the
//! same transcript text with no `autoCaptureContext`, so the transcript is
//! the only prompt candidate (a legal host/V1 payload today's wrapper does
//! not send; a wrong-implementation discriminator, not a parity claim). Test
//! 9 is a defence-in-depth fixture the current wrapper cannot produce. Test
//! 7c is not producible by today's wrapper (`plugin.js:726` returns before the
//! payload when both response arrays are empty); it pins the engine's
//! blank-eligibility rule. Tests 12 and 12b are forward/host-payload fixtures
//! (no `raw_context` / a supplied `summary` in `fireIdleCapture`).

mod common;

use std::sync::{Arc, Mutex};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir};
use mem_ai::privacy::PrivacyConfig;
use mem_bridge::capture::{
    CaptureParams, CaptureService, CaptureSummaryOutput, SkipReason, SoundMemorySettings,
};
use mem_bridge::capture_prompt::CaptureSummarizer;
use mem_bridge::vault_feeder::VaultFeeder;
use mem_embed::embedder::BoxFuture;
use mem_episodic::error::EpisodicError;
use mem_episodic::store::EpisodicStore;
use mem_store::serde_json::json;

const BLOCK: &str = "<system-reminder>\n[BACKGROUND TASK RESULT READY]\n</system-reminder>";
const MIXED: &str = "refactor the auth middleware\n<system-reminder>\n[BACKGROUND TASK RESULT READY]\n</system-reminder>";
const PROJECT: &str = "/srv/proj";
const SESSION: &str = "sess-52";
const AGENT: &str = "agent-1";

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

/// The summarizer fake: records every `context` it is handed.
#[derive(Default)]
struct RecordingSummarizer {
    contexts: Mutex<Vec<String>>,
}

impl RecordingSummarizer {
    fn contexts(&self) -> Vec<String> {
        self.contexts.lock().expect("contexts lock").clone()
    }
}

impl CaptureSummarizer for RecordingSummarizer {
    fn summarize<'a>(
        &'a self,
        context: &'a str,
    ) -> BoxFuture<'a, Result<CaptureSummaryOutput, EpisodicError>> {
        self.contexts
            .lock()
            .expect("contexts lock")
            .push(context.to_string());
        Box::pin(async {
            Ok(CaptureSummaryOutput::Capture {
                summary: "the recorded summary".to_string(),
                memory_type: "memory".to_string(),
                tags: Vec::new(),
                extension: Default::default(),
            })
        })
    }
}

struct Fixture {
    service: CaptureService,
    store: Arc<EpisodicStore>,
    summarizer: Arc<RecordingSummarizer>,
    feeder: Arc<VaultFeeder>,
}

async fn fixture(dir: &TestDir, privacy: PrivacyConfig, sound: SoundMemorySettings) -> Fixture {
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    store.ready().await.expect("store ready");
    let summarizer = Arc::new(RecordingSummarizer::default());
    let feeder = Arc::new(VaultFeeder::new(Arc::new(mem_vault::VaultStore::new(
        dir.store_config(),
    ))));
    let service = CaptureService::new(
        store.clone(),
        mem_episodic::inbox::CaptureInbox::new(store.clone()),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(common::unit_vector(0))),
        Some(summarizer.clone() as Arc<dyn CaptureSummarizer>),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    )
    .with_sound_memory(sound)
    .with_vault_feeder(Some(feeder.clone()))
    .with_privacy(privacy);
    Fixture {
        service,
        store,
        summarizer,
        feeder,
    }
}

fn params(capture_id: &str) -> CaptureParams {
    CaptureParams {
        session_id: SESSION.to_string(),
        capture_id: capture_id.to_string(),
        project_id: PROJECT.to_string(),
        agent_id: AGENT.to_string(),
        agent_name: Some("build".to_string()),
        parent_session_id: None,
        transcript: None,
        summary: None,
        metadata: None,
        raw_context: None,
        agent_role: None,
        vault: false,
    }
}

// ---------------------------------------------------------------------------
// Step 7 — the typed skip (test 7b)
// ---------------------------------------------------------------------------

#[test]
fn test_skip_reasons_are_distinct_and_labelled() {
    // A single shared string for two reasons fails the `assert_ne!`; a unit
    // `Skip` does not compile.
    assert_eq!(SkipReason::InjectedOnly.as_str(), "injected_only");
    assert_eq!(SkipReason::NoContext.as_str(), "no_context");
    assert_eq!(SkipReason::ProviderSkipped.as_str(), "provider_skip");
    assert_ne!(
        CaptureSummaryOutput::Skip(SkipReason::InjectedOnly),
        CaptureSummaryOutput::Skip(SkipReason::NoContext)
    );
}

// ---------------------------------------------------------------------------
// Step 7 — the injected-only refusal (test 7, RED until Step 7b)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_injected_only_prompt_skips_both_tiers_and_never_calls_the_summarizer() {
    let dir = TestDir::new("i52-injected-only");
    let fixture = fixture(&dir, PrivacyConfig::default(), ON_REQUIRED).await;
    let mut capture = params("i52-cap-7");
    capture.transcript = Some(BLOCK.to_string());
    capture.metadata = Some(json!({
        "promptId": "p-1",
        "autoCaptureContext": { "userPrompt": BLOCK, "textResponses": ["done"] }
    }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    assert_eq!(
        report.dead_lettered, 0,
        "the refusal must precede retention"
    );
    assert_eq!(report.retryable, 0);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(
        contexts.len(),
        0,
        "summarizer contexts = {}",
        contexts.len()
    );
    assert!(
        fixture
            .store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .is_empty(),
        "no staging row may be written"
    );
    assert!(
        fixture
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw context")
            .is_none()
    );
    assert_eq!(fixture.feeder.enqueue_count(), 0);
}

// ---------------------------------------------------------------------------
// Step 7b — the blank-prompt eligibility (test 7c, RED until Step 7b)
// ---------------------------------------------------------------------------

async fn assert_blank_prompt_skips(label: &str, transcript: Option<&str>) {
    let dir = TestDir::new(label);
    let fixture = fixture(&dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params(&format!("i52-cap-7c-{label}"));
    capture.transcript = transcript.map(str::to_string);
    capture.metadata = Some(json!({
        "promptId": "p-1",
        "autoCaptureContext": { "userPrompt": "   ", "textResponses": [], "toolCalls": [] }
    }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(
        contexts.len(),
        0,
        "summarizer contexts = {}",
        contexts.len()
    );
    assert!(
        fixture
            .store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .is_empty(),
        "a blank prompt must not become a row ({label})"
    );
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_blank_prompt_with_no_responses_or_tools_skips_with_no_context() {
    // (a) a blank prompt and a blank transcript; (b) the same blank prompt
    // with a real transcript — no transcript fallback is introduced, so both
    // legs are the same branch.
    assert_blank_prompt_skips("i52-blank-a", Some("   ")).await;
    assert_blank_prompt_skips("i52-blank-b", Some("a real authored transcript")).await;
}

// ---------------------------------------------------------------------------
// Step 7b — the refusal reads the RAW transcript (test 7d, RED until 7b)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_refusal_reads_the_raw_transcript_not_the_filtered_binding() {
    let dir = TestDir::new("i52-raw-transcript");
    let fixture = fixture(&dir, PrivacyConfig::default(), ON_REQUIRED).await;
    let mut capture = params("i52-cap-7d");
    // Test 2b's leading/trailing-blank block: the filtered residue is `"\n"`,
    // so a refusal fed the filtered binding answers `false` and the entry is
    // dead-lettered by the retention rule.
    capture.transcript = Some(format!("\n{BLOCK}\n"));
    // The `capture_raw_context.rs:60` shape: `promptId` only, no
    // `autoCaptureContext` — the transcript is the only prompt candidate.
    capture.metadata = Some(json!({ "promptId": "p-1" }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    assert_eq!(
        report.dead_lettered, 0,
        "a filtered binding would dead-letter"
    );
    assert_eq!(report.retryable, 0);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(
        contexts.len(),
        0,
        "summarizer contexts = {}",
        contexts.len()
    );
    assert!(
        fixture
            .store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// Step 8 — the mixed prompt (site 1: the transcript filter)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_mixed_prompt_keeps_the_authored_text_and_drops_the_injected_block() {
    // Site 1's oracle. The payload is metadata-free on purpose: with a
    // metadata `userPrompt` present it wins inside `auto_capture_context`, so
    // site 5 would mask whether site 1 ran at all — the same masking shape
    // that keeps site 3 load-bearing on the flush path. This is a legal
    // host/V1 payload (`capture_raw_context.rs:60`), not a wrapper-parity
    // fixture.
    let dir = TestDir::new("i52-mixed");
    let fixture = fixture(&dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params("i52-cap-8");
    capture.transcript = Some(MIXED.to_string());
    capture.metadata = Some(json!({ "promptId": "p-1" }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(contexts[0].contains("refactor the auth middleware"));
    assert!(
        !contexts[0].contains("BACKGROUND TASK RESULT READY"),
        "the injected block must not reach the summarizer: {contexts:?}"
    );
    assert_eq!(
        fixture
            .store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .len(),
        1,
        "the mixed capture still writes its staging row"
    );
}

// ---------------------------------------------------------------------------
// Step 9 — the metadata prompt is filtered too (site 5, the trap)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_metadata_prompt_is_filtered_not_only_the_transcript() {
    // A filter wired only at site 1 (the transcript) leaves this RED: the
    // metadata `userPrompt` is what `auto_capture_context` prefers, so the
    // `boilerplate` line reaches the summarizer.
    let dir = TestDir::new("i52-metadata-prompt");
    let fixture = fixture(&dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params("i52-cap-9");
    capture.transcript = Some("authored text".to_string());
    capture.metadata = Some(json!({
        "promptId": "p-1",
        "autoCaptureContext": {
            "userPrompt": "authored text\n<system-reminder>\nboilerplate\n</system-reminder>",
            "textResponses": ["done"]
        }
    }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(contexts[0].contains("authored text"));
    assert!(
        !contexts[0].contains("boilerplate"),
        "the metadata prompt must be filtered, not only the transcript: {contexts:?}"
    );
}

// ---------------------------------------------------------------------------
// Step 10 — the flag flip (test 10)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_filter_injected_prompts_off_keeps_the_boilerplate_in_the_context() {
    // Wrong implementation this kills: a filter that ignores the flag (an
    // "always filter" seam) -> the context loses `BACKGROUND TASK RESULT
    // READY`. The correct bytes carry the block verbatim.
    let dir = TestDir::new("i52-flag-off");
    let privacy = PrivacyConfig {
        filter_injected_prompts: false,
        ..PrivacyConfig::default()
    };
    let fixture = fixture(&dir, privacy, OFF).await;
    let mut capture = params("i52-cap-10");
    capture.transcript = Some(MIXED.to_string());
    capture.metadata = Some(json!({ "promptId": "p-1" }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(
        contexts[0].contains("BACKGROUND TASK RESULT READY"),
        "filter off must keep the boilerplate: {contexts:?}"
    );
    assert_eq!(
        fixture
            .store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Step 11 — a configured marker is honoured (test 11)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_a_configured_injection_marker_is_honoured_by_the_capture_seam() {
    // Wrong implementation this kills: a reader that uses
    // `DEFAULT_INJECTION_MARKERS` only instead of the resolved blocklist ->
    // `MYBANNERPAYLOAD` survives into the context.
    let dir = TestDir::new("i52-custom-marker");
    let privacy = PrivacyConfig {
        blocklist: {
            let mut markers = PrivacyConfig::default().blocklist;
            markers.push("<my-banner>".to_string());
            markers
        },
        ..PrivacyConfig::default()
    };
    let fixture = fixture(&dir, privacy, OFF).await;
    let mut capture = params("i52-cap-11");
    capture.transcript = Some("keep me\n<my-banner>MYBANNERPAYLOAD</my-banner>".to_string());
    capture.metadata = Some(json!({ "promptId": "p-1" }));

    fixture.service.enqueue(&capture).await.expect("enqueue");
    let report = fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    let contexts = fixture.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(contexts[0].contains("keep me"));
    assert!(
        !contexts[0].contains("MYBANNERPAYLOAD"),
        "the configured marker must be honoured: {contexts:?}"
    );
}

// ---------------------------------------------------------------------------
// Step 12 — the supplied raw context, both legs (site 4)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_raw_context_is_filtered_before_retention() {
    // Leg (a) — a mixed raw context keeps `keep`, loses `boilerplate`.
    // Leg (b) — an entirely injected raw context does not exist: `None`, no
    // vault job, `Done`. The wrong placement priced here is the filter applied
    // at the `Ok { raw_context: Some(..) }` construction (after both blank
    // guards): a blank value reaches the store, `validate_raw_context`
    // rejects it, and the entry becomes `Retryable` (`retryable == 1`,
    // `done == 0`).
    //
    // `raw_context_required: false` on purpose (r5 item 5): with `true`, a
    // CORRECT filter leaves no candidate for leg (b) and
    // `RAW_CONTEXT_REQUIRED_MESSAGE` dead-letters the entry, so `done == 1`
    // would be unreachable and the test would fail the implementation it
    // protects. The retention rules must not convert a proven skip into a
    // dead letter.
    let dir = TestDir::new("i52-raw-mixed");
    let mixed_fixture = fixture(&dir, PrivacyConfig::default(), ON).await;
    let mut mixed = params("i52-cap-12a");
    mixed.summary = Some("the captured summary".to_string());
    mixed.raw_context =
        Some("keep\n<system-reminder>\nboilerplate\n</system-reminder>".to_string());
    mixed_fixture
        .service
        .enqueue(&mixed)
        .await
        .expect("enqueue");
    let report = mixed_fixture.service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1);
    let retained = mixed_fixture
        .store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("raw context")
        .expect("the mixed raw context is retained");
    assert!(retained.raw_context.contains("keep"));
    assert!(
        !retained.raw_context.contains("boilerplate"),
        "the raw context must be filtered before retention: {:?}",
        retained.raw_context
    );

    let blocked_dir = TestDir::new("i52-raw-blocked");
    let blocked_fixture = fixture(&blocked_dir, PrivacyConfig::default(), ON).await;
    let mut injected = params("i52-cap-12b");
    injected.summary = Some("the captured summary".to_string());
    injected.raw_context = Some(BLOCK.to_string());
    blocked_fixture
        .service
        .enqueue(&injected)
        .await
        .expect("enqueue");
    let report = blocked_fixture.service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1, "a proven skip is Done, never Retryable");
    assert_eq!(report.retryable, 0);
    assert!(
        blocked_fixture
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw context")
            .is_none(),
        "an entirely injected raw context must not be retained"
    );
    assert_eq!(blocked_fixture.feeder.enqueue_count(), 0);
}

// ---------------------------------------------------------------------------
// Step 13 — the flush path's transcript filter (test 12b, site 3)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_an_injected_only_transcript_is_not_retained_as_the_raw_context() {
    // Entry point: `flush_supplied_summary_queued()` — NOT `drain_once`. Site
    // 2's refusal is a `process_entry` rule (only that path builds a context
    // for a provider), so under `drain_once` a correct implementation writes
    // ZERO staging rows and "one staging row" would be unachievable. The
    // flush path takes the caller's summary, never calls the summarizer, and
    // its transcript is protected by site 3 alone (its payload has no
    // `raw_context`, so site 4 never touches that value).
    //
    // Wrong implementation priced: site 3 absent -> the block is retained as
    // the raw context (`Some(<block>)`) and enqueued to the feeder. The
    // merge-level placement (§7 (a)) also keeps this green with site 3
    // deleted; Step 13's mutation is the discriminator, so this test alone
    // does not claim it.
    let dir = TestDir::new("i52-flush-injected");
    let flush_fixture = fixture(&dir, PrivacyConfig::default(), ON).await;
    let mut capture = params("i52-cap-12b");
    capture.summary = Some("the captured summary".to_string());
    capture.transcript = Some(BLOCK.to_string());

    flush_fixture
        .service
        .enqueue(&capture)
        .await
        .expect("enqueue");
    let report = flush_fixture
        .service
        .flush_supplied_summary_queued()
        .await
        .expect("flush");

    assert_eq!(report.done, 1);
    assert_eq!(report.retryable, 0);
    assert_eq!(report.dead_lettered, 0);
    assert_eq!(
        flush_fixture
            .store
            .list_sessions(PROJECT, 10)
            .await
            .expect("list")
            .len(),
        1,
        "the staged entry still commits its row"
    );
    assert!(
        flush_fixture
            .store
            .raw_context_for_session(PROJECT, SESSION, AGENT)
            .await
            .expect("raw context")
            .is_none(),
        "the injected-only transcript must not be retained as the raw context"
    );
    assert_eq!(flush_fixture.feeder.enqueue_count(), 0);
}

// ---------------------------------------------------------------------------
// Step 13b — both mechanisms compose on one transcript (test 13)
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_private_stripping_and_the_injection_filter_both_fire_on_one_transcript() {
    // Wrong implementations this kills: the filter replacing the stripper ->
    // `secret token` survives; the stripper replacing the filter ->
    // `boilerplate` survives. It does NOT discriminate site 1's order: on this
    // fixture strip-then-filter and filter-then-strip produce the same bytes,
    // because the `<private>` region here does not meet a marker region (the
    // overlap shape test 13c prices is where the two orders diverge). The order
    // is fixed by construction (the existing `map(strip_private_content)`
    // binding, then the filter) — do not claim this test as an order oracle.
    let dir = TestDir::new("i52-both-mechanisms");
    let both_fixture = fixture(&dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params("i52-cap-13");
    capture.transcript = Some(
        "keep <private>secret token</private>\n<system-reminder>\nboilerplate\n</system-reminder>"
            .to_string(),
    );
    capture.metadata = Some(json!({ "promptId": "p-1" }));

    both_fixture
        .service
        .enqueue(&capture)
        .await
        .expect("enqueue");
    let report = both_fixture.service.drain_once().await.expect("drain");

    assert_eq!(report.done, 1);
    let contexts = both_fixture.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(contexts[0].contains("[REDACTED]"), "{contexts:?}");
    assert!(!contexts[0].contains("secret token"), "{contexts:?}");
    assert!(!contexts[0].contains("boilerplate"), "{contexts:?}");
}

// ---------------------------------------------------------------------------
// Step 13c — every capture filter runs AFTER the private strip (review R2)
// ---------------------------------------------------------------------------

/// The shared overlap shape: a marker region that MEETS a `<private>` opener.
/// Filtering before stripping deletes the opener with the marker region, and
/// the stripper then sees only a stray `</private>` and emits the secret.
const OVERLAP: &str = "keep\n<system-reminder>\n<private>\n</system-reminder>\nsecret\n</private>";

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_every_capture_filter_runs_after_the_private_strip() {
    // Five legs, one per filter site that can see host text. Each names the
    // wrong implementation it prices; the pre-fix worktree fails (b), (c),
    // (d) and (e) with `secret`; (a) is the green-on-arrival oracle that
    // fails when the assembled filter is deleted.
    //
    // (a) site 5, assembled — the filter's OWN oracle, unmasked by (b): the
    //     marker lives in `toolCalls.input` and the prompt is clean. With the
    //     assembled filter deleted, `boilerplate` reaches the summarizer.
    let a_dir = TestDir::new("i52-order-a");
    let a = fixture(&a_dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params("i52-cap-13c-a");
    capture.transcript = Some("authored text".to_string());
    capture.metadata = Some(json!({
        "promptId": "p-1",
        "autoCaptureContext": {
            "userPrompt": "authored text",
            "textResponses": [],
            "toolCalls": [
                { "name": "read", "input": "<system-reminder>\nboilerplate\n</system-reminder>" }
            ]
        }
    }));
    a.service.enqueue(&capture).await.expect("enqueue");
    assert_eq!(a.service.drain_once().await.expect("drain").done, 1);
    let contexts = a.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(
        contexts[0].contains("authored text"),
        "the authored prompt must survive: {contexts:?}"
    );
    assert!(
        !contexts[0].contains("<system-reminder>") && !contexts[0].contains("boilerplate"),
        "(a) the assembled context must be filtered, not only the prompt: {contexts:?}"
    );

    // (b) site 5, assembled — the Reviewer's leak. Pre-fix the summarizer
    //     received `... ok\n\nsecret\n\n---`.
    let b_dir = TestDir::new("i52-order-b");
    let b = fixture(&b_dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params("i52-cap-13c-b");
    capture.transcript = Some("authored text".to_string());
    capture.metadata = Some(json!({
        "promptId": "p-1",
        "autoCaptureContext": {
            "userPrompt": "authored text",
            "textResponses": ["ok", OVERLAP]
        }
    }));
    b.service.enqueue(&capture).await.expect("enqueue");
    assert_eq!(b.service.drain_once().await.expect("drain").done, 1);
    let contexts = b.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(
        !contexts[0].contains("secret") && !contexts[0].contains("<system-reminder>"),
        "(b) the assembled context must be stripped before it is filtered: {contexts:?}"
    );

    // (c) site 5, prompt — the prompt filter runs before the builder, so it
    //     must strip first: moving only the ASSEMBLED filter after the strip
    //     still leaks here. A filter-before-strip prompt filter emits `secret`.
    let c_dir = TestDir::new("i52-order-c");
    let c = fixture(&c_dir, PrivacyConfig::default(), OFF).await;
    let mut capture = params("i52-cap-13c-c");
    capture.transcript = Some("authored text".to_string());
    capture.metadata = Some(json!({
        "promptId": "p-1",
        "autoCaptureContext": { "userPrompt": OVERLAP, "textResponses": ["done"] }
    }));
    c.service.enqueue(&capture).await.expect("enqueue");
    assert_eq!(c.service.drain_once().await.expect("drain").done, 1);
    let contexts = c.summarizer.contexts();
    assert_eq!(contexts.len(), 1, "one summarizer call");
    assert!(
        !contexts[0].contains("secret") && !contexts[0].contains("<system-reminder>"),
        "(c) the prompt must be stripped before it is filtered: {contexts:?}"
    );

    // (d) site 4 — the supplied raw context. Pre-fix the store retained
    //     `keep\nsecret\n`; deleting site 4's filter stores the marker line.
    let d_dir = TestDir::new("i52-order-d");
    let d = fixture(&d_dir, PrivacyConfig::default(), ON).await;
    let mut capture = params("i52-cap-13c-d");
    capture.summary = Some("the captured summary".to_string());
    capture.raw_context = Some(OVERLAP.to_string());
    d.service.enqueue(&capture).await.expect("enqueue");
    let report = d.service.drain_once().await.expect("drain");
    assert_eq!(report.done, 1);
    let retained = d
        .store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("raw context")
        .expect("the mixed raw context is retained");
    assert!(
        !retained.raw_context.contains("secret")
            && !retained.raw_context.contains("<system-reminder>"),
        "(d) the raw context must be stripped before it is filtered: {retained:?}"
    );

    // (e) site 3 — the flush path's transcript, no `raw_context`. Pre-fix the
    //     store retained `keep\nsecret\n`; deleting site 3's filter stores
    //     the marker line.
    let e_dir = TestDir::new("i52-order-e");
    let e = fixture(&e_dir, PrivacyConfig::default(), ON).await;
    let mut capture = params("i52-cap-13c-e");
    capture.summary = Some("the captured summary".to_string());
    capture.transcript = Some(OVERLAP.to_string());
    e.service.enqueue(&capture).await.expect("enqueue");
    let report = e
        .service
        .flush_supplied_summary_queued()
        .await
        .expect("flush");
    assert_eq!(report.done, 1);
    let retained = e
        .store
        .raw_context_for_session(PROJECT, SESSION, AGENT)
        .await
        .expect("raw context")
        .expect("the mixed transcript is retained");
    assert!(
        !retained.raw_context.contains("secret")
            && !retained.raw_context.contains("<system-reminder>"),
        "(e) the flush transcript must be stripped before it is filtered: {retained:?}"
    );
}
