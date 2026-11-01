//! Capture asks for structured output (ADR-0029).
//!
//! The measured failure, as a behaviour: a provider that DROPS
//! `request.json_schema` on its streaming arm answers the prompt's markdown
//! `FORMAT:` block instead of JSON, the summarizer parses markdown, and the
//! attempt is dead-lettered with `reason: "summarizer_failed"`.
//!
//! The recording is on disk (`.tmp/scratch/probe-stream.out`): 80 deltas,
//! 308 bytes of markdown, `parses as JSON: NO`. The structured arm is the one
//! that answers (`probe-capture-format.out`: a non-empty `summary` in 25.5 s).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mem_ai::provider::{
    AiProvider, BoxFuture, CompletionRequest, CompletionStream, EmbedRequest, ModelInfo,
    ProviderError, ProviderErrorKind,
};
use mem_bridge::capture::CaptureSummaryOutput;
use mem_bridge::capture_prompt::{CaptureSummarizer, ProviderCaptureSummarizer};
use mem_bridge::capture_provider::StructuredCapture;
use mem_store::serde_json::{Value, json};

/// The model id the summarizer names; the double ignores it.
const MODEL: &str = "qwen3.8-flash-next-uncensored-nothink";

/// The 308 bytes the streaming arm really returned, verbatim
/// (`.tmp/scratch/probe-stream.out`).
const STREAMED_MARKDOWN: &str = "## Request\nBound the unbounded inject barrier to 2 seconds and add logging for timeout events.\n\n## Outcome\nImplemented `INJECT_BARRIER_BUDGET` and an `AtomicBool` latch in `engine.rs` to enforce the 2-second timeout and log when the barrier is exceeded.\n\n**Tags**: `rust`, `concurrency`, `timeout`, `logging`";

/// The measured wrong provider: it answers the STREAMING arm and discards
/// `request.json_schema`, exactly as 7 of the 8 concrete `complete` impls do.
///
/// The two counters make the wrong route visible: a correct summarizer never
/// touches `complete` at all.
#[derive(Default)]
struct SchemaDroppingProvider {
    stream_calls: AtomicUsize,
    structured_calls: AtomicUsize,
}

impl SchemaDroppingProvider {
    fn stream_calls(&self) -> usize {
        self.stream_calls.load(Ordering::SeqCst)
    }

    fn structured_calls(&self) -> usize {
        self.structured_calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for SchemaDroppingProvider {
    fn name(&self) -> &str {
        "schema-dropping"
    }

    fn complete(&self, _request: CompletionRequest) -> CompletionStream {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(mem_episodic::futures::stream::iter(vec![
            Ok(json!({ "text": STREAMED_MARKDOWN })),
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

/// The capability the summarizer must use. It increments `structured_calls`,
/// so the counter proves which arm answered.
impl StructuredCapture for SchemaDroppingProvider {
    fn complete_structured<'a>(
        &'a self,
        _request: CompletionRequest,
        _schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        self.structured_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(json!({
                "summary": "Bounded the inject barrier to 2 seconds and logged the timeout.",
                "type": "task",
                "tags": ["rust"]
            }))
        })
    }
}

/// THE RED: the summarizer must not consume the arm that drops the schema.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_summarizes_when_the_streaming_arm_drops_the_schema() {
    let provider = Arc::new(SchemaDroppingProvider::default());
    let summarizer = ProviderCaptureSummarizer::new(provider.clone(), MODEL, 131_072);
    let out = summarizer
        .summarize("user: bound the unbounded inject barrier")
        .await
        .expect("a provider that can answer structured output must not be dead-lettered");
    let CaptureSummaryOutput::Capture { summary, .. } = out else {
        panic!("expected a capture: {out:?}")
    };
    assert!(!summary.is_empty(), "the summary must carry text");
    assert_eq!(
        provider.stream_calls(),
        0,
        "capture must not use the streaming arm"
    );
    assert_eq!(
        provider.structured_calls(),
        1,
        "capture uses the structured arm once"
    );
}

/// A `StructuredCapture` double that records `(request, schema)` and answers
/// one scripted result — the seam every other row in this file observes.
struct ScriptedCapture {
    recorded: std::sync::Mutex<Vec<(CompletionRequest, Value)>>,
    answer: Answer,
}

/// What the double does, built fresh per call so no `ProviderError` is cloned.
enum Answer {
    Value(Value),
    Failure(ProviderErrorKind, &'static str, Option<u16>),
}

impl ScriptedCapture {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            recorded: std::sync::Mutex::new(Vec::new()),
            answer,
        })
    }

    fn recorded(&self) -> Vec<(CompletionRequest, Value)> {
        self.recorded.lock().expect("recorded lock").clone()
    }
}

impl StructuredCapture for ScriptedCapture {
    fn complete_structured<'a>(
        &'a self,
        request: CompletionRequest,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, ProviderError>> {
        self.recorded
            .lock()
            .expect("recorded lock")
            .push((request, schema));
        let answer = match &self.answer {
            Answer::Value(value) => Ok(value.clone()),
            Answer::Failure(kind, message, status) => {
                let mut error = ProviderError::new(*kind, *message);
                error.status = *status;
                Err(error)
            }
        };
        Box::pin(async move { answer })
    }
}

/// The summarizer must ASK for the schema — passing `json!({})`, or only
/// inlining it in the prompt text, leaves the required list empty.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_request_carries_the_schema() {
    let capture = ScriptedCapture::new(Answer::Value(
        json!({ "summary": "s", "type": "task", "tags": [] }),
    ));
    let summarizer = ProviderCaptureSummarizer::new(capture.clone(), MODEL, 131_072);

    let _ = summarizer.summarize("user: capture this").await;

    let recorded = capture.recorded();
    assert_eq!(recorded.len(), 1, "one structured call");
    let required = recorded[0]
        .1
        .get("required")
        .and_then(|required| required.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        required,
        vec![json!("summary"), json!("type"), json!("tags")],
        "the frozen capture schema is what the provider is asked to validate"
    );
}

/// Mapping every error to `summarizer_failed` would hide the one failure the
/// operator can fix in config.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_structured_auth_failure_keeps_the_named_reason() {
    let capture = ScriptedCapture::new(Answer::Failure(
        ProviderErrorKind::Auth,
        "host rejected the credential",
        Some(401),
    ));
    let summarizer = ProviderCaptureSummarizer::new(capture, MODEL, 131_072);

    let error = summarizer
        .summarize("user: capture this")
        .await
        .expect_err("an auth rejection is not a capture");
    assert!(
        error.to_string().contains("host rejected the credential"),
        "the provider message survives: {error}"
    );
    let failure = summarizer.last_failure().expect("the failure is recorded");
    assert_eq!(failure.reason, "host_auth_rejected");
    assert_eq!(failure.status, Some(401));
}

/// The empty-summary rejection must survive the move off the string path.
#[test]
fn test_empty_summary_is_rejected_on_the_value_path() {
    let error = mem_bridge::capture_prompt::parse_capture_value(
        json!({ "summary": "  ", "type": "task", "tags": [] }),
    )
    .expect_err("a whitespace-only summary is not a capture");
    assert!(
        error.to_string().contains("empty summary"),
        "the named reason survives: {error}"
    );
}

/// `skip` short-circuits BEFORE the empty-summary check: reordering the two
/// turns every legitimate skip into a dead-letter.
#[test]
fn test_skip_type_short_circuits_before_the_empty_summary_check() {
    let output = mem_bridge::capture_prompt::parse_capture_value(
        json!({ "summary": "", "type": "skip", "tags": [] }),
    )
    .expect("a skip is a valid answer, not a parse failure");
    assert_eq!(
        output,
        CaptureSummaryOutput::Skip(mem_bridge::capture::SkipReason::ProviderSkipped)
    );
}

/// A parse failure is the MODEL's fault, not the provider's: recording it as a
/// provider fault would poison the next attempt's diagnosis.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_capture_summary_parse_failure_sets_no_provider_record() {
    let capture = ScriptedCapture::new(Answer::Value(Value::String("not json".to_string())));
    let summarizer = ProviderCaptureSummarizer::new(capture, MODEL, 131_072);

    let _ = summarizer
        .summarize("user: capture this")
        .await
        .expect_err("a non-object value is not a capture");
    assert_eq!(
        summarizer.last_failure(),
        None,
        "a parse failure never sets a provider record"
    );
}

/// Row 11 — the wire-shape mismatch is the one failure a capture can name
/// itself, and the `else` arm would flatten it to `summarizer_failed` — the
/// same token a model-side JSON miss produces, so the operator could not tell
/// "the host answered another shape" from "the model rambled".
#[test]
fn test_classify_gives_a_wire_shape_failure_its_own_reason() {
    let error = ProviderError::new(
        ProviderErrorKind::WireShape,
        "session prompt answered without a {\"data\":{\"text\"}} envelope",
    );
    let failure = mem_bridge::capture_prompt::classify_provider_error(&error);
    assert_eq!(failure.reason, "host_wire_shape_mismatch");
    assert_eq!(failure.error_kind, "wire-shape");
    assert_eq!(failure.status, None, "no HTTP status was involved");
}

/// Row 12 — the split is two-sided: mapping EVERY `Protocol` failure onto the
/// new reason would hide the model-side miss behind a host-shaped name.
#[test]
fn test_classify_leaves_a_protocol_failure_unnamed() {
    let error = ProviderError::new(
        ProviderErrorKind::Protocol,
        "structured output was not valid JSON",
    );
    let failure = mem_bridge::capture_prompt::classify_provider_error(&error);
    assert_eq!(failure.reason, "summarizer_failed");
    assert_eq!(failure.error_kind, "protocol");
}

/// Issue 108 — every declared host class maps to a NAMED capture reason.
///
/// The bodies are the declared error schemas (from the live OpenAPI document),
/// not hand-written reasons; the classifier reads the decoded `wire_reason`,
/// never the message. A classifier that sniffed `message` fails every row here,
/// because a production message carries no class token.
#[test]
fn test_108_classify_maps_every_wire_reason_to_a_named_capture_reason() {
    use mem_ai::provider::decode_generate_failure;

    let cases: [(u16, &str, &str); 10] = [
        (
            400,
            r#"{"_tag":"InvalidRequestError","message":"x","kind":"body","field":"prompt"}"#,
            "host_invalid_request",
        ),
        (
            401,
            r#"{"_tag":"UnauthorizedError","message":"x"}"#,
            "host_auth_rejected",
        ),
        (
            404,
            r#"{"_tag":"ProviderNotFoundError","providerID":"p","message":"x"}"#,
            "host_provider_not_found",
        ),
        (
            404,
            r#"{"_tag":"ModelNotFoundError","providerID":"p","modelID":"m","message":"x"}"#,
            "host_model_not_found",
        ),
        (
            404,
            r#"{"_tag":"LocationNotFoundError","location":{"directory":"/x"},"message":"x"}"#,
            "host_location_not_found",
        ),
        (
            422,
            r#"{"_tag":"UnsupportedModelError","providerID":"p","modelID":"m","capability":"structured_output","message":"x"}"#,
            "host_model_unsupported",
        ),
        (
            502,
            r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"rate_limit","message":"x"}"#,
            "host_provider_request_error",
        ),
        (
            503,
            r#"{"_tag":"ServiceUnavailableError","message":"x","service":"llm"}"#,
            "host_service_unavailable",
        ),
        (404, "", "host_route_moved"),
        (
            404,
            r#"{"_tag":"SomethingNewError","message":"x"}"#,
            "host_unrecognised_error",
        ),
    ];
    for (status, body, expected) in cases {
        let error = decode_generate_failure(status, body);
        let failure = mem_bridge::capture_prompt::classify_provider_error(&error);
        assert_eq!(failure.reason, expected, "body {body}");
        assert_eq!(failure.error_kind, error.kind.as_str(), "body {body}");
        assert_eq!(
            error.message,
            format!("POST /experimental/generate/text failed ({status})"),
            "the message is fixed per status, body {body}"
        );
        assert!(
            !error.message.contains("_tag") && !error.message.contains("SomethingNew"),
            "the message never carries the class token: {}",
            error.message
        );
    }
}

/// A permanent `422` is not retryable and its detail names the declared fields:
/// the operator can fix the model id without reading a reply.
#[test]
fn test_108_classify_unsupported_model_is_not_retryable_and_names_it() {
    let error = mem_ai::provider::decode_generate_failure(
        422,
        r#"{"_tag":"UnsupportedModelError","providerID":"p","modelID":"m","capability":"structured_output","message":"x"}"#,
    );
    let failure = mem_bridge::capture_prompt::classify_provider_error(&error);
    assert_eq!(failure.reason, "host_model_unsupported");
    assert!(!failure.retryable, "a 422 cannot succeed on retry");
    let detail = failure
        .detail
        .as_deref()
        .expect("the detail names the keys");
    assert!(detail.contains("providerID=p"), "{detail}");
    assert!(detail.contains("modelID=m"), "{detail}");
    assert!(detail.contains("capability=structured_output"), "{detail}");
}

/// A `502` splits on the declared `reason`, carries `retryAfterMs`, and an
/// absent/unknown reason stays RETRYABLE — inverting that default would turn a
/// newer host's `quota_exhausted` into permanent loss.
#[test]
fn test_108_classify_502_carries_retry_after_and_its_two_classes() {
    let classify = |body: &str| {
        let error = mem_ai::provider::decode_generate_failure(502, body);
        mem_bridge::capture_prompt::classify_provider_error(&error)
    };

    let rate_limited = classify(
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"rate_limit","message":"x","retryAfterMs":1500}"#,
    );
    assert_eq!(rate_limited.reason, "host_provider_request_error");
    assert!(rate_limited.retryable, "rate_limit is transient");
    assert_eq!(rate_limited.retry_after_ms, Some(1500));
    assert!(
        rate_limited
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("rate_limit"),
        "{:?}",
        rate_limited.detail
    );

    let authentication = classify(
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"authentication","message":"x"}"#,
    );
    assert!(!authentication.retryable, "authentication is terminal");
    assert!(
        authentication
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("authentication"),
        "{:?}",
        authentication.detail
    );

    let absent = classify(r#"{"_tag":"ProviderRequestError","providerID":"p","message":"x"}"#);
    assert!(absent.retryable, "an absent reason defaults to retryable");
    assert!(
        absent
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("reason=<absent>"),
        "{:?}",
        absent.detail
    );

    let unknown = classify(
        r#"{"_tag":"ProviderRequestError","providerID":"p","reason":"quota_exhausted","message":"x"}"#,
    );
    assert!(unknown.retryable, "an unknown reason defaults to retryable");
    assert!(
        unknown
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("quota_exhausted"),
        "{:?}",
        unknown.detail
    );
}

/// The model's own miss is NAMED and RETRYABLE — `ModelOutput`, never
/// `summarizer_failed`, and the detail carries only the reply length.
#[test]
fn test_108_classify_model_output_is_retryable() {
    let error =
        mem_ai::provider::remote::structured::parse_structured_lenient("p", STREAMED_MARKDOWN)
            .expect_err("markdown carries no structured value");
    let failure = mem_bridge::capture_prompt::classify_provider_error(&error);
    assert_eq!(failure.reason, "capture_output_not_json");
    assert!(failure.retryable);
    let detail = failure.detail.as_deref().expect("a length detail");
    assert!(detail.contains("reply 308 bytes"), "{detail}");
    assert!(
        !detail.contains("INJECT_BARRIER_BUDGET"),
        "the reply text is never echoed: {detail}"
    );
}
