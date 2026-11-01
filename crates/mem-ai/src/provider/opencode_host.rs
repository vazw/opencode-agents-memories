//! `OpenCodeHostProvider` — structured output and streaming completion via the
//! OpenCode HTTP host (v2 session API).
//!
//! Instead of forging provider HTTP requests ourselves (auth, token refresh,
//! routing), we delegate to the running OpenCode server: create a transient
//! least-privilege session, ask it for an answer over the documented
//! `POST …/generate` route, and delete the session so it never pollutes the
//! user's TUI session list.
//!
//! The wire is the DOCUMENTED one (`docs/evidence/issue-88-host-wire-contract-20261101/`),
//! never a captured exchange: create accepts `{"data": Session.Info}` and a bare
//! `{"id": …}`; the prompt body is exactly `{"prompt": …}`; the reply is
//! `{"data":{"text": …}}`. A structured answer is that TEXT parsed by the shared
//! structured-output parser, and the schema travels inside the prompt text
//! because the route declares no `format` field.
//!
//! Transport is an injected [`HostTransport`] seam — the production
//! implementation is [`ReqwestHostTransport`]; tests inject an in-memory mock.
//! Internal capture sessions are tracked so auto-capture never re-captures
//! them, and the client-side step budget aborts runaway structured-output
//! loops.

use mem_core::sync::{lock_or_recover, read_or_recover, write_or_recover};

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::host_auth::HostAuthorization;
use crate::provider::internal_capture::{INTERNAL_CAPTURE_SESSION_TITLE, InternalCaptureSessions};
use crate::provider::provider_trait::{
    AiProvider, AiProviderStructured, BoxFuture, CompletionStream, sealed,
};
use crate::provider::remote::structured::parse_structured;
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};

/// Stable provider name.
pub const PROVIDER_NAME: &str = "opencode-host";

/// Dedicated agent registered via the plugin config hook (step-capped).
pub const STRUCTURED_OUTPUT_AGENT: &str = "opencode-mem-structured";

/// Hard ceiling for a single internal structured-output prompt (upstream 90s).
pub const STRUCTURED_OUTPUT_TIMEOUT_MS: u64 = 90_000;

/// Soft agent step budget: abort client-side after this many `step-start`
/// parts (upstream 2).
pub const STRUCTURED_OUTPUT_MAX_STEPS: u32 = 2;

/// Bound OpenCode schema retries so a bad provider cannot spin forever.
///
/// Retained as the value `OpenCodeHostConfig::retry_count` defaults to. Since
/// the wire wave that count is no longer TRANSMITTED: the documented
/// `POST …/generate` route declares only `prompt`, so no `format`/`retryCount`
/// field exists to carry it (ADR-0029 D-29-6's recorded loss).
pub const STRUCTURED_OUTPUT_DEFAULT_RETRY_COUNT: u32 = 1;

/// Configuration for the host provider.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeHostConfig {
    /// OpenCode provider id (e.g. `"opencode"`, `"github-copilot"`).
    pub provider_id: String,
    /// Model id (`"inherit"` defers to the host's recent model).
    pub model: String,
    /// OpenCode server base URL.
    ///
    /// Defaults to the EMPTY string, which is not dialable: production always
    /// passes a resolved `{origin}/api` (see `mem_bridge::host_endpoint`). A
    /// compiled-in port would silently target whatever listens on `4096`.
    pub base_url: String,
    /// Optional reasoning variant, sent as `Model.Ref.variant` on the create.
    pub variant: Option<String>,
    /// Optional working directory forwarded as a query param.
    pub directory: Option<String>,
    /// Basic-auth credential for the host (`{origin}/api` requires it).
    pub authorization: Option<HostAuthorization>,
    /// Request budget enforced on every host call.
    pub timeout: Duration,
    /// Client-side structured-output step budget. Declarative while
    /// `note_structured_step` has no transport caller (ADR-0027's amendment).
    pub max_steps: u32,
    /// `format: json_schema` retry count, no longer transmitted: the documented
    /// `/generate` route declares no `format` field. Kept as configuration so a
    /// caller's value is not silently repurposed.
    pub retry_count: u32,
}

impl Default for OpenCodeHostConfig {
    fn default() -> Self {
        Self {
            provider_id: "opencode".to_owned(),
            model: "inherit".to_owned(),
            base_url: String::new(),
            variant: None,
            directory: None,
            authorization: None,
            timeout: Duration::from_millis(STRUCTURED_OUTPUT_TIMEOUT_MS),
            max_steps: STRUCTURED_OUTPUT_MAX_STEPS,
            retry_count: STRUCTURED_OUTPUT_DEFAULT_RETRY_COUNT,
        }
    }
}

impl OpenCodeHostConfig {
    /// Validate the config before a provider is built (used by the factory).
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.base_url.trim().is_empty() {
            return Err(
                ProviderError::config("opencode-host base_url must not be empty")
                    .with_provider(PROVIDER_NAME),
            );
        }
        if self.timeout.is_zero() {
            return Err(
                ProviderError::config("opencode-host timeout must be non-zero")
                    .with_provider(PROVIDER_NAME),
            );
        }
        Ok(())
    }
}

/// HTTP verb understood by the host transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostMethod {
    Get,
    Post,
    Delete,
}

impl HostMethod {
    fn reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Delete => reqwest::Method::DELETE,
        }
    }
}

/// One host request handed to a [`HostTransport`].
#[derive(Debug, Clone)]
pub struct HostRequest {
    pub method: HostMethod,
    pub url: String,
    pub body: Option<Value>,
    pub timeout: Duration,
    /// Cancellation signal propagated into the transport.
    pub cancellation: CancellationToken,
    /// Basic-auth credential for the host, when one was resolved.
    ///
    /// The derive is safe: [`HostAuthorization`]'s own `Debug` is redacted, so
    /// this struct can never print the password (nor its base64 header).
    pub authorization: Option<HostAuthorization>,
}

/// Raw host response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostResponse {
    pub status: u16,
    pub body: String,
}

/// HTTP seam for the OpenCode host (mock-injected in tests).
pub trait HostTransport: Send + Sync {
    /// Perform one request. Implementations must honour `timeout` and
    /// `cancellation` best-effort (the provider also enforces both).
    fn send(&self, request: HostRequest)
    -> BoxFuture<'static, Result<HostResponse, ProviderError>>;
}

/// Production transport over `reqwest`.
pub struct ReqwestHostTransport {
    client: reqwest::Client,
}

impl ReqwestHostTransport {
    /// New transport with a fresh client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestHostTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HostTransport for ReqwestHostTransport {
    fn send(
        &self,
        request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        let client = self.client.clone();
        Box::pin(async move {
            let mut builder = client
                .request(request.method.reqwest(), &request.url)
                .timeout(request.timeout);
            if let Some(authorization) = &request.authorization {
                // A zeroizing `String` outlives the builder call; reqwest owns
                // the copied `HeaderValue` for the request only.
                builder = builder.header(
                    reqwest::header::AUTHORIZATION,
                    authorization.header_value().as_str(),
                );
            }
            if let Some(body) = &request.body {
                builder = builder.json(body);
            }
            let response = tokio::select! {
                biased;
                _ = request.cancellation.cancelled() => {
                    return Err(ProviderError::cancelled(PROVIDER_NAME));
                }
                outcome = builder.send() => outcome.map_err(|error| {
                    if error.is_timeout() {
                        ProviderError::timeout(PROVIDER_NAME, request.timeout)
                    } else {
                        ProviderError::network(error.to_string()).with_provider(PROVIDER_NAME)
                    }
                })?,
            };
            let status = response.status().as_u16();
            let body = response.text().await.map_err(|error| {
                ProviderError::network(error.to_string()).with_provider(PROVIDER_NAME)
            })?;
            Ok(HostResponse { status, body })
        })
    }
}

/// Callback used to force one connected-provider refresh before failing.
pub type ConnectedRefresher = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// Result of noting one structured-output step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepNote {
    /// Whether the session is a live internal structured session.
    pub tracked: bool,
    /// Steps seen so far.
    pub steps: u32,
    /// Whether the client should abort (budget exceeded).
    pub should_abort: bool,
}

/// Shared, cheaply-cloneable provider state (so `complete` can own a `'static`
/// stream).
#[derive(Clone)]
struct HostState {
    config: Arc<OpenCodeHostConfig>,
    transport: Arc<dyn HostTransport>,
    connected: Arc<RwLock<HashSet<String>>>,
    refresher: Arc<RwLock<Option<ConnectedRefresher>>>,
    internal: InternalCaptureSessions,
    step_counts: Arc<Mutex<HashMap<String, u32>>>,
}

impl HostState {
    fn base(&self) -> String {
        self.config.base_url.trim_end_matches('/').to_owned()
    }

    fn directory_query(&self) -> String {
        match self.config.directory.as_deref() {
            Some(directory) => format!("?directory={}", encode_query_component(directory)),
            None => String::new(),
        }
    }

    fn effective_budget(&self, request_timeout: Option<Duration>) -> Duration {
        match request_timeout {
            Some(budget) => budget.min(self.config.timeout),
            None => self.config.timeout,
        }
    }

    async fn transport_send(
        &self,
        method: HostMethod,
        url: String,
        body: Option<Value>,
        cancellation: &CancellationToken,
        budget: Duration,
    ) -> Result<HostResponse, ProviderError> {
        if cancellation.is_cancelled() {
            return Err(ProviderError::cancelled(PROVIDER_NAME));
        }
        let request = HostRequest {
            method,
            url,
            body,
            timeout: budget,
            cancellation: cancellation.clone(),
            authorization: self.config.authorization.clone(),
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(ProviderError::cancelled(PROVIDER_NAME)),
            outcome = tokio::time::timeout(budget, self.transport.send(request)) => match outcome {
                Err(_) => Err(ProviderError::timeout(PROVIDER_NAME, budget)),
                Ok(Ok(response)) => Ok(response),
                Ok(Err(error)) => Err(error),
            }
        }
    }

    fn session_create_body(&self) -> Value {
        json!({
            "title": INTERNAL_CAPTURE_SESSION_TITLE,
            // The model and the structured-output agent travel HERE: `model:
            // Model.Ref` and `agent: Agent.ID` are the create body's declared
            // fields, while the prompt route declares only `prompt`.
            "agent": STRUCTURED_OUTPUT_AGENT,
            "model": self.model_ref(),
            // Scope guard — NOT a rename. The declared key is `permissions:
            // Permission.Ruleset` (`Permission.Rule{action, resource, effect}`);
            // this undeclared shape is tolerated in practice (a 2xx was
            // MEASURED for it, and `additionalProperties:false` is
            // documentation, not runtime behaviour). Rewriting it is a rebuild.
            "permission": [
                { "permission": "*", "pattern": "*", "action": "deny" },
                { "permission": "StructuredOutput", "pattern": "*", "action": "allow" }
            ],
            "metadata": { "opencode-mem": { "internal": true, "purpose": "structured-output" } }
        })
    }

    /// The declared `Model.Ref`: `{id, providerID}` plus `variant` when one is
    /// configured.
    fn model_ref(&self) -> Value {
        let mut model = json!({
            "id": self.config.model,
            "providerID": self.config.provider_id,
        });
        if let Some(variant) = &self.config.variant {
            model["variant"] = json!(variant);
        }
        model
    }

    async fn create_session(
        &self,
        cancellation: &CancellationToken,
        budget: Duration,
    ) -> Result<String, ProviderError> {
        let url = format!("{}/session{}", self.base(), self.directory_query());
        let response = self
            .transport_send(
                HostMethod::Post,
                url.clone(),
                Some(self.session_create_body()),
                cancellation,
                budget,
            )
            .await?;
        let value = decode_json(&response, "POST /session")?;
        // serde's message is dropped deliberately: it is unstable and describes
        // the input (the response body). Fixed text only.
        let reply: CreateSessionReply = serde_json::from_value(value).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::WireShape,
                "session.create answered with neither a {\"data\":{\"id\"}} envelope nor a bare {\"id\"}",
            )
            .with_provider(PROVIDER_NAME)
        })?;
        let id = match reply {
            CreateSessionReply::Enveloped { data } => data.id,
            CreateSessionReply::Bare { id } => id,
        };
        if id.trim().is_empty() {
            return Err(ProviderError::new(
                ProviderErrorKind::WireShape,
                "session.create returned a blank session id; cannot generate structured output",
            )
            .with_provider(PROVIDER_NAME));
        }
        Ok(id)
    }

    async fn prompt_session(
        &self,
        session_id: &str,
        request: &CompletionRequest,
        schema: Option<&Value>,
        cancellation: &CancellationToken,
        budget: Duration,
    ) -> Result<PromptOutcome, ProviderError> {
        let url = format!(
            "{}/session/{}/generate{}",
            self.base(),
            encode_query_component(session_id),
            self.directory_query()
        );
        let response = self
            .transport_send(
                HostMethod::Post,
                url,
                Some(prompt_body(request)),
                cancellation,
                budget,
            )
            .await?;
        let value = decode_json(&response, "POST /session/{id}/generate")?;
        parse_prompt_response(&value, schema.is_some())
    }

    async fn delete_session(&self, session_id: &str, cancellation: &CancellationToken) {
        let url = format!(
            "{}/session/{}{}",
            self.base(),
            encode_query_component(session_id),
            self.directory_query()
        );
        // Best-effort: failures are intentionally swallowed.
        let _ = self
            .transport_send(
                HostMethod::Delete,
                url,
                None,
                cancellation,
                self.config.timeout,
            )
            .await;
    }

    /// Full prompt lifecycle with guaranteed cleanup.
    async fn run(
        &self,
        request: CompletionRequest,
        schema: Option<Value>,
    ) -> Result<PromptOutcome, ProviderError> {
        let cancellation = request.cancellation.clone();
        let budget = self.effective_budget(request.timeout);

        let session_id = self.create_session(&cancellation, budget).await?;
        self.internal.track(&session_id);
        // step_counts: diagnostic budget per session; a lost count resets one budget (ADR-0013 D-2 D2-R)
        lock_or_recover(&self.step_counts).insert(session_id.clone(), 0);

        let mut guard = CleanupGuard::new(self.clone(), session_id.clone(), cancellation.clone());
        let result = self
            .prompt_session(
                &session_id,
                &request,
                schema.as_ref(),
                &cancellation,
                budget,
            )
            .await;

        // Normal path: delete synchronously, then disarm the drop guard.
        guard.disarm();
        self.delete_session(&session_id, &cancellation).await;
        self.internal.untrack(&session_id);
        // step_counts: single remove at session end (ADR-0013 D-2 D2-R)
        lock_or_recover(&self.step_counts).remove(&session_id);
        result
    }

    /// Run one completion and flatten it into value chunks.
    async fn run_chunks(&self, request: CompletionRequest) -> Vec<Result<Value, ProviderError>> {
        let schema = request.json_schema.clone();
        match self.run(request, schema).await {
            Ok(outcome) => outcome.into_chunks(),
            Err(error) => vec![Err(error)],
        }
    }

    async fn fetch_config(&self) -> Result<Value, ProviderError> {
        let url = format!("{}/config", self.base());
        let cancellation = CancellationToken::new();
        let response = self
            .transport_send(
                HostMethod::Get,
                url,
                None,
                &cancellation,
                self.config.timeout,
            )
            .await?;
        decode_json(&response, "GET /config")
    }

    fn is_provider_connected(&self, name: &str) -> bool {
        // connected: read-only contains; re-derived from GET /config (ADR-0013 D-2 D2-R)
        read_or_recover(&self.connected).contains(name)
    }

    fn set_connected_providers(&self, providers: Vec<String>) {
        // connected: clear + extend in one write guard, so readers never see a partial set (ADR-0013 D-2 D2-R)
        let mut connected = write_or_recover(&self.connected);
        connected.clear();
        connected.extend(providers);
    }

    fn set_connected_providers_refresher(&self, refresher: ConnectedRefresher) {
        // refresher: one Option store; a stale value only changes who refreshes (ADR-0013 D-2 D2-R)
        *write_or_recover(&self.refresher) = Some(refresher);
    }

    async fn ensure_provider_connected(&self, name: &str) -> bool {
        if self.is_provider_connected(name) {
            return true;
        }
        // refresher: clone out, invoked after the guard is dropped (ADR-0013 D-2 D2-R)
        let refresher = read_or_recover(&self.refresher).clone();
        if let Some(refresher) = refresher {
            // The refresher logs; a failed refresh keeps the previous set.
            let _ = refresher().await;
        }
        self.is_provider_connected(name)
    }

    /// Refresh the connected-provider snapshot from `GET /config`.
    async fn refresh_connected_providers(&self) -> Result<(), ProviderError> {
        let value = self.fetch_config().await?;
        self.set_connected_providers(provider_ids(&value));
        Ok(())
    }

    fn note_structured_step(&self, session_id: &str) -> StepNote {
        if !self.internal.is_tracked(session_id) {
            return StepNote {
                tracked: false,
                steps: 0,
                should_abort: false,
            };
        }
        // step_counts: entry-or-insert then saturating_add(1) (ADR-0013 D-2 D2-R)
        let mut counts = lock_or_recover(&self.step_counts);
        let steps = counts.entry(session_id.to_owned()).or_insert(0);
        *steps = steps.saturating_add(1);
        StepNote {
            tracked: true,
            steps: *steps,
            should_abort: *steps > self.config.max_steps,
        }
    }
}

/// The prompt text the documented route takes: `{"prompt": string}` is its
/// entire body, so the system side is folded in rather than sent separately.
///
/// The system prompt is `request.system` when set, else the first system
/// message; a blank system side contributes nothing (and no separator).
fn prompt_text(request: &CompletionRequest) -> String {
    let system = request
        .system
        .clone()
        .or_else(|| {
            request
                .messages
                .iter()
                .find(|message| message.role == crate::provider::request::Role::System)
                .map(|message| message.content.clone())
        })
        .unwrap_or_default();
    let user_text = request.last_user_message().unwrap_or_default();
    if system.trim().is_empty() {
        return user_text.to_owned();
    }
    format!("{system}\n\n{user_text}")
}

fn prompt_body(request: &CompletionRequest) -> Value {
    json!({ "prompt": prompt_text(request) })
}

/// `POST /api/session` answer, in both shapes the port must accept.
///
/// The document says 2.2.5 answers `{"data": Session.Info}`; a host that does
/// not wrap answers a bare `{"id": …}`. `#[serde(untagged)]` declares the
/// precedence: the ENVELOPE wins, the bare object is the fall-through.
/// Evidence (READ): docs/evidence/issue-88-host-wire-contract-20261101/
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CreateSessionReply {
    Enveloped { data: SessionRef },
    Bare { id: String },
}

#[derive(serde::Deserialize)]
struct SessionRef {
    id: String,
}

/// RAII cleanup for the transient session: if the future is dropped before
/// normal completion (e.g. the stream is dropped), cancel in-flight work and
/// best-effort delete the session.
struct CleanupGuard {
    state: HostState,
    session_id: String,
    cancellation: CancellationToken,
    armed: bool,
}

impl CleanupGuard {
    fn new(state: HostState, session_id: String, cancellation: CancellationToken) -> Self {
        Self {
            state,
            session_id,
            cancellation,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.state.internal.untrack(&self.session_id);
        // Cancel the in-flight request, then delete with a fresh token.
        self.cancellation.cancel();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = self.state.clone();
            let session_id = self.session_id.clone();
            handle.spawn(async move {
                state
                    .delete_session(&session_id, &CancellationToken::new())
                    .await;
            });
        }
    }
}

/// Parsed prompt outcome.
#[derive(Debug, Clone)]
struct PromptOutcome {
    structured: Option<Value>,
    text: Option<String>,
}

impl PromptOutcome {
    fn into_chunks(self) -> Vec<Result<Value, ProviderError>> {
        let mut chunks = Vec::new();
        if let Some(structured) = self.structured {
            chunks.push(Ok(json!({ "json": structured })));
        } else if let Some(text) = self.text {
            chunks.push(Ok(json!({ "text": text })));
        }
        chunks.push(Ok(json!({ "done": true })));
        chunks
    }
}

/// The documented generate reply: `{"data":{"text": string}}`.
///
/// The text is the model's whole answer — the capture JSON, fenced or bare —
/// or prose when the wire carried no schema to answer to.
#[derive(serde::Deserialize)]
struct GenerateEnvelope {
    data: GenerateData,
}

#[derive(serde::Deserialize)]
struct GenerateData {
    text: String,
}

/// Decode the documented reply.
///
/// Two failure branches that must stay distinct: a reply that is not this
/// ENVELOPE is a wire-shape mismatch ([`ProviderErrorKind::WireShape`]), while a
/// text that is not JSON is the model's own miss and stays
/// [`ProviderErrorKind::Protocol`] — the shared parser's classification.
fn parse_prompt_response(
    value: &Value,
    structured_requested: bool,
) -> Result<PromptOutcome, ProviderError> {
    // serde's message is dropped deliberately: it is unstable and describes the
    // input (the response body).
    let envelope: GenerateEnvelope = serde_json::from_value(value.clone()).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::WireShape,
            "session prompt answered without a {\"data\":{\"text\"}} envelope",
        )
        .with_provider(PROVIDER_NAME)
    })?;
    let text = envelope.data.text;
    if !structured_requested {
        return Ok(PromptOutcome {
            structured: None,
            text: Some(text),
        });
    }
    let structured = parse_structured(PROVIDER_NAME, &text)?;
    Ok(PromptOutcome {
        structured: Some(structured),
        text: Some(text),
    })
}

fn decode_json(response: &HostResponse, label: &str) -> Result<Value, ProviderError> {
    if !(200..300).contains(&response.status) {
        // Never echo the response body: it can carry prompts/secrets.
        return Err(ProviderError::from_status(
            PROVIDER_NAME,
            response.status,
            format!("{label} failed ({})", response.status),
        ));
    }
    if response.body.trim().is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            format!("{label} returned an empty response body"),
        )
        .with_provider(PROVIDER_NAME));
    }
    serde_json::from_str(&response.body).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::Protocol,
            format!("{label} returned a non-JSON body: <redacted response body>"),
        )
        .with_provider(PROVIDER_NAME)
    })
}

/// The `GET /api/config` documents (measured 2.2.5 shape: a top-level ARRAY of
/// `Config.Entry`, each with `type: "document"`).
fn config_documents(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// The providers a config document declares (`info.providers`, an OBJECT keyed
/// by provider id) — one iterator entry per (provider id, provider document).
fn config_providers(entry: &Value) -> &serde_json::Map<String, Value> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, Value>> = std::sync::OnceLock::new();
    entry
        .get("info")
        .and_then(|info| info.get("providers"))
        .and_then(Value::as_object)
        .unwrap_or_else(|| EMPTY.get_or_init(serde_json::Map::new))
}

fn provider_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for entry in config_documents(value)
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("document"))
    {
        for provider_id in config_providers(entry).keys() {
            if !ids.contains(provider_id) {
                ids.push(provider_id.clone());
            }
        }
    }
    ids
}

fn models_from_config(value: &Value) -> Vec<ModelInfo> {
    let mut models = Vec::new();
    for entry in config_documents(value)
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("document"))
    {
        for (provider_id, provider) in config_providers(entry) {
            let Some(entries) = provider.get("models").and_then(Value::as_object) else {
                continue;
            };
            for (model_id, model) in entries {
                // Live entries can carry only `{"disabled":…}`: fall back to
                // the object key, and KEEP the entry rather than skipping it.
                let id = model
                    .get("modelID")
                    .and_then(Value::as_str)
                    .unwrap_or(model_id)
                    .to_owned();
                let mut info = ModelInfo::new(id, provider_id.clone());
                info.display_name = model.get("name").and_then(Value::as_str).map(str::to_owned);
                models.push(info);
            }
        }
    }
    models
}

/// Minimal percent-encoding for query components (unreserved + `/`).
fn encode_query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Conservative token estimate (`ceil(chars / 4)`) used when the host exposes
/// no token-count endpoint.
pub fn estimate_token_count(text: &str) -> u32 {
    (text.chars().count() as u32).div_ceil(4)
}

/// Structured output + streaming completion over the OpenCode host.
pub struct OpenCodeHostProvider {
    state: HostState,
}

impl OpenCodeHostProvider {
    /// Build a provider over an injected transport.
    pub fn new(config: OpenCodeHostConfig, transport: Arc<dyn HostTransport>) -> Self {
        Self {
            state: HostState {
                config: Arc::new(config),
                transport,
                connected: Arc::new(RwLock::new(HashSet::new())),
                refresher: Arc::new(RwLock::new(None)),
                internal: InternalCaptureSessions::global().clone(),
                step_counts: Arc::new(Mutex::new(HashMap::new())),
            },
        }
    }

    /// The active config.
    pub fn config(&self) -> &OpenCodeHostConfig {
        &self.state.config
    }

    /// The request budget the provider enforces.
    pub fn timeout(&self) -> Duration {
        self.state.config.timeout
    }

    /// Replace the connected-provider snapshot.
    pub fn set_connected_providers(&self, providers: Vec<String>) {
        self.state.set_connected_providers(providers);
    }

    /// Whether `name` is in the connected-provider snapshot.
    pub fn is_provider_connected(&self, name: &str) -> bool {
        self.state.is_provider_connected(name)
    }

    /// Register (or replace) the refresh callback used on a connectivity miss.
    pub fn set_connected_providers_refresher(&self, refresher: ConnectedRefresher) {
        self.state.set_connected_providers_refresher(refresher);
    }

    /// Refresh-on-miss: await one refresh then re-check (never refreshes
    /// twice for the same call, never throws).
    pub async fn ensure_provider_connected(&self, name: &str) -> bool {
        self.state.ensure_provider_connected(name).await
    }

    /// Refresh the connected-provider snapshot from `GET /config`.
    pub async fn refresh_connected_providers(&self) -> Result<(), ProviderError> {
        self.state.refresh_connected_providers().await
    }

    /// Record a `step-start` on an internal structured-output session.
    pub fn note_structured_step(&self, session_id: &str) -> StepNote {
        self.state.note_structured_step(session_id)
    }

    /// True while an internal structured-output session is live.
    pub fn is_internal_structured_session(&self, session_id: &str) -> bool {
        self.state.internal.is_tracked(session_id)
    }
}

#[async_trait]
impl AiProvider for OpenCodeHostProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        let state = self.state.clone();
        Box::pin(
            futures::stream::once(async move { state.run_chunks(request).await })
                .flat_map(futures::stream::iter),
        )
    }

    async fn embed(&self, _request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Unsupported,
            "opencode-host provider does not expose embeddings",
        )
        .with_provider(PROVIDER_NAME))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let value = self.state.fetch_config().await?;
        Ok(models_from_config(&value))
    }

    async fn count_tokens(&self, _model: &str, text: &str) -> Result<u32, ProviderError> {
        // The host exposes no token-count endpoint; return the conservative
        // estimate the trait contract allows.
        Ok(estimate_token_count(text))
    }
}

impl sealed::Sealed for OpenCodeHostProvider {}

#[async_trait]
impl AiProviderStructured for OpenCodeHostProvider {
    async fn complete_structured(
        &self,
        request: CompletionRequest,
        json_schema: Value,
    ) -> Result<Value, ProviderError> {
        let outcome = self.state.run(request, Some(json_schema)).await?;
        outcome.structured.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Protocol,
                "the generate reply carried no JSON object",
            )
            .with_provider(PROVIDER_NAME)
        })
    }

    async fn supports_structured_output(&self, _model: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_upstream_ceiling() {
        let config = OpenCodeHostConfig::default();
        assert_eq!(config.timeout, Duration::from_millis(90_000));
        assert_eq!(config.max_steps, STRUCTURED_OUTPUT_MAX_STEPS);
        assert_eq!(config.retry_count, STRUCTURED_OUTPUT_DEFAULT_RETRY_COUNT);
        // The default base URL is deliberately NOT dialable: production passes
        // a resolved `{origin}/api`, and a compiled-in port would silently
        // target whatever listens on it.
        assert_eq!(config.base_url, "");
        assert_eq!(
            config.validate().unwrap_err().kind,
            ProviderErrorKind::Config
        );
    }

    #[test]
    fn validate_rejects_empty_base_url_and_zero_timeout() {
        let config = OpenCodeHostConfig {
            base_url: String::new(),
            ..OpenCodeHostConfig::default()
        };
        assert_eq!(
            config.validate().unwrap_err().kind,
            ProviderErrorKind::Config
        );

        let config = OpenCodeHostConfig {
            timeout: Duration::ZERO,
            ..OpenCodeHostConfig::default()
        };
        assert_eq!(
            config.validate().unwrap_err().kind,
            ProviderErrorKind::Config
        );
    }

    #[test]
    fn query_component_encoding_escapes_reserved() {
        assert_eq!(encode_query_component("a/b c?d"), "a%2Fb%20c%3Fd");
        assert_eq!(encode_query_component("ses_1"), "ses_1");
    }

    #[test]
    fn token_estimate_is_ceil_over_four() {
        assert_eq!(estimate_token_count(""), 0);
        assert_eq!(estimate_token_count("a"), 1);
        assert_eq!(estimate_token_count("abcd"), 1);
        assert_eq!(estimate_token_count("abcde"), 2);
    }

    // The deleted row `assistant_error_details_never_echo_prompt_fields` pinned
    // `format_assistant_error`: the old reply's `info.error.name`/`data` were
    // rendered with a whitelist and a prompt fragment was never echoed. The
    // documented reply has no field for an assistant-side error (the assistant
    // -side error name/data are a RECORDED LOSS, ADR-0029's amendment), so the
    // types are gone. Its still-true claim — no response body reaches a message
    // — is inherited by the `test_2_2_5_*` rows' absence assertions (row 4 on
    // the create body, row 10 on the reply text) and by `decode_json`.
}
