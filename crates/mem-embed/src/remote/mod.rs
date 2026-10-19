//! Remote (OpenAI-compatible) embedding lane: endpoint options, the
//! `RemoteEmbedder` `Embedder` impl (warmup "ping" probe + embed, both through
//! the `retry` loop), re-exported over the `RemoteTransport` HTTP seam.

use std::sync::Arc;

use crate::embedder::{BoxFuture, Embedder};
use crate::error::EmbedError;

pub mod reqwest_transport;
pub mod retry;

pub use reqwest_transport::ReqwestTransport;
pub use retry::{
    REMOTE_ATTEMPT_TIMEOUT_MS, REMOTE_MAX_ATTEMPTS, REMOTE_RETRY_BACKOFF_MS, RemoteHttpResponse,
    RemoteRequest, RemoteRetryOptions, RemoteTransport, SleepFn, fetch_remote_embedding_with_retry,
    system_sleep,
};

/// Endpoint + model + retry policy for the remote lane.
#[derive(Debug, Clone)]
pub struct RemoteEmbedOptions {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub retry: RemoteRetryOptions,
}

/// Remote provider embedder.
pub struct RemoteEmbedder {
    options: RemoteEmbedOptions,
    transport: Arc<dyn RemoteTransport>,
    sleep: SleepFn,
}

impl RemoteEmbedder {
    pub fn new(
        options: RemoteEmbedOptions,
        transport: Arc<dyn RemoteTransport>,
        sleep: SleepFn,
    ) -> Self {
        Self {
            options,
            transport,
            sleep,
        }
    }
}

impl Embedder for RemoteEmbedder {
    /// Upstream probe: one minimal `"ping"` embedding through the full retry
    /// pipeline (the vector is discarded) so warmup exercises auth, URL and
    /// response parsing, not just reachability.
    fn warmup(&self) -> BoxFuture<'_, Result<(), EmbedError>> {
        Box::pin(async move {
            let _vector = fetch_remote_embedding_with_retry(
                &*self.transport,
                "ping",
                &self.options,
                &self.sleep,
            )
            .await?;
            Ok(())
        })
    }

    /// One embed through the retry path; task prefixes are applied upstream of
    /// this seam (see the `Embedder` trait docs).
    fn embed(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, EmbedError>> {
        let text = text.to_owned();
        Box::pin(async move {
            fetch_remote_embedding_with_retry(&*self.transport, &text, &self.options, &self.sleep)
                .await
        })
    }
}
