//! OpenCode host provider contract (upstream `opencode-provider.test.ts`).
//!
//! Drives the provider over an injected mock HTTP transport: streaming
//! completion, structured output + the step-budget loop guard, configured
//! timeouts, connected-provider refresh, and the trait-only leakage gate.
//!
//! FIXTURE NOTE: the `{"id":…}` / `{"info":…}` create/prompt bodies below are
//! the PRE-2.2.5 fiction. They stay because they exercise the provider's own
//! session lifecycle; the live 2.2.5 shapes are pinned by
//! `test_2_2_5_session_create_response_is_a_named_protocol_error` and by the
//! measured `/api/config` document-array rows. Rebuilding the lifecycle
//! fixtures is the wire wave's job.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use mem_ai::provider::{
    AiProvider, AiProviderStructured, BoxFuture, ChatMessage, CompletionRequest, HostAuthorization,
    HostMethod, HostRequest, HostResponse, OpenCodeHostConfig, OpenCodeHostProvider, ProviderError,
    ProviderErrorKind, ProviderKind, ProviderRegistry, STRUCTURED_OUTPUT_MAX_STEPS,
    StaticSecretResolver, Zeroizing,
};

/// Records every host call and synthesises the OpenCode v2 session protocol.
struct MockTransport {
    calls: Mutex<Vec<(HostMethod, String)>>,
    /// The credential each request carried (the transport seam's contract).
    authorizations: Mutex<Vec<Option<HostAuthorization>>>,
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
            authorizations: Mutex::new(Vec::new()),
            hang_message: false,
            structured: false,
            text: "hello world".to_owned(),
        }
    }

    fn calls(&self) -> Vec<(HostMethod, String)> {
        self.calls.lock().unwrap().clone()
    }

    fn authorizations(&self) -> Vec<Option<HostAuthorization>> {
        self.authorizations.lock().unwrap().clone()
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
        self.authorizations
            .lock()
            .unwrap()
            .push(request.authorization.clone());

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
    let _gate = crate::lock_global_capture();
    let mut mock = MockTransport::new();
    mock.hang_message = true;
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

/// The 2.2.5 create response is `{"data": Session.Info}` — the pre-2.2.5 port
/// looks for a top-level `id`, so the failure is a NAMED `Protocol` error. This
/// row pins the transport layer AND the wire mismatch; the wire wave deletes it
/// when it decodes `data`.
#[tokio::test]
async fn test_2_2_5_session_create_response_is_a_named_protocol_error() {
    struct MeasuredTransport {
        calls: Mutex<Vec<(HostMethod, String, Option<HostAuthorization>)>>,
    }

    impl mem_ai::provider::HostTransport for MeasuredTransport {
        fn send(
            &self,
            request: HostRequest,
        ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
            self.calls.lock().unwrap().push((
                request.method,
                request.url.clone(),
                request.authorization.clone(),
            ));
            // Copied from `.tmp/openapi.json` (`/api/session.post` →
            // `{data: Session.Info}`; required: id, projectID, cost, tokens,
            // time, location).
            let response = HostResponse {
                status: 200,
                body: r#"{"data":{"id":"ses_2_2_5","projectID":"prj_1","cost":0,"tokens":{"input":10,"output":20,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1,"updated":2},"location":{"directory":"/tmp"}}}"#
                    .to_owned(),
            };
            Box::pin(std::future::ready(Ok(response)))
        }
    }

    let _gate = crate::lock_global_capture();
    let transport = Arc::new(MeasuredTransport {
        calls: Mutex::new(Vec::new()),
    });
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
    let first = stream
        .next()
        .await
        .expect("the flattened error is the stream's first chunk")
        .expect_err("2.2.5's create response is not the port's shape");
    assert_eq!(first.kind, ProviderErrorKind::Protocol);
    assert!(
        first.message.contains("no session id"),
        "the reason names the missing session id: {}",
        first.message
    );

    let calls = transport.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one create, no prompt without a session id");
    assert_eq!(calls[0].0, HostMethod::Post);
    assert_eq!(calls[0].1, "http://127.0.0.1:49374/api/session");
    assert_eq!(
        calls[0]
            .2
            .as_ref()
            .expect("the create carries the credential")
            .header_value()
            .as_str(),
        "Basic b3BlbmNvZGU6czNjcmV0"
    );
}
