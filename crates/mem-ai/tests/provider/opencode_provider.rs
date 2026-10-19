//! OpenCode host provider contract (upstream `opencode-provider.test.ts`).
//!
//! Drives the provider over an injected mock HTTP transport: streaming
//! completion, structured output + the step-budget loop guard, configured
//! timeouts, connected-provider refresh, and the trait-only leakage gate.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use mem_ai::provider::{
    AiProvider, AiProviderStructured, BoxFuture, ChatMessage, CompletionRequest, HostMethod,
    HostRequest, HostResponse, OpenCodeHostConfig, OpenCodeHostProvider, ProviderError,
    ProviderErrorKind, ProviderKind, ProviderRegistry, STRUCTURED_OUTPUT_MAX_STEPS,
    StaticSecretResolver,
};

/// Records every host call and synthesises the OpenCode v2 session protocol.
struct MockTransport {
    calls: Mutex<Vec<(HostMethod, String)>>,
    /// When true, `POST .../message` never resolves (agent loop).
    hang_message: bool,
    /// When true, the prompt returns `info.structured_output`.
    structured: bool,
    text: String,
}

impl MockTransport {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            hang_message: false,
            structured: false,
            text: "hello world".to_owned(),
        }
    }

    fn calls(&self) -> Vec<(HostMethod, String)> {
        self.calls.lock().unwrap().clone()
    }

    fn deleted(&self) -> bool {
        self.calls()
            .iter()
            .any(|(method, _)| *method == HostMethod::Delete)
    }
}

impl mem_ai::provider::HostTransport for MockTransport {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        self.calls
            .lock()
            .unwrap()
            .push((request.method, request.url.clone()));

        if self.hang_message
            && request.method == HostMethod::Post
            && request.url.contains("/message")
        {
            return Box::pin(std::future::pending());
        }

        let response = match request.method {
            HostMethod::Post if request.url.ends_with("/session") => HostResponse {
                status: 200,
                body: r#"{"id":"ses_test"}"#.to_owned(),
            },
            HostMethod::Post if request.url.contains("/message") => {
                if self.structured {
                    HostResponse {
                        status: 200,
                        body: r#"{"info":{"structured_output":{"topic":"auth","count":3}},"parts":[]}"#
                            .to_owned(),
                    }
                } else {
                    HostResponse {
                        status: 200,
                        body: format!(
                            r#"{{"info":{{}},"parts":[{{"type":"text","text":{}}}]}}"#,
                            serde_json::to_string(&self.text).unwrap()
                        ),
                    }
                }
            }
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

    // Session create → prompt → delete, all through the host transport.
    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "create + prompt + delete: {calls:?}");
    assert!(transport.deleted());
}

#[tokio::test]
async fn test_opencode_host_structured_output_loop() {
    let mut mock = MockTransport::new();
    mock.structured = true;
    let transport = Arc::new(mock);
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
    let mut mock = MockTransport::new();
    mock.hang_message = true;
    let transport = Arc::new(mock);

    let config = OpenCodeHostConfig {
        timeout: Duration::from_millis(30),
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
