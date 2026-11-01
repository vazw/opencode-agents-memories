//! OpenCode host provider contract (upstream `opencode-provider.test.ts`).
//!
//! Drives the provider over an injected mock HTTP transport: streaming
//! completion, structured output over the stateless generate route, configured
//! timeouts, connected-provider refresh, and the trait-only leakage gate.
//!
//! The provider makes exactly ONE request per attempt —
//! `POST {base}/experimental/generate/text` — so the rows below pin the URL,
//! the body's declared keys (`prompt`, plus `model` when one is configured),
//! and the `{"data":{"text": …}}` reply. Every shape is READ from the API
//! description (`docs/evidence/issue-88-host-wire-contract-20261101/`, the
//! live 2.2.5 OpenAPI document); no exchange with a live host was captured.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use mem_ai::provider::{
    AiProvider, AiProviderStructured, BoxFuture, ChatMessage, CompletionRequest, HostAuthorization,
    HostMethod, HostRequest, HostResponse, OpenCodeHostConfig, OpenCodeHostProvider, ProviderError,
    ProviderErrorKind, ProviderKind, ProviderRegistry, StaticSecretResolver, Zeroizing,
};

/// The generate URL the provider must call, built from `host_config()`.
const GENERATE_URL: &str = "http://127.0.0.1:49374/api/experimental/generate/text";

/// Records every host call and synthesises the stateless generate route.
struct MockTransport {
    calls: Mutex<Vec<(HostMethod, String, Option<serde_json::Value>)>>,
    /// The credential each request carried (the transport seam's contract).
    authorizations: Mutex<Vec<Option<HostAuthorization>>>,
    /// When true, the generate call never resolves.
    hang_generate: bool,
    /// The generate reply's `text`, delivered inside the documented
    /// `{"data":{"text": …}}` envelope.
    text: String,
    /// A raw generate reply body, for the rows that pin a WRONG envelope.
    reply_body: Option<String>,
}

impl MockTransport {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            authorizations: Mutex::new(Vec::new()),
            hang_generate: false,
            text: "hello world".to_owned(),
            reply_body: None,
        }
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
            && request.url.ends_with("/experimental/generate/text")
        {
            return Box::pin(std::future::pending());
        }

        let response = match request.method {
            HostMethod::Post if request.url.ends_with("/experimental/generate/text") => {
                HostResponse {
                    status: 200,
                    body: self.reply_body.clone().unwrap_or_else(|| {
                        format!(
                            r#"{{"data":{{"text":{}}}}}"#,
                            serde_json::to_string(&self.text).expect("the reply text encodes")
                        )
                    }),
                }
            }
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

    // One stateless call: no create, no prompt-session, no delete.
    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "exactly one request: {calls:?}");
    assert_eq!(calls[0].1, GENERATE_URL);
}

#[tokio::test]
async fn test_opencode_host_timeout_config() {
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
        .expect_err("a hanging generate call times out on the configured budget");
    assert_eq!(error.kind, ProviderErrorKind::Timeout);
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
    assert_eq!(recorded.len(), 1, "one stateless call: {recorded:?}");
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
    assert_eq!(transport.calls()[0].1, GENERATE_URL);
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

/// A config whose resolved model is the host's own choice: the `inherit`
/// sentinel means "send no model at all", never `{"id":"inherit"}`.
fn inherit_host_config() -> OpenCodeHostConfig {
    OpenCodeHostConfig {
        model: "inherit".to_owned(),
        base_url: "http://127.0.0.1:49374/api".to_owned(),
        ..OpenCodeHostConfig::default()
    }
}

/// The stateless route takes `{"prompt": …}` and, when one is resolved, the
/// declared `Model.Ref` — and NOTHING else: `additionalProperties:false` is a
/// runtime contract here, unlike the old create body's undeclared `permission`.
#[tokio::test]
async fn test_108_posts_to_experimental_generate_text_with_prompt_and_model() {
    let transport = Arc::new(MockTransport::new());
    let config = OpenCodeHostConfig {
        provider_id: "opencode-go".to_owned(),
        model: "beam-free".to_owned(),
        base_url: "http://127.0.0.1:49374/api".to_owned(),
        ..OpenCodeHostConfig::default()
    };
    let provider = OpenCodeHostProvider::new(config, transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "one stateless request: {calls:?}");
    assert_eq!(calls[0].0, HostMethod::Post);
    assert_eq!(calls[0].1, GENERATE_URL);
    let body = calls[0].2.as_ref().expect("the call carries a JSON body");
    let mut keys: Vec<&String> = body
        .as_object()
        .expect("the body is an object")
        .keys()
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["model", "prompt"], "declared keys only: {body}");
    assert_eq!(
        body["model"],
        serde_json::json!({ "id": "beam-free", "providerID": "opencode-go" }),
        "`Model.Ref{{id, providerID}}` is the declared shape: {body}"
    );
    for undeclared in ["agent", "format", "permission", "modelID"] {
        assert!(
            body.get(undeclared).is_none(),
            "`{undeclared}` is not declared on this route: {body}"
        );
    }
}

/// One attempt is exactly one HTTP request: a residual create+delete or a
/// provider-internal retry multiplies the capture ladder.
#[tokio::test]
async fn test_108_exactly_one_request_per_attempt() {
    let transport = Arc::new(MockTransport::new());
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "exactly one request per attempt: {calls:?}");
    assert_eq!(calls[0].1, GENERATE_URL);
    assert_eq!(calls[0].0, HostMethod::Post);
    assert!(
        calls.iter().all(|(_, url, _)| !url.contains("/session")),
        "no session machinery survives: {calls:?}"
    );
}

/// `inherit` is the port's sentinel for "use the host's base configuration",
/// so the `model` key must be absent rather than carrying `{"id":"inherit"}`.
#[tokio::test]
async fn test_108_inherit_model_omits_the_model_key() {
    let transport = Arc::new(MockTransport::new());
    let provider = OpenCodeHostProvider::new(inherit_host_config(), transport.clone());

    let mut stream = provider.complete(plain_request());
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let body = calls[0].2.as_ref().expect("the call carries a JSON body");
    let keys: Vec<&String> = body
        .as_object()
        .expect("the body is an object")
        .keys()
        .collect();
    assert_eq!(
        keys,
        vec!["prompt"],
        "no model key when it is inherited: {body}"
    );
}

/// The documented reply envelope carries the structured value as TEXT, so
/// reading `info.structured_output` reds this row.
#[tokio::test]
async fn test_108_generate_text_carries_the_structured_value() {
    let transport = Arc::new(MockTransport::new().with_reply_body(
        r#"{"data":{"text":"{\"summary\":\"s\",\"type\":\"task\",\"tags\":[]}"}}"#,
    ));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let request = CompletionRequest::new("gpt-4o-mini");
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "type": { "type": "string" },
            "tags": { "type": "array" }
        }
    });
    let value = AiProviderStructured::complete_structured(&provider, request, schema)
        .await
        .expect("the reply text parses");
    assert_eq!(value["summary"], "s");
    assert_eq!(value["type"], "task");
}

/// A reply that is not the documented envelope is a SHAPE mismatch, so
/// reporting `Protocol` loses the split between "wrong shape" and "the model
/// did not answer in JSON".
#[tokio::test]
async fn test_108_generate_reply_without_an_envelope_is_a_wire_shape_error() {
    let transport = Arc::new(MockTransport::new().with_reply_body(r#"{"id":"ses_1"}"#));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let error = AiProviderStructured::complete_structured(
        &provider,
        CompletionRequest::new("gpt-4o-mini"),
        serde_json::json!({ "type": "object" }),
    )
    .await
    .expect_err("a bare session id is not the generate envelope");
    assert_eq!(error.kind, ProviderErrorKind::WireShape);
    assert!(
        error.message.contains("data") && error.message.contains("text"),
        "the reason names the envelope it wanted: {}",
        error.message
    );
}

/// Prose is the MODEL's miss, not the wire's: it is a named `ModelOutput`
/// outcome (retryable) rather than `WireShape` or a bare `Protocol`.
#[tokio::test]
async fn test_108_prose_reply_is_a_named_model_output_miss() {
    let transport = Arc::new(MockTransport::new().with_text("Here is a summary of the work."));
    let provider = OpenCodeHostProvider::new(host_config(), transport.clone());

    let error = AiProviderStructured::complete_structured(
        &provider,
        CompletionRequest::new("gpt-4o-mini"),
        serde_json::json!({ "type": "object" }),
    )
    .await
    .expect_err("prose is not a structured answer");
    assert_eq!(error.kind, ProviderErrorKind::ModelOutput);
    assert!(
        error.message.contains("direct JSON")
            && error.message.contains("fenced block")
            && error.message.contains("balanced object"),
        "the miss names all three shapes: {}",
        error.message
    );
    assert!(
        !error.message.contains("Here is a summary"),
        "the reply text is never echoed: {}",
        error.message
    );
}
