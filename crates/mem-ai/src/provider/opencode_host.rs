//! `OpenCodeHostProvider` — structured output and streaming completion via the
//! OpenCode HTTP host's STATELESS generate route.
//!
//! The provider makes exactly ONE request per attempt: `POST
//! {base}/experimental/generate/text` with `{"prompt": …}` and, when a model is
//! resolved, `{"model": {"id", "providerID", "variant"?}}`. The reply is
//! `{"data":{"text": …}}`; a structured answer is that TEXT recovered by the
//! shared three-shape parser ([`parse_structured_lenient`]) and the schema
//! travels inside the prompt text, because the route declares no `format`
//! field. No session is created, named, deleted or tracked, so the host's
//! session list is never touched.
//!
//! The wire is the DOCUMENTED one (`docs/evidence/issue-88-host-wire-contract-20261101/`,
//! `.tmp/scratch/live-openapi.json`): `requestBody.additionalProperties:false`
//! with `prompt` required and `model` nullable, no query parameters. Every
//! non-2xx body is decoded into a closed class token (`wire_reason`) plus a
//! bounded `detail` built from the declared error fields — never from the
//! host's `message`, never from a reply.
//!
//! Transport is an injected [`HostTransport`] seam — the production
//! implementation is [`ReqwestHostTransport`]; tests inject an in-memory mock.

use mem_core::sync::{read_or_recover, write_or_recover};
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::provider::error::{ProviderError, ProviderErrorKind, wire_reason};
use crate::provider::host_auth::HostAuthorization;
use crate::provider::provider_trait::{
    AiProvider, AiProviderStructured, BoxFuture, CompletionStream, sealed,
};
use crate::provider::remote::structured::parse_structured_lenient;
use crate::provider::request::{CompletionRequest, EmbedRequest, ModelInfo};

/// Stable provider name.
pub const PROVIDER_NAME: &str = "opencode-host";

/// The host's stateless generate route, relative to the resolved `/api` base.
pub const GENERATE_TEXT_PATH: &str = "/experimental/generate/text";

/// Hard ceiling for a single internal structured-output prompt (upstream 90s).
pub const STRUCTURED_OUTPUT_TIMEOUT_MS: u64 = 90_000;

/// Longest `detail` value rendered into a decoded failure (operator payload,
/// never a decision input).
const MAX_DETAIL_CHARS: usize = 120;

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
    /// Optional reasoning variant, sent as `Model.Ref.variant`.
    pub variant: Option<String>,
    /// Basic-auth credential for the host (`{origin}/api` requires it).
    pub authorization: Option<HostAuthorization>,
    /// Request budget enforced on every host call.
    pub timeout: Duration,
}

impl Default for OpenCodeHostConfig {
    fn default() -> Self {
        Self {
            provider_id: "opencode".to_owned(),
            model: "inherit".to_owned(),
            base_url: String::new(),
            variant: None,
            authorization: None,
            timeout: Duration::from_millis(STRUCTURED_OUTPUT_TIMEOUT_MS),
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
}

impl HostMethod {
    fn reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
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

/// Shared, cheaply-cloneable provider state (so `complete` can own a `'static`
/// stream).
#[derive(Clone)]
struct HostState {
    config: Arc<OpenCodeHostConfig>,
    transport: Arc<dyn HostTransport>,
    connected: Arc<RwLock<HashSet<String>>>,
    refresher: Arc<RwLock<Option<ConnectedRefresher>>>,
}

impl HostState {
    fn base(&self) -> String {
        self.config.base_url.trim_end_matches('/').to_owned()
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

    /// The generate request body.
    ///
    /// `additionalProperties:false` is a runtime contract here, so this is
    /// exactly `prompt` plus `model` when (and only when) a real model is
    /// resolved: an empty model, the `inherit` sentinel, or a missing provider
    /// id all omit the key rather than sending a value the host cannot route.
    fn generate_body(&self, prompt: String) -> Value {
        let mut body = json!({ "prompt": prompt });
        let model = self.config.model.trim();
        let provider_id = self.config.provider_id.trim();
        if !(model.is_empty() || model == "inherit" || provider_id.is_empty()) {
            body["model"] = self.model_ref();
        }
        body
    }

    /// One stateless generate call: the entire attempt.
    async fn generate(
        &self,
        request: CompletionRequest,
        structured: bool,
    ) -> Result<PromptOutcome, ProviderError> {
        let cancellation = request.cancellation.clone();
        let budget = self.effective_budget(request.timeout);
        let url = format!("{}{GENERATE_TEXT_PATH}", self.base());
        let response = self
            .transport_send(
                HostMethod::Post,
                url,
                Some(self.generate_body(prompt_text(&request))),
                &cancellation,
                budget,
            )
            .await?;
        decode_generate_reply(&response, structured)
    }

    /// Run one completion and flatten it into value chunks.
    async fn run_chunks(&self, request: CompletionRequest) -> Vec<Result<Value, ProviderError>> {
        let structured = request.json_schema.is_some();
        match self.generate(request, structured).await {
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
}

/// The bytes `prompt_text` spends between the folded system side and the user
/// text. `build_bounded_summary_prompt` charges exactly these bytes (issue 111):
/// one literal, two spenders, or the capture request overshoots its budget.
pub const SYSTEM_USER_SEPARATOR: &str = "\n\n";

/// The prompt text the route takes: `{"prompt": string}` carries no separate
/// system field, so the system side is folded in rather than sent separately.
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
    format!("{system}{SYSTEM_USER_SEPARATOR}{user_text}")
}

/// Parsed generate outcome.
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
#[derive(serde::Deserialize)]
struct GenerateEnvelope {
    data: GenerateData,
}

#[derive(serde::Deserialize)]
struct GenerateData {
    text: String,
}

/// Decode one generate reply: status, then body, then envelope.
fn decode_generate_reply(
    response: &HostResponse,
    structured_requested: bool,
) -> Result<PromptOutcome, ProviderError> {
    if !(200..300).contains(&response.status) {
        // Never echo the response body: it is operator payload only.
        return Err(decode_generate_failure(response.status, &response.body));
    }
    if response.body.trim().is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            format!("POST {GENERATE_TEXT_PATH} returned an empty response body"),
        )
        .with_provider(PROVIDER_NAME));
    }
    // serde's message is dropped deliberately: it is unstable and describes
    // the input (the response body).
    let value: Value = serde_json::from_str(&response.body).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::Protocol,
            format!("POST {GENERATE_TEXT_PATH} returned a non-JSON body: <redacted response body>"),
        )
        .with_provider(PROVIDER_NAME)
    })?;
    parse_generate_response(&value, structured_requested)
}

/// Decode the documented reply body.
///
/// A reply that is not the `{"data":{"text"}}` ENVELOPE is a wire-shape
/// mismatch ([`ProviderErrorKind::WireShape`]); a text that carries no
/// structured value is the model's own miss and is named
/// [`ProviderErrorKind::ModelOutput`] by the shared lenient parser.
fn parse_generate_response(
    value: &Value,
    structured_requested: bool,
) -> Result<PromptOutcome, ProviderError> {
    let envelope: GenerateEnvelope = serde_json::from_value(value.clone()).map_err(|_| {
        ProviderError::new(
            ProviderErrorKind::WireShape,
            "the generate reply carried no {\"data\":{\"text\"}} envelope",
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
    let structured = parse_structured_lenient(PROVIDER_NAME, &text)?;
    Ok(PromptOutcome {
        structured: Some(structured),
        text: Some(text),
    })
}

/// Decode one non-2xx generate response into a named failure class.
///
/// The class token is a closed set ([`wire_reason`]); the message is fixed and
/// generic per status (`POST <path> failed (<status>)`), so neither the host's
/// `message` nor the body can reach a log line or a decision. `detail` is
/// built here from the DECLARED error fields only, each bounded to
/// [`MAX_DETAIL_CHARS`].
pub fn decode_generate_failure(status: u16, body: &str) -> ProviderError {
    let message = format!("POST {GENERATE_TEXT_PATH} failed ({status})");
    let parsed = serde_json::from_str::<Value>(body)
        .ok()
        .filter(Value::is_object);
    let tagged = parsed.as_ref().and_then(|value| {
        value
            .get("_tag")
            .and_then(Value::as_str)
            .map(|tag| (tag, value))
    });
    let Some((tag, value)) = tagged else {
        // No `_tag`: on a 404 the route itself is the missing thing.
        if status == 404 {
            return ProviderError::new(ProviderErrorKind::Config, message)
                .with_provider(PROVIDER_NAME)
                .with_status(status)
                .with_wire_reason(wire_reason::ROUTE_MOVED)
                .with_detail(format!("route={GENERATE_TEXT_PATH}"));
        }
        return ProviderError::from_status(PROVIDER_NAME, status, message);
    };
    let class = tag_class(tag);

    if class.starts_with("InvalidRequest") {
        let detail = detail_value(value, "field").or_else(|| detail_value(value, "kind"));
        return wire_failure(
            ProviderErrorKind::Config,
            wire_reason::INVALID_REQUEST,
            status,
            message,
            detail,
        );
    }
    if class.starts_with("Unauthorized") {
        return wire_failure(
            ProviderErrorKind::Auth,
            wire_reason::UNAUTHORIZED,
            status,
            message,
            None,
        );
    }
    if class.starts_with("ProviderNotFound") {
        return wire_failure(
            ProviderErrorKind::Config,
            wire_reason::PROVIDER_NOT_FOUND,
            status,
            message,
            detail_pairs(value, &["providerID"]),
        );
    }
    if class.starts_with("ModelNotFound") {
        return wire_failure(
            ProviderErrorKind::Config,
            wire_reason::MODEL_NOT_FOUND,
            status,
            message,
            detail_pairs(value, &["providerID", "modelID"]),
        );
    }
    if class.starts_with("LocationNotFound") {
        return wire_failure(
            ProviderErrorKind::Config,
            wire_reason::LOCATION_NOT_FOUND,
            status,
            message,
            detail_value(value, "location").map(|location| format!("location={location}")),
        );
    }
    if class.starts_with("UnsupportedModel") {
        return wire_failure(
            ProviderErrorKind::Config,
            wire_reason::MODEL_UNSUPPORTED,
            status,
            message,
            detail_pairs(value, &["providerID", "modelID", "capability"]),
        );
    }
    if class.starts_with("ProviderRequest") {
        return decode_provider_request_failure(status, message, value);
    }
    if class.starts_with("ServiceUnavailable") {
        return wire_failure(
            ProviderErrorKind::HttpStatus,
            wire_reason::SERVICE_UNAVAILABLE,
            status,
            message,
            detail_value(value, "service").map(|service| format!("service={service}")),
        );
    }
    // A decodable but unknown tag: never silently a route-move.
    wire_failure(
        kind_from_status(status),
        wire_reason::UNRECOGNISED,
        status,
        message,
        Some(format!("tag={}", bounded_detail(tag))),
    )
}

/// `ProviderRequestError` splits into a retryable and a terminal class on the
/// declared `reason`; absent/unknown reasons stay retryable (a conformant host
/// cannot omit a required field, and the ladder bounds the cost at five).
fn decode_provider_request_failure(status: u16, message: String, value: &Value) -> ProviderError {
    let reason = value.get("reason").and_then(Value::as_str);
    let terminal = matches!(
        reason,
        Some("authentication") | Some("content_filter") | Some("invalid_request")
    );
    let token = if terminal {
        wire_reason::PROVIDER_REQUEST_TERMINAL
    } else {
        wire_reason::PROVIDER_REQUEST_RETRYABLE
    };
    let mut detail = format!("reason={}", reason.unwrap_or("<absent>"));
    let retry_after = value.get("retryAfterMs").and_then(Value::as_u64);
    if !terminal {
        if let Some(host_status) = value.get("status").and_then(Value::as_u64) {
            detail.push_str(&format!(";status={host_status}"));
        }
        if let Some(ms) = retry_after {
            detail.push_str(&format!(";retryAfterMs={ms}"));
        }
    }
    let error = wire_failure(
        ProviderErrorKind::HttpStatus,
        token,
        status,
        message,
        Some(detail),
    );
    match retry_after {
        Some(ms) => error.with_retry_after(ms),
        None => error,
    }
}

/// Strip one optional `ErrorEncoded`/`Encoded` suffix so both spellings of a
/// declared tag classify the same way.
fn tag_class(tag: &str) -> &str {
    let tag = tag.trim();
    tag.strip_suffix("ErrorEncoded")
        .or_else(|| tag.strip_suffix("Encoded"))
        .unwrap_or(tag)
}

/// Build one decoded failure: fixed message, closed token, bounded detail.
fn wire_failure(
    kind: ProviderErrorKind,
    token: &'static str,
    status: u16,
    message: String,
    detail: Option<String>,
) -> ProviderError {
    let error = ProviderError::new(kind, message)
        .with_provider(PROVIDER_NAME)
        .with_status(status)
        .with_wire_reason(token);
    match detail {
        Some(detail) => error.with_detail(detail),
        None => error,
    }
}

fn kind_from_status(status: u16) -> ProviderErrorKind {
    match status {
        401 | 403 => ProviderErrorKind::Auth,
        429 => ProviderErrorKind::RateLimited,
        _ => ProviderErrorKind::HttpStatus,
    }
}

/// `key=value` for one declared string field.
fn detail_value(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(bounded_detail)
}

/// `key=value;key=value` for the declared fields that are present.
fn detail_pairs(value: &Value, keys: &[&str]) -> Option<String> {
    let pairs: Vec<String> = keys
        .iter()
        .filter_map(|key| detail_value(value, key).map(|detail| format!("{key}={detail}")))
        .collect();
    if pairs.is_empty() {
        None
    } else {
        Some(pairs.join(";"))
    }
}

/// Bound one detail value at a UTF-8 char boundary.
fn bounded_detail(value: &str) -> String {
    if value.chars().count() <= MAX_DETAIL_CHARS {
        return value.to_owned();
    }
    value.chars().take(MAX_DETAIL_CHARS).collect()
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
        _json_schema: Value,
    ) -> Result<Value, ProviderError> {
        // The route has no `json_schema` enforcement: the schema is already in
        // the prompt text, and the reply is text, not a schema-bound object.
        let outcome = self.state.generate(request, true).await?;
        outcome.structured.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::ModelOutput,
                "the generate reply carried no structured value",
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
    fn token_estimate_is_ceil_over_four() {
        assert_eq!(estimate_token_count(""), 0);
        assert_eq!(estimate_token_count("a"), 1);
        assert_eq!(estimate_token_count("abcd"), 1);
        assert_eq!(estimate_token_count("abcde"), 2);
    }

    fn decode(status: u16, tag: &str, extra: &str) -> ProviderError {
        let body = format!("{{\"_tag\":\"{tag}\",\"message\":\"x\"{extra}}}");
        decode_generate_failure(status, &body)
    }

    /// Every declared `_tag` — both spellings — maps to its closed token, and
    /// every declared `ProviderRequestError.reason` to one of the two 502
    /// tokens. Replacing any case with the default reds it.
    #[test]
    fn test_wire_error_table_covers_every_declared_generate_failure() {
        let simple: [(&str, u16, &'static str, ProviderErrorKind); 7] = [
            (
                "InvalidRequest",
                400,
                wire_reason::INVALID_REQUEST,
                ProviderErrorKind::Config,
            ),
            (
                "Unauthorized",
                401,
                wire_reason::UNAUTHORIZED,
                ProviderErrorKind::Auth,
            ),
            (
                "ProviderNotFound",
                404,
                wire_reason::PROVIDER_NOT_FOUND,
                ProviderErrorKind::Config,
            ),
            (
                "ModelNotFound",
                404,
                wire_reason::MODEL_NOT_FOUND,
                ProviderErrorKind::Config,
            ),
            (
                "LocationNotFound",
                404,
                wire_reason::LOCATION_NOT_FOUND,
                ProviderErrorKind::Config,
            ),
            (
                "UnsupportedModel",
                422,
                wire_reason::MODEL_UNSUPPORTED,
                ProviderErrorKind::Config,
            ),
            (
                "ServiceUnavailable",
                503,
                wire_reason::SERVICE_UNAVAILABLE,
                ProviderErrorKind::HttpStatus,
            ),
        ];
        // The closed set is the manifest: every token must have a fixture here,
        // so a twelfth token reds this row until it is exercised. A SET, not a
        // list: the tag loop visits each class twice.
        let mut exercised: std::collections::BTreeSet<&'static str> =
            std::collections::BTreeSet::new();
        for (class, status, token, kind) in simple {
            for tag in [format!("{class}Error"), format!("{class}ErrorEncoded")] {
                exercised.insert(token);
                let error = decode(status, &tag, "");
                assert_eq!(error.wire_reason, Some(token), "tag {tag}");
                assert_eq!(error.kind, kind, "tag {tag}");
                assert_eq!(error.status, Some(status), "tag {tag}");
                assert_eq!(
                    error.message,
                    format!("POST /experimental/generate/text failed ({status})"),
                    "the message is fixed and generic, tag {tag}"
                );
                assert!(
                    !error.message.contains(&tag),
                    "the class never leaks into the message"
                );
            }
        }

        let retryable_reasons = ["rate_limit", "overloaded", "provider"];
        let terminal_reasons = ["authentication", "content_filter", "invalid_request"];
        for reason in retryable_reasons
            .iter()
            .map(|reason| (reason, wire_reason::PROVIDER_REQUEST_RETRYABLE))
            .chain(
                terminal_reasons
                    .iter()
                    .map(|reason| (reason, wire_reason::PROVIDER_REQUEST_TERMINAL)),
            )
        {
            exercised.insert(reason.1);
            for tag in ["ProviderRequestError", "ProviderRequestErrorEncoded"] {
                let error = decode(502, tag, &format!(",\"reason\":\"{}\"", reason.0));
                assert_eq!(error.wire_reason, Some(reason.1), "reason {}", reason.0);
                assert_eq!(error.kind, ProviderErrorKind::HttpStatus);
                assert_eq!(error.status, Some(502));
                let detail = error.detail.as_deref().expect("a reason detail");
                assert!(detail.contains(&format!("reason={}", reason.0)), "{detail}");
            }
        }

        let route_moved = decode_generate_failure(404, "");
        let unrecognised =
            decode_generate_failure(404, r#"{"_tag":"SomethingNewError","message":"x"}"#);
        exercised.insert(route_moved.wire_reason.expect("route-moved"));
        exercised.insert(unrecognised.wire_reason.expect("unrecognised"));
        let declared: std::collections::BTreeSet<&'static str> =
            wire_reason::ALL.into_iter().collect();
        assert_eq!(
            exercised, declared,
            "every token in wire_reason::ALL needs a fixture in this table"
        );
    }

    #[test]
    fn test_wire_error_404_without_a_tag_is_route_moved() {
        for body in [
            "",
            "<html><body>not found</body></html>",
            "{\"message\":\"x\"}",
        ] {
            let error = decode_generate_failure(404, body);
            assert_eq!(error.wire_reason, Some(wire_reason::ROUTE_MOVED), "{body}");
            assert_eq!(error.kind, ProviderErrorKind::Config);
            assert_eq!(error.status, Some(404));
            assert!(
                error
                    .detail
                    .as_deref()
                    .unwrap_or_default()
                    .contains(GENERATE_TEXT_PATH),
                "the detail names the path"
            );
        }
    }

    #[test]
    fn test_wire_error_unknown_tag_is_unrecognised_not_route_moved() {
        let error = decode_generate_failure(404, r#"{"_tag":"SomethingNewError","message":"x"}"#);
        assert_eq!(error.wire_reason, Some(wire_reason::UNRECOGNISED));
        assert_eq!(error.kind, ProviderErrorKind::HttpStatus);
        assert!(
            error
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("tag=SomethingNewError"),
            "{:?}",
            error.detail
        );
    }
}
