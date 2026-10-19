//! Shared HTTP plumbing for the remote providers.
//!
//! Written once and reused by every adapter:
//!
//! - [`HttpTransport`] — the injectable transport seam (production:
//!   [`ReqwestTransport`]; tests: an in-memory mock).
//! - [`RemoteHttpClient`] — the middleware stack: auth resolution
//!   ([`crate::provider::remote::auth`]), exponential-backoff retry
//!   ([`crate::provider::remote::retry`]), per-request tower timeout
//!   ([`crate::provider::remote::timeout`]), and unified status mapping
//!   ([`crate::provider::remote::error_map`]).
//! - [`ProviderInner`] — the state every adapter embeds (provider name,
//!   resolved headers, auth strategy, effective budget, JSON POST helper).
//! - [`RemoteProviderConfig`] — the typed config every adapter shares.
//! - SSE/extra-param helpers used by the adapters.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::provider_trait::{BoxFuture, CompletionStream};
use crate::provider::request::{CompletionRequest, Role};
use crate::provider::secret_resolver::SecretResolver;

use super::auth::{AuthStrategy, resolve_auth};
use super::error_map::{StatusOverrides, map_status_with};
use super::retry::RetryPolicy;
use super::timeout::send_with_timeout;

/// Default per-request budget when neither the request nor the config sets one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// `Content-Type` value every adapter sends.
pub const CONTENT_TYPE_JSON: &str = "application/json";

/// Body keys a provider's `extra_params` may never overwrite.
pub const PROTECTED_BODY_KEYS: [&str; 9] = [
    "model",
    "messages",
    "tools",
    "tool_choice",
    "temperature",
    "input",
    "instructions",
    "conversation",
    "stream",
];

/// HTTP verb understood by the transport seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    /// Stable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// One request handed to a [`HttpTransport`].
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub timeout: Duration,
    pub cancellation: CancellationToken,
}

/// Raw transport response (status + full body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// HTTP seam — mock-injected in tests, `reqwest` in production.
pub trait HttpTransport: Send + Sync {
    /// Perform one request. Implementations should honour `timeout` and
    /// `cancellation` best-effort (the client also enforces both).
    fn send(&self, request: HttpRequest)
    -> BoxFuture<'static, Result<HttpResponse, ProviderError>>;
}

/// Production transport over `reqwest`.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// A transport with a fresh client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for ReqwestTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'static, Result<HttpResponse, ProviderError>> {
        let client = self.client.clone();
        Box::pin(async move {
            let method = match request.method {
                HttpMethod::Get => reqwest::Method::GET,
                HttpMethod::Post => reqwest::Method::POST,
            };
            let mut builder = client
                .request(method, &request.url)
                .timeout(request.timeout);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = &request.body {
                builder = builder.json(body);
            }
            let response = tokio::select! {
                biased;
                _ = request.cancellation.cancelled() => {
                    return Err(ProviderError::cancelled("mem-ai"));
                }
                outcome = builder.send() => outcome.map_err(|error| {
                    if error.is_timeout() {
                        ProviderError::timeout("mem-ai", request.timeout)
                    } else {
                        ProviderError::network(error.to_string())
                    }
                })?,
            };
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .map_err(|error| ProviderError::network(error.to_string()))?;
            Ok(HttpResponse { status, body })
        })
    }
}

/// Typed config shared by every remote provider.
///
/// Provider modules layer defaults over [`RemoteProviderConfig::new`] (base
/// URL, model). Empty `base_url` means "provider default or error", decided by
/// the adapter's endpoint resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteProviderConfig {
    /// Model id sent to the provider.
    pub model: String,
    /// Configured base URL (empty → provider default / required).
    pub base_url: String,
    /// `SecretResolver` reference for the API key; `None` → unauthenticated.
    pub api_key_ref: Option<String>,
    /// Request budget ceiling enforced on every call.
    pub timeout: Duration,
    /// Optional `max_tokens` override (adapters apply their own default).
    pub max_tokens: Option<u32>,
    /// Optional sampling temperature override.
    pub temperature: Option<f32>,
    /// Provider-specific extras merged into the request body (safe keys only).
    pub extra_params: BTreeMap<String, Value>,
    /// Force structured requests to require the tool call (OpenAI-family).
    pub force_tool_choice: bool,
}

impl Default for RemoteProviderConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            base_url: String::new(),
            api_key_ref: None,
            timeout: DEFAULT_TIMEOUT,
            max_tokens: None,
            temperature: None,
            extra_params: BTreeMap::new(),
            force_tool_choice: true,
        }
    }
}

impl RemoteProviderConfig {
    /// A config targeting `model` with every other field defaulted.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    /// Set the base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the secret reference used to authenticate.
    pub fn with_api_key_ref(mut self, reference: impl Into<String>) -> Self {
        self.api_key_ref = Some(reference.into());
        self
    }

    /// Set the request budget ceiling.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Validate that the config is usable by the adapter.
    ///
    /// `require_base_url` is false for providers whose endpoint has a default.
    pub fn validate(&self, provider: &str, require_base_url: bool) -> Result<(), ProviderError> {
        if require_base_url && self.base_url.trim().is_empty() {
            return Err(ProviderError::config(format!(
                "{provider} requires a configured base URL"
            ))
            .with_provider(provider));
        }
        if self.timeout.is_zero() {
            return Err(
                ProviderError::config(format!("{provider} timeout must be non-zero"))
                    .with_provider(provider),
            );
        }
        Ok(())
    }
}

/// One outbound request before auth injection.
#[derive(Debug, Clone)]
pub struct OutboundRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub timeout: Option<Duration>,
    pub cancellation: CancellationToken,
    pub auth: AuthStrategy,
}

impl OutboundRequest {
    /// A POST to `url` with the given headers/auth.
    pub fn post(
        url: impl Into<String>,
        headers: BTreeMap<String, String>,
        auth: AuthStrategy,
    ) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers,
            body: None,
            timeout: None,
            cancellation: CancellationToken::new(),
            auth,
        }
    }

    /// Attach the JSON body.
    pub fn with_body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    /// Set the per-request budget.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Attach the cancellation token.
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }
}

/// The shared middleware stack over an injected transport.
#[derive(Clone)]
pub struct RemoteHttpClient {
    provider: String,
    transport: Arc<dyn HttpTransport>,
    resolver: Arc<dyn SecretResolver>,
    retry: RetryPolicy,
    default_timeout: Duration,
    overrides: StatusOverrides,
    entropy: Arc<AtomicU64>,
}

impl RemoteHttpClient {
    /// A client with the default retry policy and timeout.
    pub fn new(
        provider: impl Into<String>,
        transport: Arc<dyn HttpTransport>,
        resolver: Arc<dyn SecretResolver>,
    ) -> Self {
        Self {
            provider: provider.into(),
            transport,
            resolver,
            retry: RetryPolicy::default(),
            default_timeout: DEFAULT_TIMEOUT,
            overrides: StatusOverrides::new(),
            entropy: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Replace the retry policy.
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Replace the default budget.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Add provider-specific status overrides.
    pub fn with_status_overrides(mut self, overrides: StatusOverrides) -> Self {
        self.overrides = overrides;
        self
    }

    /// The provider label attached to every error.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The retry policy in force.
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry
    }

    /// Resolve auth, then execute `request` under retry + timeout.
    pub async fn send(&self, request: OutboundRequest) -> Result<HttpResponse, ProviderError> {
        let budget = request.timeout.unwrap_or(self.default_timeout);
        if request.cancellation.is_cancelled() {
            return Err(ProviderError::cancelled(&self.provider));
        }

        let mut headers = request.headers.clone();
        if let Some((name, value)) = resolve_auth(&self.resolver, &request.auth).await? {
            // The resolved secret lives only here, for the duration of the call.
            headers.insert(name, value);
        }

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let http_request = HttpRequest {
                method: request.method,
                url: request.url.clone(),
                headers: headers.clone(),
                body: request.body.clone(),
                timeout: budget,
                cancellation: request.cancellation.clone(),
            };

            let error = match send_with_timeout(
                self.transport.clone(),
                http_request,
                budget,
                &self.provider,
            )
            .await
            {
                Ok(response) if (200..300).contains(&response.status) => return Ok(response),
                Ok(response) => map_status_with(
                    &self.provider,
                    response.status,
                    &response.body,
                    &self.overrides,
                ),
                Err(error) => error,
            };

            if error.is_retryable() && attempt < self.retry.max_attempts {
                let delay = self.retry.delay(attempt, self.next_entropy());
                tokio::select! {
                    biased;
                    _ = request.cancellation.cancelled() => {
                        return Err(ProviderError::cancelled(&self.provider));
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                continue;
            }
            return Err(error.with_provider(&self.provider));
        }
    }

    fn next_entropy(&self) -> u64 {
        let counter = self.entropy.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        counter.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos
    }
}

/// State shared by every remote adapter.
#[derive(Clone)]
pub struct ProviderInner {
    provider: String,
    config: RemoteProviderConfig,
    client: Arc<RemoteHttpClient>,
    headers: BTreeMap<String, String>,
    auth: AuthStrategy,
}

impl ProviderInner {
    /// Build the inner state for `provider`.
    pub fn new(
        provider: impl Into<String>,
        config: RemoteProviderConfig,
        client: Arc<RemoteHttpClient>,
        headers: BTreeMap<String, String>,
        auth: AuthStrategy,
    ) -> Self {
        Self {
            provider: provider.into(),
            config,
            client,
            headers,
            auth,
        }
    }

    /// The provider label.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The active config.
    pub fn config(&self) -> &RemoteProviderConfig {
        &self.config
    }

    /// The shared HTTP client.
    pub fn client(&self) -> &Arc<RemoteHttpClient> {
        &self.client
    }

    /// The static (non-auth) request headers.
    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    /// The effective budget for a request: the request's budget clamped to the
    /// config ceiling, or the ceiling when the request sets none.
    pub fn budget(&self, request_timeout: Option<Duration>) -> Duration {
        match request_timeout {
            Some(budget) => budget.min(self.config.timeout),
            None => self.config.timeout,
        }
    }

    /// POST `body` to `url` and return the raw response body.
    pub async fn post_raw(
        &self,
        url: &str,
        body: Value,
        timeout: Option<Duration>,
        cancellation: &CancellationToken,
    ) -> Result<String, ProviderError> {
        let request =
            OutboundRequest::post(url.to_owned(), self.headers.clone(), self.auth.clone())
                .with_body(body)
                .with_timeout(self.budget(timeout))
                .with_cancellation(cancellation.clone());
        let response = self.client.send(request).await?;
        Ok(response.body)
    }

    /// POST `body` to `url` and parse the response body as JSON.
    pub async fn post_json(
        &self,
        url: &str,
        body: Value,
        timeout: Option<Duration>,
        cancellation: &CancellationToken,
    ) -> Result<Value, ProviderError> {
        let raw = self.post_raw(url, body, timeout, cancellation).await?;
        decode_json(&self.provider, &raw)
    }
}

/// Parse a JSON response body, mapping malformed bodies to `Protocol` without
/// echoing the payload.
pub fn decode_json(provider: &str, body: &str) -> Result<Value, ProviderError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            "provider returned an empty response body",
        )
        .with_provider(provider));
    }
    serde_json::from_str(trimmed).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::Protocol,
            "provider returned a non-JSON response body: <redacted>",
        )
        .with_provider(provider)
    })
}

/// The system prompt for `request` (explicit field, else the first system
/// message).
pub fn effective_system_prompt(request: &CompletionRequest) -> Option<String> {
    request.system.clone().or_else(|| {
        request
            .messages
            .iter()
            .find(|message| message.role == Role::System)
            .map(|message| message.content.clone())
    })
}

/// The conversation messages (everything that is not a system message).
pub fn conversation(
    request: &CompletionRequest,
) -> impl Iterator<Item = &crate::provider::request::ChatMessage> {
    request
        .messages
        .iter()
        .filter(|message| message.role != Role::System)
}

/// Parse an SSE body into its `data:` payloads, dropping the `[DONE]`
/// sentinel. Handles `\n` and `\r\n` line endings and multi-line data.
pub fn sse_data_events(body: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut current = String::new();
    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            if !current.is_empty() {
                events.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let data = rest.strip_prefix(' ').unwrap_or(rest);
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(data);
        }
    }
    if !current.is_empty() {
        events.push(current);
    }
    events.retain(|event| event != "[DONE]");
    events
}

/// Merge `extra` into `body`, skipping keys that would clobber core fields.
pub fn apply_safe_extra_params(body: &mut Value, extra: &BTreeMap<String, Value>) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    for (key, value) in extra {
        if PROTECTED_BODY_KEYS.contains(&key.as_str()) {
            continue;
        }
        object.insert(key.clone(), value.clone());
    }
}

/// Build a [`CompletionStream`] from a provider POST whose body is SSE.
///
/// `extract` maps one SSE `data:` payload to a text delta (`None` for
/// non-text events). The stream yields `{ "text": ... }` chunks followed by
/// `{ "done": true }`, or a single `Err` on failure.
pub fn completion_from_sse(
    inner: ProviderInner,
    url: String,
    body: Value,
    cancellation: CancellationToken,
    timeout: Option<Duration>,
    extract: fn(&str) -> Option<String>,
) -> CompletionStream {
    Box::pin(
        futures::stream::once(async move {
            match inner.post_raw(&url, body, timeout, &cancellation).await {
                Ok(text) => {
                    let mut chunks: Vec<Result<Value, ProviderError>> = Vec::new();
                    for event in sse_data_events(&text) {
                        if let Some(delta) = extract(&event)
                            && !delta.is_empty()
                        {
                            chunks.push(Ok(serde_json::json!({ "text": delta })));
                        }
                    }
                    chunks.push(Ok(serde_json::json!({ "done": true })));
                    chunks
                }
                Err(error) => vec![Err(error)],
            }
        })
        .flat_map(futures::stream::iter),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::provider::secret_resolver::StaticSecretResolver;

    /// Scripted transport: pops one response (or hangs) per call.
    enum Script {
        Reply(u16, String),
        Hang,
    }

    struct MockTransport {
        calls: Mutex<usize>,
        scripts: Mutex<Vec<Script>>,
        captures: Mutex<Vec<HttpRequest>>,
    }

    impl MockTransport {
        fn new(scripts: Vec<Script>) -> Self {
            Self {
                calls: Mutex::new(0),
                scripts: Mutex::new(scripts),
                captures: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }

        fn captured(&self) -> Vec<HttpRequest> {
            self.captures.lock().unwrap().clone()
        }
    }

    impl HttpTransport for MockTransport {
        fn send(
            &self,
            request: HttpRequest,
        ) -> BoxFuture<'static, Result<HttpResponse, ProviderError>> {
            *self.calls.lock().unwrap() += 1;
            self.captures.lock().unwrap().push(request);
            let script = {
                let mut scripts = self.scripts.lock().unwrap();
                if scripts.is_empty() {
                    Script::Reply(200, "{}".to_owned())
                } else {
                    scripts.remove(0)
                }
            };
            Box::pin(async move {
                match script {
                    Script::Reply(status, body) => Ok(HttpResponse { status, body }),
                    Script::Hang => std::future::pending().await,
                }
            })
        }
    }

    fn client(transport: Arc<dyn HttpTransport>) -> RemoteHttpClient {
        RemoteHttpClient::new(
            "test-provider",
            transport,
            Arc::new(StaticSecretResolver::new().with_secret("env:KEY", "sk-secret")),
        )
        .with_retry(RetryPolicy::new(3, Duration::ZERO, Duration::ZERO))
    }

    fn outbound() -> OutboundRequest {
        OutboundRequest::post(
            "https://example.test/v1",
            BTreeMap::new(),
            AuthStrategy::bearer("env:KEY"),
        )
    }

    #[tokio::test]
    async fn test_retry_on_429() {
        let transport = Arc::new(MockTransport::new(vec![
            Script::Reply(429, r#"{"error":{"message":"slow down"}}"#.to_owned()),
            Script::Reply(429, String::new()),
            Script::Reply(200, "{\"ok\":true}".to_owned()),
        ]));
        let response = client(transport.clone())
            .send(outbound())
            .await
            .expect("third attempt succeeds");
        assert_eq!(response.status, 200);
        assert_eq!(transport.calls(), 3, "two retries then success");
    }

    #[tokio::test]
    async fn test_retry_on_5xx() {
        let transport = Arc::new(MockTransport::new(vec![
            Script::Reply(503, String::new()),
            Script::Reply(200, "{\"ok\":true}".to_owned()),
        ]));
        let response = client(transport.clone())
            .send(outbound())
            .await
            .expect("second attempt succeeds");
        assert_eq!(response.status, 200);
        assert_eq!(transport.calls(), 2);
    }

    #[tokio::test]
    async fn test_retry_stops_after_max_attempts() {
        let transport = Arc::new(MockTransport::new(vec![
            Script::Reply(500, String::new()),
            Script::Reply(500, String::new()),
            Script::Reply(500, String::new()),
            Script::Reply(200, "{}".to_owned()),
        ]));
        let error = client(transport.clone())
            .send(outbound())
            .await
            .expect_err("gives up after the attempt ceiling");
        assert_eq!(error.status, Some(500));
        assert_eq!(transport.calls(), 3, "never exceeds max_attempts");
    }

    #[tokio::test]
    async fn test_no_retry_on_auth_error() {
        let transport = Arc::new(MockTransport::new(vec![
            Script::Reply(401, String::new()),
            Script::Reply(200, "{}".to_owned()),
        ]));
        let error = client(transport.clone())
            .send(outbound())
            .await
            .expect_err("401 is terminal");
        assert_eq!(error.kind, ProviderErrorKind::Auth);
        assert_eq!(transport.calls(), 1, "auth errors are not retried");
    }

    #[tokio::test]
    async fn test_timeout_maps_to_timeout_kind() {
        let transport = Arc::new(MockTransport::new(vec![
            Script::Hang,
            Script::Hang,
            Script::Hang,
        ]));
        let error = client(transport)
            .send(outbound().with_timeout(Duration::from_millis(20)))
            .await
            .expect_err("hanging transport times out");
        assert_eq!(error.kind, ProviderErrorKind::Timeout);
    }

    #[tokio::test]
    async fn test_error_mapping_unified() {
        for (status, kind) in [
            (400, ProviderErrorKind::HttpStatus),
            (401, ProviderErrorKind::Auth),
            (403, ProviderErrorKind::Auth),
            (404, ProviderErrorKind::HttpStatus),
            (429, ProviderErrorKind::RateLimited),
            (529, ProviderErrorKind::HttpStatus),
        ] {
            let transport = Arc::new(MockTransport::new(vec![Script::Reply(
                status,
                String::new(),
            )]));
            // Isolate status classification from retry: a single attempt.
            let client = RemoteHttpClient::new(
                "test-provider",
                transport,
                Arc::new(StaticSecretResolver::new()),
            )
            .with_retry(RetryPolicy::new(1, Duration::ZERO, Duration::ZERO));
            let error = client.send(outbound()).await.expect_err("non-2xx");
            assert_eq!(error.kind, kind, "status {status}");
            assert_eq!(error.provider, "test-provider");
            assert_eq!(error.status, Some(status));
        }
    }

    #[tokio::test]
    async fn auth_header_is_injected_from_resolver() {
        let transport = Arc::new(MockTransport::new(vec![Script::Reply(
            200,
            "{}".to_owned(),
        )]));
        client(transport.clone())
            .send(outbound())
            .await
            .expect("ok");
        let captured = transport.captured();
        assert_eq!(
            captured[0].headers.get("authorization").map(String::as_str),
            Some("Bearer sk-secret")
        );
    }

    #[test]
    fn sse_events_skip_done_and_join_multiline_data() {
        let body = "event: message\ndata: {\"a\":1}\n\ndata: [DONE]\n\n";
        assert_eq!(sse_data_events(body), vec!["{\"a\":1}".to_owned()]);

        let body = "data: line1\ndata: line2\n\n";
        assert_eq!(sse_data_events(body), vec!["line1\nline2".to_owned()]);
    }

    #[test]
    fn safe_extra_params_never_clobber_protected_keys() {
        let mut body = serde_json::json!({ "model": "m", "messages": [] });
        let mut extra = BTreeMap::new();
        extra.insert(
            "thinking".to_owned(),
            serde_json::json!({ "type": "adaptive" }),
        );
        extra.insert("model".to_owned(), serde_json::json!("nope"));
        apply_safe_extra_params(&mut body, &extra);
        assert_eq!(body["thinking"], serde_json::json!({ "type": "adaptive" }));
        assert_eq!(body["model"], "m");
    }
}
