//! Remote retry core: the upstream `fetchRemoteEmbeddingWithRetry` loop —
//! per-attempt timeout, linear backoff, upstream retryable classification.

use std::sync::Arc;
use std::time::Duration;

use crate::embedder::BoxFuture;
use crate::error::EmbedError;
use crate::remote::RemoteEmbedOptions;

/// Per-attempt timeout (upstream 8s).
pub const REMOTE_ATTEMPT_TIMEOUT_MS: u64 = 8000;
/// Total attempts (upstream 3).
pub const REMOTE_MAX_ATTEMPTS: u32 = 3;
/// Linear backoff unit (upstream 500ms; attempt N waits `backoff * N`).
pub const REMOTE_RETRY_BACKOFF_MS: u64 = 500;

/// One OpenAI-compatible `/embeddings` call.
#[derive(Debug, Clone)]
pub struct RemoteRequest {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub input: String,
    pub attempt_timeout: Duration,
}

/// Raw transport response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHttpResponse {
    pub status: u16,
    pub body: String,
}

/// HTTP seam for remote embedding (fake-injected in tests).
pub trait RemoteTransport: Send + Sync {
    fn post_json(
        &self,
        request: RemoteRequest,
    ) -> BoxFuture<'_, Result<RemoteHttpResponse, EmbedError>>;
}

/// Retry policy knobs; upstream defaults 8s / 3 / 500ms.
#[derive(Debug, Clone)]
pub struct RemoteRetryOptions {
    pub attempt_timeout: Duration,
    pub max_attempts: u32,
    pub backoff: Duration,
}

impl Default for RemoteRetryOptions {
    fn default() -> Self {
        Self {
            attempt_timeout: Duration::from_millis(REMOTE_ATTEMPT_TIMEOUT_MS),
            max_attempts: REMOTE_MAX_ATTEMPTS,
            backoff: Duration::from_millis(REMOTE_RETRY_BACKOFF_MS),
        }
    }
}

/// Injectable sleep so tests can record backoff without waiting.
pub type SleepFn = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// Production sleep.
pub fn system_sleep() -> SleepFn {
    Arc::new(|duration| Box::pin(async move { tokio::time::sleep(duration).await }))
}

/// OpenAI `/embeddings` response shape: `{ data: [{ embedding: [...] }] }`.
#[derive(serde::Deserialize)]
struct EmbeddingsPayload {
    data: Vec<EmbeddingEntry>,
}

#[derive(serde::Deserialize)]
struct EmbeddingEntry {
    embedding: Vec<f32>,
}

/// Classify + decode one response: non-2xx → `RemoteHttp`, bad JSON →
/// `RemoteInvalidJson`, missing/empty `data[0].embedding` → non-retryable
/// `RemoteInvalidJson`-class failure.
fn decode_embedding_response(response: &RemoteHttpResponse) -> Result<Vec<f32>, EmbedError> {
    if !(200..=299).contains(&response.status) {
        // Divergence (5): the transport contract carries no fetch-style
        // statusText, so it is derived as "" and the Display is pinned with
        // the status only (see test_non_retryable_4xx_fails_fast).
        return Err(EmbedError::RemoteHttp {
            status: response.status,
            status_text: String::new(),
        });
    }
    let payload: EmbeddingsPayload =
        serde_json::from_str(&response.body).map_err(|parse_error| {
            EmbedError::RemoteInvalidJson {
                message: parse_error.to_string(),
            }
        })?;
    payload
        .data
        .into_iter()
        .next()
        .map(|entry| entry.embedding)
        .filter(|embedding| !embedding.is_empty())
        .ok_or_else(|| EmbedError::RemoteInvalidJson {
            message: "embedding data missing or empty".to_owned(),
        })
}

/// Upstream `fetchRemoteEmbeddingWithRetry` loop: attempt 1..=max, per-attempt
/// timeout (a hung attempt costs one attempt, never the whole budget; the same
/// timeout is handed to the transport), LINEAR backoff (`backoff × attempt`),
/// upstream retryable classification — only 429/5xx/timeout/network retry;
/// everything else (invalid JSON, shape errors, other 4xx) fails fast.
pub async fn fetch_remote_embedding_with_retry(
    transport: &dyn RemoteTransport,
    input: &str,
    options: &RemoteEmbedOptions,
    sleep: &SleepFn,
) -> Result<Vec<f32>, EmbedError> {
    let retry = &options.retry;
    let url = format!("{}/embeddings", options.url);
    for attempt in 1..=retry.max_attempts {
        let request = RemoteRequest {
            url: url.clone(),
            api_key: options.api_key.clone(),
            model: options.model.clone(),
            input: input.to_owned(),
            attempt_timeout: retry.attempt_timeout,
        };
        let attempt_result =
            match tokio::time::timeout(retry.attempt_timeout, transport.post_json(request)).await {
                Err(_elapsed) => Err(EmbedError::RemoteTimeout {
                    timeout_ms: retry.attempt_timeout.as_millis() as u64,
                }),
                Ok(Err(error)) => Err(error),
                Ok(Ok(response)) => decode_embedding_response(&response),
            };
        match attempt_result {
            Ok(embedding) => return Ok(embedding),
            Err(error) => {
                if !error.is_retryable() || attempt == retry.max_attempts {
                    return Err(error);
                }
                sleep(retry.backoff * attempt).await;
            }
        }
    }
    // max_attempts = 0: upstream threw a generic `Error(String(undefined))`;
    // mapped onto a non-retryable error shape here.
    Err(EmbedError::RemoteInvalidJson {
        message: format!(
            "remote embedding produced no attempt (max_attempts = {})",
            retry.max_attempts
        ),
    })
}
