//! `PrivacyFilterLayer` middleware behavior (AC7, D8).
//!
//! The layer filters **requests only**: messages, `system`, embed inputs and
//! `count_tokens` text are filtered on the way out; completion chunks, embed
//! vectors, token counts, model lists and structured values come back
//! verbatim. The structured-capability half of AC7 lives in the in-crate
//! `#[cfg(test)] mod tests` of `src/privacy/layer.rs` — `AiProviderStructured`
//! is sealed, so no integration test can implement it.

#[path = "common/mod.rs"]
mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use mem_ai::privacy::layer::{PrivacyConfig, PrivacyFilterLayer};
use mem_ai::provider::{
    AiProvider, ChatMessage, CompletionRequest, CompletionStream, EmbedRequest, ModelInfo,
    OpenCodeHostConfig, ProviderConfig, ProviderError, ProviderRegistry, Role,
    StaticSecretResolver,
};
use serde_json::{Value, json};

const SECRET: &str = "sk-live-abc123";
const INJECTED: &str = "<system-reminder>\n[BACKGROUND TASK RESULT READY]\n</system-reminder>";
const AUTHORED: &str = "Refactor the auth middleware.";

/// Records every request it receives and replies with a fixed chunk.
struct RecordingProvider {
    name: String,
    completions: Mutex<Vec<CompletionRequest>>,
    embeddings: Mutex<Vec<EmbedRequest>>,
    counted_texts: Mutex<Vec<String>>,
    chunk: Value,
}

impl RecordingProvider {
    fn new() -> Self {
        Self {
            name: "recording".to_owned(),
            completions: Mutex::new(Vec::new()),
            embeddings: Mutex::new(Vec::new()),
            counted_texts: Mutex::new(Vec::new()),
            chunk: json!({ "text": "ok" }),
        }
    }

    fn with_chunk(chunk: Value) -> Self {
        Self {
            chunk,
            ..Self::new()
        }
    }

    fn last_completion(&self) -> CompletionRequest {
        self.completions
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("no completion request was recorded")
    }

    fn completion_count(&self) -> usize {
        self.completions.lock().unwrap().len()
    }

    fn last_embedding(&self) -> EmbedRequest {
        self.embeddings
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("no embed request was recorded")
    }

    fn last_counted_text(&self) -> String {
        self.counted_texts
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("no count_tokens call was recorded")
    }
}

#[async_trait]
impl AiProvider for RecordingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete(&self, request: CompletionRequest) -> CompletionStream {
        self.completions.lock().unwrap().push(request);
        let chunk = self.chunk.clone();
        Box::pin(stream::once(async move { Ok(chunk) }))
    }

    async fn embed(&self, request: EmbedRequest) -> Result<Vec<Vec<f32>>, ProviderError> {
        let vectors = vec![vec![0.0_f32]; request.input.len()];
        self.embeddings.lock().unwrap().push(request);
        Ok(vectors)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![ModelInfo::new("recording-model", "recording")])
    }

    async fn count_tokens(&self, _model: &str, text: &str) -> Result<u32, ProviderError> {
        self.counted_texts.lock().unwrap().push(text.to_owned());
        Ok(text.chars().count() as u32)
    }
}

fn layer() -> PrivacyFilterLayer {
    PrivacyFilterLayer::new(PrivacyConfig::default())
}

async fn drain(mut stream: CompletionStream) -> Vec<Result<Value, ProviderError>> {
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk);
    }
    chunks
}

// ---------------------------------------------------------------------------
// Request side: filtering what leaves the process.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_layer_strips_private_content_from_completion_messages() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = CompletionRequest::new("m").with_message(ChatMessage::user(format!(
        "hello <private>{SECRET}</private> world"
    )));

    let _ = drain(wrapped.complete(request)).await;

    let recorded = inner.last_completion();
    assert_eq!(recorded.messages[0].content, "hello [REDACTED] world");
    assert!(!recorded.messages[0].content.contains(SECRET));
}

#[tokio::test]
async fn test_layer_strips_private_content_from_system_prompt() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = CompletionRequest::new("m")
        .with_system(format!("be nice <private>{SECRET}</private>"))
        .with_message(ChatMessage::user(AUTHORED));

    let _ = drain(wrapped.complete(request)).await;

    assert_eq!(
        inner.last_completion().system.as_deref(),
        Some("be nice [REDACTED]")
    );
}

#[tokio::test]
async fn test_layer_drops_injected_user_message() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = CompletionRequest::new("m")
        .with_message(ChatMessage::user(INJECTED))
        .with_message(ChatMessage::user(AUTHORED));

    let _ = drain(wrapped.complete(request)).await;

    let recorded = inner.last_completion();
    assert_eq!(recorded.messages.len(), 1);
    assert_eq!(recorded.messages[0].content, AUTHORED);
}

#[tokio::test]
async fn test_layer_keeps_authored_messages_and_order() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = CompletionRequest::new("m")
        .with_message(ChatMessage::user("first"))
        .with_message(ChatMessage::assistant("second"))
        .with_message(ChatMessage::user("third"));

    let _ = drain(wrapped.complete(request)).await;

    let contents: Vec<String> = inner
        .last_completion()
        .messages
        .into_iter()
        .map(|message| message.content)
        .collect();
    assert_eq!(contents, vec!["first", "second", "third"]);
}

#[tokio::test]
async fn test_layer_keeps_allowlisted_internal_prompt() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let internal = format!("<!-- OMO_INTERNAL_INITIATOR -->\n# User Profile Analysis\n{AUTHORED}");
    let request = CompletionRequest::new("m").with_message(ChatMessage::user(internal.clone()));

    let _ = drain(wrapped.complete(request)).await;

    let recorded = inner.last_completion();
    assert_eq!(recorded.messages.len(), 1);
    assert_eq!(recorded.messages[0].content, internal);
}

#[tokio::test]
async fn test_layer_does_not_filter_assistant_or_system_roles() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = CompletionRequest::new("m")
        .with_message(ChatMessage::assistant(INJECTED))
        .with_message(ChatMessage::system(INJECTED));

    let _ = drain(wrapped.complete(request)).await;

    let recorded = inner.last_completion();
    assert_eq!(recorded.messages.len(), 2);
    assert_eq!(recorded.messages[0].role, Role::Assistant);
    assert_eq!(recorded.messages[1].role, Role::System);
}

#[tokio::test]
async fn test_layer_errors_when_all_messages_are_injected() {
    // D8: never send an empty conversation, and never call the inner provider.
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = CompletionRequest::new("m")
        .with_message(ChatMessage::user(format!("{INJECTED}\n{INJECTED}")));

    let chunks = drain(wrapped.complete(request)).await;

    assert_eq!(inner.completion_count(), 0);
    assert_eq!(chunks.len(), 1);
    let error = chunks[0]
        .as_ref()
        .expect_err("the single chunk is an error");
    assert_eq!(error.message, "all messages were filtered as injected");
}

#[tokio::test]
async fn test_layer_strips_private_content_from_embed_inputs() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = EmbedRequest::new(
        "m",
        vec![
            format!("intro <private>{SECRET}</private>"),
            AUTHORED.to_owned(),
        ],
    );

    let _ = wrapped.embed(request).await.unwrap();

    let recorded = inner.last_embedding();
    assert_eq!(recorded.input[0], "intro [REDACTED]");
    assert!(!recorded.input[0].contains(SECRET));
    assert_eq!(recorded.input[1], AUTHORED);
}

#[tokio::test]
async fn test_layer_embed_preserves_index_alignment_for_fully_private_input() {
    // D8: embed never drops inputs — the caller zips vectors to inputs.
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let request = EmbedRequest::new(
        "m",
        vec![format!("<private>{SECRET}</private>"), AUTHORED.to_owned()],
    );

    let vectors = wrapped.embed(request).await.unwrap();

    let recorded = inner.last_embedding();
    assert_eq!(recorded.input.len(), 2);
    assert_eq!(recorded.input[0], "[REDACTED]");
    assert_eq!(vectors.len(), 2);
}

#[tokio::test]
async fn test_layer_count_tokens_counts_filtered_text() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    let filtered = format!("aaaa<private>{SECRET}</private>");

    let count = wrapped.count_tokens("m", &filtered).await.unwrap();

    let counted = inner.last_counted_text();
    assert_eq!(counted, "aaaa[REDACTED]");
    assert_eq!(count, "aaaa[REDACTED]".chars().count() as u32);
}

// ---------------------------------------------------------------------------
// Response side: verbatim pass-through (D8).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_layer_delegates_list_models() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());

    let models = wrapped.list_models().await.unwrap();

    assert_eq!(models, vec![ModelInfo::new("recording-model", "recording")]);
}

#[tokio::test]
async fn test_layer_forwards_provider_name() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = layer().wrap(inner.clone());
    assert_eq!(wrapped.name(), "recording");
}

#[tokio::test]
async fn test_layer_respects_disabled_strip_private_flag() {
    let inner = Arc::new(RecordingProvider::new());
    let wrapped = PrivacyFilterLayer::new(PrivacyConfig {
        strip_private: false,
        ..PrivacyConfig::default()
    })
    .wrap(inner.clone());
    let request = CompletionRequest::new("m")
        .with_message(ChatMessage::user(format!("<private>{SECRET}</private>")));

    let _ = drain(wrapped.complete(request)).await;

    let recorded = inner.last_completion();
    assert_eq!(
        recorded.messages[0].content,
        format!("<private>{SECRET}</private>")
    );
}

#[tokio::test]
async fn test_layer_wraps_registry_provider() {
    let mut registry = ProviderRegistry::new(Arc::new(StaticSecretResolver::new()));
    let created = registry
        .register_builtin()
        .create(&ProviderConfig::OpenCodeHost(OpenCodeHostConfig {
            // Explicit: the factory validates the base URL, and the default is
            // deliberately empty (not dialable).
            base_url: "http://127.0.0.1:4096".to_owned(),
            ..OpenCodeHostConfig::default()
        }))
        .expect("the built-in host factory should build");

    let wrapped = layer().wrap(created);

    assert_eq!(wrapped.name(), mem_ai::provider::PROVIDER_NAME);
}

#[tokio::test]
async fn test_layer_forwards_completion_chunks_unchanged() {
    // The chunk carries a `<private>` marker: it must arrive verbatim, proving
    // there is no response-side filtering.
    let chunk = json!({ "text": format!("<private>{SECRET}</private>"), "done": true });
    let inner = Arc::new(RecordingProvider::with_chunk(chunk.clone()));
    let wrapped = layer().wrap(inner.clone());

    let chunks = drain(
        wrapped.complete(CompletionRequest::new("m").with_message(ChatMessage::user(AUTHORED))),
    )
    .await;

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].as_ref().unwrap(), &chunk);
}
