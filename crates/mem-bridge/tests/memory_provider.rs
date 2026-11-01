//! The documented manual auto-capture provider: config → registry → adapter.
//!
//! Every row drives the REAL wiring with an injected recording transport, so no
//! test touches the network: the endpoint under test is a private LiteLLM URL
//! that must never be dialled from CI.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use common::TestDir;
use mem_ai::provider::{
    BoxFuture, CompletionRequest, HttpRequest, HttpResponse, HttpTransport, ProviderConfig,
    ProviderError, RemoteProviderConfig,
};
use mem_bridge::host_endpoint::ProcessClock;
use mem_bridge::host_provider::{HostResolverDeps, LazyHostProvider};
use mem_bridge::manual_provider::{
    MEMORY_API_KEY_REF, REASON_UNUSABLE, build_manual_provider, is_cleartext_non_loopback,
    provider_config,
};
use mem_core::config::{Config, ConfigPaths, MemoryProvider};
use mem_core::env::EnvSource;
use mem_core::logging::LogSink;
use mem_episodic::futures::StreamExt as _;
use mem_store::platform::ProcessProbe;

/// The owner's configured endpoint, used verbatim so a wrong URL is visible.
const MEMORY_URL: &str = "http://10.90.11.16:4000/v1";
const MEMORY_MODEL: &str = "qwen3.8-flash-next-uncensored-nothink";
const MEMORY_KEY: &str = "sk-dummy-key-do-not-log";

fn sink() -> LogSink {
    static ONCE: OnceLock<LogSink> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = common::temp_dir("memory-provider-log");
        let sink = LogSink::new(dir.join("opencode-mem.log"));
        mem_core::logging::init_process_logging(sink.clone())
            .expect("the per-binary sink installs");
        sink
    })
    .clone()
}

fn sink_text() -> String {
    std::fs::read_to_string(sink().path()).expect("the per-binary sink file exists")
}

/// Records every outbound request; answers a minimal OpenAI chat completion.
#[derive(Default)]
struct RecordingTransport {
    calls: Mutex<Vec<HttpRequest>>,
}

impl RecordingTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn calls(&self) -> Vec<HttpRequest> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl HttpTransport for RecordingTransport {
    fn send(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'static, Result<HttpResponse, ProviderError>> {
        self.calls.lock().expect("calls lock").push(request);
        Box::pin(std::future::ready(Ok(HttpResponse {
            status: 200,
            body: r#"{"choices":[{"message":{"content":"{}"}}]}"#.to_owned(),
        })))
    }
}

fn env_with_home(home: &Path) -> BTreeMap<String, String> {
    [("HOME".to_string(), home.display().to_string())]
        .into_iter()
        .collect()
}

/// Writes the global config file and loads it through the real chain.
fn load_config(home: &Path, body: &str, extra_env: &[(&str, &str)]) -> Config {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create the config dir");
    std::fs::write(dir.join("opencode-mem.json"), body).expect("write the config");
    let mut env = env_with_home(home);
    for (key, value) in extra_env {
        env.insert((*key).to_string(), (*value).to_string());
    }
    Config::load(&ConfigPaths::standard(home), &env).expect("the config loads")
}

/// The OpenCode-host provider the engine builds, over inert doubles.
struct HostProbe;

impl ProcessProbe for HostProbe {
    fn is_alive(&self, _pid: u32) -> bool {
        false
    }
}

struct NoClock;

impl ProcessClock for NoClock {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

struct UnusedTransport;

impl mem_ai::provider::HostTransport for UnusedTransport {
    fn send(
        &self,
        _request: mem_ai::provider::HostRequest,
    ) -> BoxFuture<'static, Result<mem_ai::provider::HostResponse, ProviderError>> {
        Box::pin(std::future::ready(Ok(mem_ai::provider::HostResponse {
            status: 404,
            body: String::new(),
        })))
    }
}

fn host_provider(state: &Path) -> Arc<LazyHostProvider> {
    let env: BTreeMap<String, String> =
        [("XDG_STATE_HOME".to_string(), state.display().to_string())]
            .into_iter()
            .collect();
    Arc::new(LazyHostProvider::new(HostResolverDeps {
        override_url: None,
        provider_id: Some("opencode-go".to_string()),
        model_id: Some("beam-free".to_string()),
        variant: None,
        env: Arc::new(env) as Arc<dyn EnvSource + Send + Sync>,
        probe: Arc::new(HostProbe),
        clock: Arc::new(NoClock),
        transport: Arc::new(UnusedTransport),
    }))
}

const BOTH_CONFIGURED: &str = r#"{
  "autoCaptureEnabled": true,
  "opencodeProvider": "opencode-go",
  "opencodeModel": "beam-free",
  "memoryProvider": "openai-chat",
  "memoryModel": "qwen3.8-flash-next-uncensored-nothink",
  "memoryApiUrl": "http://10.90.11.16:4000/v1",
  "memoryApiKey": "sk-dummy-key-do-not-log"
}"#;

const OPENCODE_ONLY: &str = r#"{
  "autoCaptureEnabled": true,
  "opencodeProvider": "opencode-go",
  "opencodeModel": "beam-free"
}"#;

const PARTIAL_MANUAL: &str = r#"{
  "autoCaptureEnabled": true,
  "opencodeProvider": "opencode-go",
  "opencodeModel": "beam-free",
  "memoryModel": "qwen3.8-flash-next-uncensored-nothink"
}"#;

/// `memoryProvider` NAMED and nothing else: a claim with no credentials.
const PROVIDER_ONLY_CLAIM: &str = r#"{
  "autoCaptureEnabled": true,
  "opencodeProvider": "opencode-go",
  "opencodeModel": "beam-free",
  "memoryProvider": "anthropic"
}"#;

/// The manual block, when complete, wins over the OpenCode host.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_manual_fallback_wins_over_the_opencode_host() {
    let _ = sink();
    let home = TestDir::new("memory-provider-wins-home");
    let state = TestDir::new("memory-provider-wins-state");
    let config = load_config(home.path(), BOTH_CONFIGURED, &[]);

    let provider =
        mem_bridge::engine::engine_capture_provider(&config, host_provider(state.path()))
            .expect("the complete manual block composes a provider");
    assert_eq!(
        provider.name(),
        "openai-chat",
        "the configured manual fallback is the capture provider"
    );

    let content = sink_text();
    assert!(
        content.contains("\"providerId\":\"openai-chat\""),
        "the ready line names the provider actually in use: {content}"
    );
    assert!(
        content.contains(&format!("\"baseUrl\":\"{MEMORY_URL}\"")),
        "the ready line names the configured endpoint: {content}"
    );
}

/// With no manual field set, the OpenCode host provider applies (unchanged).
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_unconfigured_manual_fallback_uses_the_opencode_host() {
    let home = TestDir::new("memory-provider-host-home");
    let state = TestDir::new("memory-provider-host-state");
    let config = load_config(home.path(), OPENCODE_ONLY, &[]);

    let provider =
        mem_bridge::engine::engine_capture_provider(&config, host_provider(state.path()))
            .expect("the host path still composes");
    assert_eq!(provider.name(), "opencode-host");
}

/// A CLAIMED but unusable manual block fails closed with a NAMED reason.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_claimed_but_incomplete_manual_config_fails_closed_with_a_named_reason() {
    let _ = sink();
    let home = TestDir::new("memory-provider-partial-home");
    let state = TestDir::new("memory-provider-partial-state");
    let config = load_config(home.path(), PARTIAL_MANUAL, &[]);

    let provider =
        mem_bridge::engine::engine_capture_provider(&config, host_provider(state.path()));
    assert!(
        provider.is_none(),
        "a half-filled manual block never silently falls back to another provider"
    );

    let content = sink_text();
    assert!(
        content.contains(&format!("\"reason\":\"{REASON_UNUSABLE}\"")),
        "the failure is named: {content}"
    );
}
/// An EXPLICIT `memoryProvider` alone is a claim (ADR-0028 D-28-2, rework F2):
/// the operator named a provider, so a missing credential fails closed instead
/// of quietly dialling the OpenCode host.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_memory_provider_only_claim_fails_closed_with_a_named_reason() {
    let _ = sink();
    let home = TestDir::new("memory-provider-claim-home");
    let state = TestDir::new("memory-provider-claim-state");
    let config = load_config(home.path(), PROVIDER_ONLY_CLAIM, &[]);
    assert!(
        config.memory_provider_claimed,
        "the key's presence is carried on the resolved config"
    );

    let provider =
        mem_bridge::engine::engine_capture_provider(&config, host_provider(state.path()));
    assert!(
        provider.is_none(),
        "an operator who named a provider must never silently get the OpenCode host"
    );

    let content = sink_text();
    assert!(
        content.contains(&format!("\"reason\":\"{REASON_UNUSABLE}\"")),
        "the failure is named: {content}"
    );
}

/// Mandate 5 (ADR-0028 D-28-6): cleartext `http://` to a NON-loopback host is
/// the insecure case; loopback `http://` (local Ollama-style providers) and
/// every `https://` endpoint are not.
#[test]
fn test_cleartext_non_loopback_predicate() {
    for insecure in [
        "http://10.90.11.16:4000/v1",
        "http://192.168.1.5/v1",
        "http://litellm.example/v1",
        "http://[2001:db8::1]:4000/v1",
    ] {
        assert!(
            is_cleartext_non_loopback(insecure),
            "{insecure} carries a bearer secret in the clear"
        );
    }
    for allowed in [
        "http://127.0.0.1:4000/v1",
        "http://127.0.0.1:11434/v1",
        "http://localhost:11434/v1",
        "http://LOCALHOST:8000/v1",
        "http://[::1]:4000/v1",
        "https://10.90.11.16:4000/v1",
        "https://litellm.example/v1",
        "not a url",
        "",
    ] {
        assert!(
            !is_cleartext_non_loopback(allowed),
            "{allowed} is not a cleartext non-loopback endpoint"
        );
    }
}

/// The insecure case is named once on a log line, and the line carries no
/// credential (it logs the origin-shaped URL, not the query/Debug form).
#[test]
fn test_cleartext_non_loopback_endpoint_is_named_on_the_log() {
    let _ = sink();
    let home = TestDir::new("memory-provider-insecure-home");
    let config = load_config(
        home.path(),
        r#"{
  "memoryProvider": "openai-chat",
  "memoryModel": "qwen3.8-flash-next-uncensored-nothink",
  "memoryApiUrl": "http://10.90.11.16:4000/v1",
  "memoryApiKey": "sk-dummy-key-do-not-log"
}"#,
        &[],
    );

    let built = build_manual_provider(&config, RecordingTransport::new())
        .expect("the block still builds: the warning is a report, not a refusal");
    assert!(built.is_some(), "the operator's endpoint keeps working");

    let content = sink_text();
    assert!(
        content.contains(mem_core::logging::signatures::AUTO_CAPTURE_PROVIDER_INSECURE_ENDPOINT),
        "the cleartext endpoint is named: {content}"
    );
    assert!(
        content.contains("\"baseUrl\":\"http://10.90.11.16:4000/v1\""),
        "the warning names the endpoint: {content}"
    );
    assert!(
        !content.contains(MEMORY_KEY),
        "the warning never carries the key"
    );
}

/// F6: userinfo in `memoryApiUrl` must never reach the sink — the ready line
/// logs the sanitized URL, like the host arm's resolved origin.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_ready_line_strips_url_userinfo() {
    let _ = sink();
    let home = TestDir::new("memory-provider-userinfo-home");
    let state = TestDir::new("memory-provider-userinfo-state");
    // `https://` so this row isolates the redaction from the cleartext warning.
    let config = load_config(
        home.path(),
        r#"{
  "memoryProvider": "openai-chat",
  "memoryModel": "qwen3.8-flash-next-uncensored-nothink",
  "memoryApiUrl": "https://user:pass@litellm.example/v1",
  "memoryApiKey": "sk-dummy-key-do-not-log"
}"#,
        &[],
    );

    let provider =
        mem_bridge::engine::engine_capture_provider(&config, host_provider(state.path()))
            .expect("the complete manual block composes a provider");
    assert_eq!(provider.name(), "openai-chat");

    let content = sink_text();
    assert!(
        content.contains("\"baseUrl\":\"https://litellm.example/v1\""),
        "the ready line logs the sanitized endpoint: {content}"
    );
    assert!(
        !content.contains("user:pass"),
        "userinfo never reaches the sink: {content}"
    );
}

/// The acceptance row: the configured endpoint is dialled with the key the
/// `env://` reference resolved to, over the production registry factory.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_manual_fallback_posts_to_the_configured_url_with_the_env_resolved_key() {
    let _ = sink();
    let home = TestDir::new("memory-provider-post-home");
    let config = load_config(
        home.path(),
        r#"{
  "autoCaptureEnabled": true,
  "memoryProvider": "openai-chat",
  "memoryModel": "qwen3.8-flash-next-uncensored-nothink",
  "memoryApiUrl": "http://10.90.11.16:4000/v1",
  "memoryApiKey": "env://LITELLM_API_KEY"
}"#,
        &[("LITELLM_API_KEY", MEMORY_KEY)],
    );
    assert_eq!(
        config.memory_api_key.as_deref(),
        Some(MEMORY_KEY),
        "the env:// reference resolves through the config chain"
    );

    let transport = RecordingTransport::new();
    let provider = build_manual_provider(&config, transport.clone())
        .expect("the block builds")
        .expect("the block is configured");
    assert_eq!(provider.name(), "openai-chat");

    // Drive one completion through the REAL adapter and the recording transport.
    let request = CompletionRequest::new(MEMORY_MODEL)
        .with_message(mem_ai::provider::ChatMessage::user("capture this"));
    let mut stream = provider.complete(request);
    while stream.next().await.is_some() {}

    let calls = transport.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].url, "http://10.90.11.16:4000/v1/chat/completions",
        "the configured memoryApiUrl is the endpoint"
    );
    assert_eq!(
        calls[0].headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {MEMORY_KEY}").as_str()),
        "the resolved key rides as a bearer header: {:?}",
        calls[0].headers
    );
    assert_eq!(
        calls[0].body.as_ref().expect("a JSON body")["model"],
        MEMORY_MODEL
    );

    // The secret never reaches the log.
    assert!(
        !sink_text().contains(MEMORY_KEY),
        "the resolved key must never be logged"
    );
}

/// A complete-but-rejected config still maps to the one named reason.
#[test]
fn test_unusable_manual_config_maps_to_the_named_reason() {
    let home = TestDir::new("memory-provider-unusable-home");
    // A placeholder key is a claim that cannot be built.
    let config = load_config(
        home.path(),
        r#"{
  "memoryProvider": "openai-chat",
  "memoryModel": "qwen3.8-flash-next-uncensored-nothink",
  "memoryApiUrl": "http://10.90.11.16:4000/v1",
  "memoryApiKey": "your-api-key"
}"#,
        &[],
    );
    let error = match build_manual_provider(&config, RecordingTransport::new()) {
        Ok(_) => panic!("a placeholder key is not a usable provider"),
        Err(reason) => reason,
    };
    assert_eq!(error, REASON_UNUSABLE);
}

/// The env reference is resolved at load time; when it is absent the failure
/// NAMES the variable instead of leaving the engine with no provider.
#[test]
fn test_missing_env_reference_names_the_variable() {
    let home = TestDir::new("memory-provider-missing-env-home");
    let dir = home.path().join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(
        dir.join("opencode-mem.json"),
        r#"{"memoryProvider":"openai-chat","memoryModel":"m","memoryApiUrl":"http://127.0.0.1:1/v1","memoryApiKey":"env://LITELLM_API_KEY"}"#,
    )
    .expect("write the config");

    let error = Config::load(
        &ConfigPaths::standard(home.path()),
        &env_with_home(home.path()),
    )
    .expect_err("an unresolvable env reference is an error, never a silent None");
    let rendered = error.to_string();
    assert!(
        rendered.contains("LITELLM_API_KEY"),
        "the failure names the missing variable: {rendered}"
    );
}

/// `providerConfig` maps every `memoryProvider` value onto its adapter.
#[test]
fn test_every_memory_provider_maps_to_a_typed_config() {
    let remote = RemoteProviderConfig::new("m");
    for provider in [
        MemoryProvider::OpenAiChat,
        MemoryProvider::OpenAiResponses,
        MemoryProvider::Anthropic,
        MemoryProvider::Minimax,
        MemoryProvider::AtlasCloud,
        MemoryProvider::OrcaRouter,
    ] {
        let config = provider_config(provider, remote.clone());
        assert_eq!(config.model(), "m");
        assert_eq!(config.kind().as_str(), provider.as_str());
    }
    assert_eq!(MEMORY_API_KEY_REF, "memoryApiKey");
    let _: ProviderConfig = provider_config(MemoryProvider::OpenAiChat, remote);
}
