//! OpenCode host provider contract (upstream `opencode-provider.test.ts`).
//!
//! Drives the provider over an injected mock HTTP transport: streaming
//! completion, structured output + the step-budget loop guard, configured
//! timeouts, connected-provider refresh, and the trait-only leakage gate.
//!
//! FIXTURE NOTE: the `{"id":…}` / `{"info":…}` create/prompt bodies below are
//! synthetic. They stay because they exercise the provider's own session
//! lifecycle; the DOCUMENTED 2.2.5 shapes are pinned by the `test_2_2_5_*` rows
//! here (from `docs/evidence/issue-88-host-wire-contract-20261101/`) and by the
//! measured `/api/config` document-array rows. Every such shape is READ from the
//! API description — no exchange with a live host has ever been captured.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use mem_ai::provider::{
    AiProvider, AiProviderStructured, BoxFuture, ChatMessage, CompletionRequest, HostAuthorization,
    HostMethod, HostRequest, HostResponse, OpenCodeHostConfig, OpenCodeHostProvider, ProviderError,
    ProviderErrorKind, ProviderKind, ProviderRegistry, STRUCTURED_OUTPUT_MAX_STEPS,
    StaticSecretResolver, Zeroizing,
};

/// The sample create body from
/// `docs/evidence/issue-88-host-wire-contract-20261101/create-body.sample.json`
/// (`/api/session.post` → `{data: Session.Info}`). Schema-derived: this is what
/// the document declares, not a captured exchange.
const SAMPLED_2_2_5_CREATE_BODY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/evidence/issue-88-host-wire-contract-20261101/create-body.sample.json"
));

/// Records every host call and synthesises the OpenCode v2 session protocol.
struct MockTransport {
    calls: Mutex<Vec<(HostMethod, String, Option<serde_json::Value>)>>,
    /// The credential each request carried (the transport seam's contract).
    authorizations: Mutex<Vec<Option<HostAuthorization>>>,
    /// When true, the prompt call never resolves (agent loop).
    hang_generate: bool,
    /// The prompt's `text`, delivered inside the documented
    /// `{"data":{"text": …}}` envelope.
    text: String,
    /// A raw prompt reply body, for the rows that pin a WRONG envelope.
    reply_body: Option<String>,
    /// The `POST …/session` answer, so a row can pin any create shape.
    create_body: String,
}

impl MockTransport {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            hang_generate: false,
            text: "hello world".to_owned(),
            reply_body: None,
            create_body: r#"{"id":"ses_test"}"#.to_owned(),
        }
    }

    fn with_create_body(mut self, create_body: &str) -> Self {
        self.create_body = create_body.to_owned();
        self
    }

    fn with_text(mut self, text: &str) -> Self {
        self.text = text.to_owned();
        self
    }

    fn with_reply_body(mut self, reply_body: &str) -> Self {
        self.reply_body = Some(reply_body.to_owned());
        self
    }

    fn calls(&self) -> Vec<(HostMethod, String, Option<serde_json::Value>)> {
        self.calls.lock().unwrap().clone()
    }

    fn authorizations(&self) -> Vec<Option<HostAuthorization>> {
        self.authorizations.lock().unwrap().clone()
    }

    fn deleted(&self) -> bool {
        self.calls()
            .iter()
            .any(|(method, _, _)| *method == HostMethod::Delete)
    }
}

impl mem_ai::provider::HostTransport for MockTransport {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        self.calls.lock().unwrap().push((
            request.method,
            request.url.clone(),
            request.body.clone(),
        ));
        self.authorizations
            .lock()
            .unwrap()
            .push(request.authorization.clone());

        if self.hang_generate
            && request.method == HostMethod::Post
            && request.url.contains("/generate")
        {
            return Box::pin(std::future::pending());
        }

        let response = match request.method {
            HostMethod::Post if request.url.ends_with("/session") => HostResponse {
                status: 200,
                body: self.create_body.clone(),
            },
            HostMethod::Post if request.url.contains("/generate") => HostResponse {
                status: 200,
                body: self.reply_body.clone().unwrap_or_else(|| {
                    format!(
                        r#"{{"data":{{"text":{}}}}}"#,
                        serde_json::to_string(&self.text).expect("the reply text encodes")
                    )
                }),
            },
            HostMethod::Delete => HostResponse {
                status: 200,
                body: "true".to_owned(),
            },
            _ => HostResponse {
                status: 404,
                body: String::new(),
            },
        };
        Box::pin(std::future::ready(Ok(response)))
    }
}

fn host_config() -> OpenCodeHostConfig {
    OpenCodeHostConfig {
        model: "gpt-4o-mini".to_owned(),
        base_url: "http://127.0.0.1:49374/api".to_owned(),
        ..OpenCodeHostConfig::default()
    }
}

fn plain_request() -> CompletionRequest {
    let mut request = CompletionRequest::new("gpt-4o-mini");
    request.messages.push(ChatMessage::user("user prompt"));
    request
}

#[tokio::test]
async fn test_opencode_host_complete_streams() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new());
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    let mut text = String::new();
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk ok");
        if let Some(value) = chunk.get("text").and_then(|value| value.as_str()) {
            text.push_str(value);
        }
        if chunk.get("done").and_then(|value| value.as_bool()) == Some(true) {
            saw_done = true;
        }
    }

    assert_eq!(text, "hello world");
    assert!(saw_done, "stream terminates with a done chunk");

    // Session create → prompt → delete, all through the host transport.
    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "create + prompt + delete: {calls:?}");
    assert!(transport.deleted());
}

#[tokio::test]
async fn test_opencode_host_structured_output_loop() {
    let _gate = crate::lock_global_capture();
    // The documented reply carries the structured value as TEXT inside the
    // `{"data":{"text": …}}` envelope; the shared parser strips one fence.
    let transport =
        Arc::new(MockTransport::new().with_text("```json\n{\"topic\":\"auth\",\"count\":3}\n```"));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    // Structured completion returns the parsed JSON schema value.
    let request = CompletionRequest::new("gpt-4o-mini");
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "topic": { "type": "string" }, "count": { "type": "number" } },
        "required": ["topic", "count"]
    });
    let value = AiProviderStructured::complete_structured(&provider, request, schema)
        .await
        .expect("structured output");
    assert_eq!(value["topic"], "auth");
    assert_eq!(value["count"], 3);

    // The step counter is the client-side loop guard: it aborts after
    // STRUCTURED_OUTPUT_MAX_STEPS (2) `step-start` parts, and only counts for
    // live internal capture sessions.
    assert_eq!(STRUCTURED_OUTPUT_MAX_STEPS, 2);
    let guard = mem_ai::provider::InternalCaptureSessions::global();
    guard.clear();
    assert_eq!(
        (
            provider.note_structured_step("ses_unknown").steps,
            provider.note_structured_step("ses_unknown").should_abort
        ),
        (0, false),
        "untracked sessions are not step-counted"
    );
    guard.track("ses_loop");
    assert!(provider.is_internal_structured_session("ses_loop"));
    let first = provider.note_structured_step("ses_loop");
    assert_eq!((first.steps, first.should_abort), (1, false));
    let second = provider.note_structured_step("ses_loop");
    assert_eq!((second.steps, second.should_abort), (2, false));
    let third = provider.note_structured_step("ses_loop");
    assert_eq!((third.steps, third.should_abort), (3, true));
    guard.clear();
}

#[tokio::test]
async fn test_opencode_host_timeout_config() {
    let _gate = crate::lock_global_capture();
    let mut mock = MockTransport::new();
    mock.hang_generate = true;
    let transport = Arc::new(mock);

    let config = OpenCodeHostConfig {
        timeout: Duration::from_millis(30),
        base_url: "http://127.0.0.1:49374/api".to_owned(),
        ..host_config()
    };
    let provider = OpenCodeHostProvider::new(config, transport.clone());

    let request = CompletionRequest::new("gpt-4o-mini");
    let schema = serde_json::json!({ "type": "object" });
    let error = AiProviderStructured::complete_structured(&provider, request, schema)
        .await
        .expect_err("hanging prompt times out on the configured budget");
    assert_eq!(error.kind, ProviderErrorKind::Timeout);

    // Cleanup still runs for the transient session.
    assert!(transport.deleted());
}

#[tokio::test]
async fn test_opencode_host_connected_refresh() {
    let transport = Arc::new(MockTransport::new());
    let provider = Arc::new(OpenCodeHostProvider::new(host_config(), transport));

    provider.set_connected_providers(vec!["opencode".to_owned()]);
    provider.set_connected_providers_refresher(Arc::new({
        let weak = Arc::downgrade(&provider);
        move || {
            let weak = weak.clone();
            Box::pin(async move {
                if let Some(provider) = weak.upgrade() {
                    provider
                        .set_connected_providers(vec!["opencode".to_owned(), "newapi".to_owned()]);
                }
            })
        }
    }));

    assert!(provider.ensure_provider_connected("opencode").await);
    assert!(provider.ensure_provider_connected("newapi").await);
    assert!(!provider.ensure_provider_connected("ghost").await);
}

#[test]
fn test_no_provider_leakage() {
    // Consumers can only talk to providers through the object-safe trait:
    // `dyn AiProvider` is constructible and the factory returns it erased.
    fn assert_object_safe(_provider: &dyn AiProvider) {}

    let registry = {
        let mut registry = ProviderRegistry::new(Arc::new(StaticSecretResolver::new()));
        registry.register_builtin();
        registry
    };
    let config = mem_ai::provider::ProviderConfig::OpenCodeHost(host_config());
    assert_eq!(config.kind(), ProviderKind::OpenCodeHost);

    let provider: Arc<dyn AiProvider> = registry.create(&config).expect("resolves");
    assert_object_safe(provider.as_ref());
    assert_eq!(provider.name(), "opencode-host");

    // The registry's public resolver seam is the trait, not a concrete impl.
    let resolver: &Arc<dyn mem_ai::provider::SecretResolver> = registry.resolver();
    assert!(Arc::strong_count(resolver) >= 1);
}

/// The credential the host requires on every `{origin}/api` request.
#[tokio::test]
async fn test_opencode_host_sends_basic_authorization() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new());
    let config = OpenCodeHostConfig {
        base_url: "http://127.0.0.1:49374/api".to_owned(),
        authorization: Some(HostAuthorization::basic_from_owned(
            "opencode",
            Zeroizing::new("s3cret".to_string()),
        )),
        ..host_config()
    };
    let provider = OpenCodeHostProvider::new(config, transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let recorded = transport.authorizations();
    assert_eq!(recorded.len(), 3, "create + prompt + delete: {recorded:?}");
    for authorization in &recorded {
        let authorization = authorization
            .as_ref()
            .expect("every host request carries the resolved credential");
        assert_eq!(
            authorization.header_value().as_str(),
            "Basic b3BlbmNvZGU6czNjcmV0"
        );
    }
    // The URL the header is attached to is the `/api` base, never the root.
    assert_eq!(transport.calls()[0].1, "http://127.0.0.1:49374/api/session");
}

/// Answers `GET /config` with a fixed document, 404 elsewhere.
struct ConfigTransport {
    config_body: String,
}

impl ConfigTransport {
    fn new(config_body: &str) -> Self {
        Self {
            config_body: config_body.to_owned(),
        }
    }
}

impl mem_ai::provider::HostTransport for ConfigTransport {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        let response = if request.method == HostMethod::Get && request.url.ends_with("/api/config")
        {
            HostResponse {
                status: 200,
                body: self.config_body.clone(),
            }
        } else {
            HostResponse {
                status: 404,
                body: String::new(),
            }
        };
        Box::pin(std::future::ready(Ok(response)))
    }
}

/// `GET /api/config` returns `Config.Entry[]` (measured live shape): a
/// top-level ARRAY of documents whose `info.providers` is an object keyed by
/// provider id.
#[tokio::test]
async fn test_provider_ids_read_the_config_document_array() {
    let transport = Arc::new(ConfigTransport::new(
        r#"[{"type":"document","info":{"providers":{"opencode-go":{"models":{"beam-free":{}}}}}},{"type":"document","info":{"providers":{"github-copilot":{"models":{}}}}}]"#,
    ));
    let provider = OpenCodeHostProvider::new(host_config(), transport);

    provider
        .refresh_connected_providers()
        .await
        .expect("the measured array parses");
    assert!(provider.is_provider_connected("opencode-go"));
    assert!(provider.is_provider_connected("github-copilot"));
    assert!(
        !provider.is_provider_connected("providers"),
        "the legacy `value.get(\"providers\")` read yields nothing against an array"
    );
}

/// A live entry carries `{"disabled":…}` with no `modelID`: the object KEY is
/// the id, and such entries are KEPT, never skipped.
#[tokio::test]
async fn test_models_from_config_read_the_config_document_array() {
    let transport = Arc::new(ConfigTransport::new(
        r#"[{"type":"document","info":{"providers":{"opencode-go":{"models":{"m":{"disabled":true},"n":{"modelID":"n","name":"Model N"}}}}}}]"#,
    ));
    let provider = OpenCodeHostProvider::new(host_config(), transport);

    let models = provider
        .list_models()
        .await
        .expect("the measured array parses");
    assert_eq!(models.len(), 2, "{models:?}");
    let mut with_metadata: Vec<_> = models
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.provider.as_str(),
                model.display_name.as_deref(),
            )
        })
        .collect();
    with_metadata.sort_unstable();
    assert_eq!(
        with_metadata,
        vec![
            ("m", "opencode-go", None),
            ("n", "opencode-go", Some("Model N")),
        ]
    );
}

/// Config carrying the credential the host requires on every `{origin}/api`
/// request.
fn authorized_host_config() -> OpenCodeHostConfig {
    OpenCodeHostConfig {
        authorization: Some(HostAuthorization::basic_from_owned(
            "opencode",
            Zeroizing::new("s3cret".to_string()),
        )),
        ..host_config()
    }
}

/// The first chunk of one completion: `run()` completes (create → prompt →
/// delete) before any chunk is yielded, so this also flushes every call.
async fn first_chunk(
    provider: &OpenCodeHostProvider,
    request: CompletionRequest,
) -> Result<serde_json::Value, ProviderError> {
    provider.complete(request).next().await.expect("one chunk")
}

/// Row 1 — AC1: the documented create envelope descends to the prompt call.
///
/// The previous row (`test_2_2_5_session_create_response_is_a_named_protocol_error`)
/// pinned the BUG: `{"data":{"id":…}}` decoded as "no session id". Its still-true
/// claims — the create URL, the credential on the create, and "no prompt without
/// a session id" — are inherited here and by row 5.
#[tokio::test]
async fn test_2_2_5_create_envelope_yields_the_prompt_call() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_create_body(SAMPLED_2_2_5_CREATE_BODY));
    let provider = OpenCodeHostProvider::new(authorized_host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "create + prompt + delete: {calls:?}");
    assert_eq!(calls[0].0, HostMethod::Post);
    assert_eq!(calls[0].1, "http://127.0.0.1:49374/api/session");
    assert!(
        calls[1].1.contains("/session/ses_2_2_5/"),
        "the envelope's id reaches the prompt call: {calls:?}"
    );
    assert_eq!(calls[2].0, HostMethod::Delete);

    // The credential rides every request, starting with the create.
    assert_eq!(
        transport.authorizations()[0]
            .as_ref()
            .expect("the create carries the credential")
            .header_value()
            .as_str(),
        "Basic b3BlbmNvZGU6czNjcmV0"
    );
}

/// Row 2 — the bare shape is PERMANENT live behaviour (pre-2.2.5 hosts dial
/// fine), so deleting the fall-through reds this row.
#[tokio::test]
async fn test_2_2_5_bare_session_id_still_parses() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_create_body(r#"{"id":"ses_bare"}"#));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "create + prompt + delete: {calls:?}");
    assert!(
        calls[1].1.contains("/session/ses_bare/"),
        "the bare id reaches the prompt call: {calls:?}"
    );
}

/// Row 3 — `#[serde(untagged)]` declares the precedence: the ENVELOPE wins over
/// a sibling bare `id`, so flipping the variant order reds this row.
#[tokio::test]
async fn test_2_2_5_envelope_wins_over_a_bare_id() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(
        MockTransport::new().with_create_body(r#"{"id":"ses_bare","data":{"id":"ses_nested"}}"#),
    );
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "create + prompt + delete: {calls:?}");
    assert!(
        calls[1].1.contains("/session/ses_nested/"),
        "the enveloped id wins: {calls:?}"
    );
}

/// Row 4 — a create answer with no id in EITHER shape is a shape mismatch, not
/// a parse failure; the message names the call and never echoes the body.
#[tokio::test]
async fn test_2_2_5_create_without_any_id_is_a_wire_shape_error() {
    let _gate = crate::lock_global_capture();
    let transport =
        Arc::new(MockTransport::new().with_create_body(r#"{"data":{"projectID":"prj_1"}}"#));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let error = first_chunk(&provider, plain_request())
        .await
        .expect_err("a create answer without an id cannot prompt");
    assert_eq!(error.kind, ProviderErrorKind::WireShape);
    assert!(
        error.message.contains("session.create"),
        "the reason names the call: {}",
        error.message
    );
    assert!(
        !error.message.contains("prj_1"),
        "the response body is never echoed: {}",
        error.message
    );
    assert_eq!(transport.calls().len(), 1, "no prompt without a session id");
}

/// Row 5 — a blank id would build `/session//generate`; the guard turns it into
/// the same named shape error instead.
#[tokio::test]
async fn test_2_2_5_blank_session_id_is_a_wire_shape_error() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_create_body(r#"{"data":{"id":"   "}}"#));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let error = first_chunk(&provider, plain_request())
        .await
        .expect_err("an empty session id cannot prompt");
    assert_eq!(error.kind, ProviderErrorKind::WireShape);
    assert!(
        error.message.contains("blank session id"),
        "the reason names the blank id: {}",
        error.message
    );
    assert_eq!(transport.calls().len(), 1, "no prompt without a session id");
}

/// Row 6 — AC2: the prompt goes to the documented route and carries ONLY
/// `prompt`. The route has no `format` field, so everything else must be gone.
#[tokio::test]
async fn test_2_2_5_prompt_posts_to_generate_with_only_a_prompt() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_create_body(SAMPLED_2_2_5_CREATE_BODY));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "create + prompt + delete: {calls:?}");
    assert_eq!(
        calls[1].1, "http://127.0.0.1:49374/api/session/ses_2_2_5/generate",
        "the prompt takes the documented route"
    );
    let body = calls[1].2.as_ref().expect("the prompt carries a JSON body");
    let keys: Vec<&String> = body
        .as_object()
        .expect("the prompt body is an object")
        .keys()
        .collect();
    assert_eq!(keys, vec!["prompt"], "exactly one key: {body}");
}

/// Row 7 — the model and the structured-output agent travel on the CREATE body,
/// where `Model.Ref` declares them. Deleting `model_ref()`, keeping the old
/// `modelID` key, or omitting `agent` all red this row.
#[tokio::test]
async fn test_2_2_5_create_body_declares_model_and_agent() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_create_body(SAMPLED_2_2_5_CREATE_BODY));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    let body = calls[0].2.as_ref().expect("the create carries a JSON body");
    assert_eq!(
        body["model"],
        serde_json::json!({ "id": "gpt-4o-mini", "providerID": "opencode" }),
        "`Model.Ref{{id, providerID}}` is the declared shape: {body}"
    );
    assert_eq!(
        body["agent"], "opencode-mem-structured",
        "the structured-output agent is named on the create: {body}"
    );
    assert!(
        body.get("modelID").is_none(),
        "the undeclared `modelID` spelling is gone: {body}"
    );

    // A configured variant is `Model.Ref`'s optional third field.
    let transport = Arc::new(MockTransport::new().with_create_body(SAMPLED_2_2_5_CREATE_BODY));
    let provider = OpenCodeHostProvider::new(
        OpenCodeHostConfig {
            variant: Some("high".to_string()),
            ..host_config()
        },
        transport.clone(),
    );
    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}
    let calls = transport.calls();
    let body = calls[0].2.as_ref().expect("the create carries a JSON body");
    assert_eq!(body["model"]["variant"], "high", "{body}");
}

/// Row 8 — the documented reply envelope carries the structured value as text,
/// so reading `info.structured_output` reds this row.
#[tokio::test]
async fn test_2_2_5_generate_reply_text_carries_the_structured_value() {
    let _gate = crate::lock_global_capture();
    let transport =
        Arc::new(MockTransport::new().with_text("```json\n{\"topic\":\"auth\",\"count\":3}\n```"));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let request = CompletionRequest::new("gpt-4o-mini");
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "topic": { "type": "string" }, "count": { "type": "number" } },
        "required": ["topic", "count"]
    });
    let value = AiProviderStructured::complete_structured(&provider, request, schema)
        .await
        .expect("the reply text parses once its fence is stripped");
    assert_eq!(value["topic"], "auth");
    assert_eq!(value["count"], 3);
}

/// Row 9 — a reply that is not the documented envelope is a SHAPE mismatch, so
/// reporting `Protocol` loses the split between "wrong shape" and "the model
/// did not answer in JSON".
#[tokio::test]
async fn test_2_2_5_generate_reply_without_an_envelope_is_a_wire_shape_error() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_reply_body(
        r#"{"info":{"structured_output":{"topic":"auth","count":3}},"parts":[]}"#,
    ));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let error = AiProviderStructured::complete_structured(
        &provider,
        CompletionRequest::new("gpt-4o-mini"),
        serde_json::json!({ "type": "object" }),
    )
    .await
    .expect_err("the pre-2.2.5 reply is not the documented envelope");
    assert_eq!(error.kind, ProviderErrorKind::WireShape);
    assert!(
        error.message.contains("data") && error.message.contains("text"),
        "the reason names the envelope it wanted: {}",
        error.message
    );
}

/// Row 10 — prose is the MODEL's fault, not the wire's: mapping it to
/// `WireShape` would send an operator hunting a host that is answering fine.
#[tokio::test]
async fn test_2_2_5_prose_reply_stays_a_protocol_error() {
    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MockTransport::new().with_text("Here is a summary of the work."));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let error = AiProviderStructured::complete_structured(
        &provider,
        CompletionRequest::new("gpt-4o-mini"),
        serde_json::json!({ "type": "object" }),
    )
    .await
    .expect_err("prose is not a structured answer");
    assert_eq!(error.kind, ProviderErrorKind::Protocol);
    assert!(
        error.message.contains("not valid JSON"),
        "the shared parser's reason survives: {}",
        error.message
    );
    assert!(
        !error.message.contains("Here is a summary"),
        "the reply text is never echoed: {}",
        error.message
    );
}
