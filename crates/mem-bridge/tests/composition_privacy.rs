//! Issue 52 — the mint's three oracles (D-52-4).
//!
//! The tests live OUTSIDE `composition.rs` on purpose: `build_composed_runtime`
//! is `pub`, `ComposedRuntime.provider` is `pub` and `CaptureService::privacy()`
//! is `pub`, so the mint is reachable from an integration test and the file's
//! diff gate stays closable by a correct implementation (§3.3 exception 2,
//! §9.3).
//!
//! The recording provider's `AiProvider` impl is written in the **desugared**
//! form (the precedent is `crates/mem-episodic/tests/common/mod.rs`'s
//! `FakeProvider`): no `async-trait` dependency is added to `mem-bridge`.
//! The oracle does not drain the returned stream: `PrivacyFilterProvider::
//! complete` calls the inner provider synchronously, so the recorder holds the
//! request before the stream is polled (dropping the stream is fine).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mem_ai::privacy::PrivacyConfig;
use mem_ai::provider::request::{ChatMessage, CompletionRequest, EmbedRequest, ModelInfo};
use mem_ai::provider::{AiProvider, BoxFuture, CompletionStream, ProviderError};
use mem_bridge::composition::build_composed_runtime;
use mem_store::serde_json::Value;

struct RecordingProvider {
    requests: Mutex<Vec<CompletionRequest>>,
}

impl RecordingProvider {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<CompletionRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl AiProvider for RecordingProvider {
    fn name(&self) -> &str {
        "recording-provider"
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        self.requests.lock().expect("requests lock").push(request);
        Box::pin(mem_episodic::futures::stream::iter(Vec::<
            Result<Value, ProviderError>,
        >::new()))
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

struct NoopEmbedder;

impl mem_embed::embedder::Embedder for NoopEmbedder {
    fn warmup(
        &self,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<(), mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(()) })
    }

    fn embed(
        &self,
        _text: &str,
    ) -> mem_embed::embedder::BoxFuture<'_, Result<Vec<f32>, mem_embed::error::EmbedError>> {
        Box::pin(async { Ok(vec![0.0; 8]) })
    }
}

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mem-bridge-i52-{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn load_config(root: &std::path::Path, home: &std::path::Path) -> mem_core::config::Config {
    let env: BTreeMap<String, String> = [("HOME".to_string(), home.to_string_lossy().to_string())]
        .into_iter()
        .collect();
    let mut config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
            .expect("the default config loads");
    config.storage_path = root.to_path_buf();
    config.embedding_dimensions = 8;
    config
}

fn two_message_request() -> CompletionRequest {
    CompletionRequest::new("m")
        .with_message(ChatMessage::user("clean question"))
        .with_message(ChatMessage::user("<my-banner>x</my-banner>"))
}

// ---------------------------------------------------------------------------
// Step 14 — test 14: the mint wraps the provider
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_composed_runtime_wraps_the_minted_provider_in_the_privacy_layer() {
    // Wrong implementations this kills:
    // - minted but never wrapped -> 2 messages reach the recorder;
    // - wrapped with `PrivacyConfig::default()` (ignores the configured
    //   marker) -> 2 messages, because `<my-banner>` is not a default marker;
    // - `Arc::new(layer.wrap(..))` -> does not compile.
    let root = temp_dir("composition-wrap");
    let home = temp_dir("composition-wrap-home");
    let mut config = load_config(&root, &home);
    config.chat_message.filter_injected_prompts = true;
    config
        .chat_message
        .injection_markers
        .push("<my-banner>".to_string());

    let recorder = Arc::new(RecordingProvider::new());
    let runtime = build_composed_runtime(
        &root,
        &config,
        Arc::new(NoopEmbedder),
        Some(recorder.clone() as Arc<dyn AiProvider>),
    )
    .await
    .expect("the runtime composes");

    let provider = runtime
        .provider
        .clone()
        .expect("the composed runtime exposes the capture provider");
    let _stream = provider.complete(two_message_request());

    let recorded = recorder.requests();
    assert_eq!(recorded.len(), 1, "the inner provider is reached once");
    assert_eq!(
        recorded[0].messages.len(),
        1,
        "the injected message is dropped before the inner provider"
    );
    assert_eq!(recorded[0].messages[0].content, "clean question");

    drop(runtime);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// Step 14 — test 15: the minted layer reads the flag
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_the_minted_layer_reads_filter_injected_prompts() {
    // A hard-coded `filter_injected_prompts: true` in `from_core` leaves this
    // RED: only one message would reach the recorder.
    let root = temp_dir("composition-flag");
    let home = temp_dir("composition-flag-home");
    let mut config = load_config(&root, &home);
    config.chat_message.filter_injected_prompts = false;
    config
        .chat_message
        .injection_markers
        .push("<my-banner>".to_string());

    let recorder = Arc::new(RecordingProvider::new());
    let runtime = build_composed_runtime(
        &root,
        &config,
        Arc::new(NoopEmbedder),
        Some(recorder.clone() as Arc<dyn AiProvider>),
    )
    .await
    .expect("the runtime composes");

    let provider = runtime.provider.clone().expect("provider");
    let _stream = provider.complete(two_message_request());

    let recorded = recorder.requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].messages.len(),
        2,
        "filter off: both messages reach the provider"
    );

    drop(runtime);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// Step 14 — test 16: the resolved config reaches the capture service
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_composed_runtime_hands_the_resolved_privacy_config_to_capture() {
    // Omitting `.with_privacy(...)` leaves `PrivacyConfig::default()` (flag
    // `true`, no `<my-banner>`) -> not equal. `PartialEq` is over `bool`s and
    // two `Vec<String>`s, so the comparison is element-wise by construction.
    let root = temp_dir("composition-privacy-link");
    let home = temp_dir("composition-privacy-link-home");
    let mut config = load_config(&root, &home);
    config.chat_message.filter_injected_prompts = false;
    config
        .chat_message
        .injection_markers
        .push("<my-banner>".to_string());

    let runtime = build_composed_runtime(&root, &config, Arc::new(NoopEmbedder), None)
        .await
        .expect("the runtime composes");

    assert_eq!(
        runtime.capture.privacy(),
        &PrivacyConfig::from_core(&config)
    );

    drop(runtime);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&home);
}
