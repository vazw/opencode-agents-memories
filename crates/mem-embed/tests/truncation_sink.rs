//! The truncation events must reach the engine's FILE SINK, not just exist.
//!
//! Its own test binary because the global subscriber is once-per-process
//! (precedent: `crates/mem-core/tests/logging_sink_scope.rs`). `embed()` runs
//! the encode inside `spawn_blocking`, whose thread does not inherit a
//! thread-local dispatcher, so a scoped `with_default` subscriber would miss
//! the event — the global install is the only honest oracle here.
//!
//! The install is the same call `src/main.rs` makes
//! (`init_process_logging(LogSink::new(path))`), so no dev-dependency is
//! needed: a bare `tracing::warn!` from `mem-embed` (target `mem_embed::…`)
//! is dropped by `SinkLayer` and leaves this file without the event.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use mem_core::config::EmbeddingPooling;
use mem_core::logging::{LogSink, init_process_logging};
use mem_embed::Embedder;
use mem_embed::local::{LocalLoadOptions, LocalOnnxEmbedder};

const MODEL: &str = "org/tiny-embed";
/// The fixture declares 8 positions, so the derived cap is 8 - 2 = 6.
const FIXTURE_CONFIG: &str = r#"{"model_type":"tiny","max_position_embeddings":8}"#;
/// Step 1 measured 2,002 ids for this input.
const MEASURED_IDS: u64 = 2_002;
const CAP: u64 = 6;

/// The process-wide sink plus the temp dir that owns its file.
struct Sink {
    _dir: common::TempDir,
    path: PathBuf,
}

/// Install the global subscriber exactly once, whatever the test order.
fn sink() -> &'static Sink {
    static SINK: OnceLock<Sink> = OnceLock::new();
    SINK.get_or_init(|| {
        let dir = common::temp_dir("truncation-sink");
        let path = dir.path().join("opencode-mem.log");
        init_process_logging(LogSink::new(path.clone()))
            .expect("the global logging install must succeed in a fresh process");
        Sink { _dir: dir, path }
    })
}

fn failing_input() -> String {
    "hello ".repeat(2_000)
}

fn fixture_embedder(cache: &common::TempDir) -> LocalOnnxEmbedder {
    let model_dir = cache.path().join(MODEL);
    common::onnx_fixture::write_tiny_model(&model_dir, "onnx/model.onnx", 1.0, false);
    std::fs::copy(
        common::onnx_fixture::tiny_tokenizer_path(),
        model_dir.join("tokenizer.json"),
    )
    .expect("copy the committed tokenizer fixture");
    common::write_file(&model_dir, "config.json", FIXTURE_CONFIG);

    LocalOnnxEmbedder::new(LocalLoadOptions {
        model: MODEL.to_string(),
        cache_root: cache.path().to_path_buf(),
        dtype: None,
        pooling: EmbeddingPooling::Mean,
        intra_threads: Some(1),
        hub: None,
        max_tokens: None,
    })
}

fn read_sink() -> String {
    fs::read_to_string(&sink().path).expect("the log file is readable")
}

/// The `data_json` of the first line whose message is `event`.
fn payload_for(content: &str, event: &str) -> Option<serde_json::Value> {
    content.lines().find_map(|line| {
        let (_, rest) = line.split_once("] ")?;
        let (message, data) = rest.split_once(": ")?;
        if message != event {
            return None;
        }
        serde_json::from_str(data).ok()
    })
}

fn payload_or_panic(content: &str, event: &str) -> serde_json::Value {
    payload_for(content, event)
        .unwrap_or_else(|| panic!("`{event}` must reach the file sink; file was:\n{content}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_truncation_reaches_the_log_sink() {
    // Install the sink BEFORE any event is emitted (the global subscriber is
    // once-per-process and a pre-install `log` call is a no-op).
    sink();
    let cache = common::temp_dir("truncation-sink-embed");
    let embedder = fixture_embedder(&cache);

    embedder.embed(&failing_input()).await.expect("embed");

    let content = read_sink();
    let payload = payload_or_panic(&content, "embedding.input_truncated");
    assert_eq!(
        payload["cap_tokens"].as_u64(),
        Some(CAP),
        "the event carries the resolved cap"
    );
    assert_eq!(
        payload["dropped_tokens"].as_u64(),
        Some(MEASURED_IDS - CAP),
        "a bare `tracing::warn!` mutant leaves the file without this event"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_load_time_cap_resolution_reaches_the_log_sink() {
    sink();
    let cache = common::temp_dir("truncation-sink-load");
    let embedder = fixture_embedder(&cache);

    embedder.warmup().await.expect("warmup loads the model");

    let content = read_sink();
    let payload = payload_or_panic(&content, "embedding.input_cap_resolved");
    assert_eq!(
        payload["cap_tokens"].as_u64(),
        Some(CAP),
        "the load-time line names the resolved cap (mutant: no `data_json`)"
    );
    assert_eq!(
        payload["source"].as_str(),
        Some("model"),
        "the load-time line names where the cap came from"
    );
    assert_eq!(payload["special_tokens"].as_u64(), Some(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_truncation_line_carries_counts_not_content() {
    sink();
    let cache = common::temp_dir("truncation-sink-keys");
    let embedder = fixture_embedder(&cache);

    embedder.embed(&failing_input()).await.expect("embed");

    let content = read_sink();
    let payload = payload_or_panic(&content, "embedding.input_truncated");
    let mut keys: Vec<&str> = payload
        .as_object()
        .expect("the payload is a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["cap_tokens", "dropped_tokens", "model", "text_chars"],
        "the exact key set forbids any text field (not merely a long substring)"
    );
    assert_eq!(
        payload["text_chars"].as_u64(),
        Some(12_000),
        "the input length is a count, not the text"
    );
    assert!(
        !content.contains("hello hello"),
        "no input text may ever reach the sink"
    );
}
