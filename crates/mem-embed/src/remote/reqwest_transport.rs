//! Reqwest-backed remote transport: POST JSON with bearer auth and a
//! per-attempt client timeout, mapped onto the retry loop's error classes.

use crate::embedder::BoxFuture;
use crate::error::EmbedError;
use crate::remote::retry::{RemoteHttpResponse, RemoteRequest, RemoteTransport};

/// Real reqwest transport for OpenAI-compatible embedding endpoints.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
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

impl RemoteTransport for ReqwestTransport {
    /// `POST {url}` with `Authorization: Bearer {api_key}` and the JSON body
    /// `{ "input": ..., "model": ... }`; the client timeout is the request's
    /// per-attempt budget (the retry loop wraps a matching `tokio::timeout`).
    /// Transport failures classify as `RemoteTimeout` (elapsed) or
    /// `RemoteNetwork` (everything else); the status line and body text are
    /// carried verbatim for the retry loop to classify.
    fn post_json(
        &self,
        request: RemoteRequest,
    ) -> BoxFuture<'_, Result<RemoteHttpResponse, EmbedError>> {
        let attempt_timeout = request.attempt_timeout;
        let timeout_ms = attempt_timeout.as_millis() as u64;
        Box::pin(async move {
            let response = self
                .client
                .post(&request.url)
                .bearer_auth(&request.api_key)
                .timeout(attempt_timeout)
                .json(&serde_json::json!({
                    "input": request.input,
                    "model": request.model,
                }))
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        EmbedError::RemoteTimeout { timeout_ms }
                    } else {
                        EmbedError::RemoteNetwork {
                            message: error.to_string(),
                        }
                    }
                })?;
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .map_err(|error| EmbedError::RemoteNetwork {
                    message: error.to_string(),
                })?;
            Ok(RemoteHttpResponse { status, body })
        })
    }
}
