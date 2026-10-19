//! `mem-ai::provider` — the unified AI provider seam (US 4.1).
//!
//! Everything above this module talks to providers through the object-safe
//! [`AiProvider`] trait; provider-specific types never cross the boundary.
//! [`AiProviderStructured`] is the sealed capability extension for
//! `format: json_schema` structured output.
//!
//! - [`ProviderRegistry`] resolves typed [`ProviderConfig`] values into
//!   `Arc<dyn AiProvider>`, receiving its [`SecretResolver`] by injection.
//! - [`OpenCodeHostProvider`] implements the seam over the OpenCode HTTP host.
//! - [`InternalCaptureSessions`] is the process-global loop guard.
//! - [`FileSecretResolver`] is W2.3's file-backed [`SecretResolver`]: strict
//!   permissions, bounded reads, path-only error messages.

pub mod error;
pub mod internal_capture;
pub mod opencode_host;
pub mod registry;
pub mod remote;
pub mod request;
pub mod secret_resolver;

#[path = "trait.rs"]
pub mod provider_trait;

pub use error::{ProviderError, ProviderErrorKind};
pub use internal_capture::{
    INTERNAL_CAPTURE_SESSION_TITLE, InternalCaptureSessions, UNTRACK_GRACE,
    is_internal_capture_session_title,
};
pub use opencode_host::{
    ConnectedRefresher, HostMethod, HostRequest, HostResponse, HostTransport, OpenCodeHostConfig,
    OpenCodeHostProvider, PROVIDER_NAME, ReqwestHostTransport, STRUCTURED_OUTPUT_AGENT,
    STRUCTURED_OUTPUT_DEFAULT_RETRY_COUNT, STRUCTURED_OUTPUT_MAX_STEPS,
    STRUCTURED_OUTPUT_TIMEOUT_MS, StepNote, estimate_token_count,
};
pub use provider_trait::{AiProvider, AiProviderStructured, BoxFuture, CompletionStream};
pub use registry::{ProviderConfig, ProviderKind, ProviderRegistry};
pub use remote::{
    AnthropicProvider, AtlasCloudProvider, AuthStrategy, GeminiProvider, HttpMethod, HttpRequest,
    HttpResponse, HttpTransport, MinimaxProvider, OpenAiChatProvider, OpenAiResponsesProvider,
    OrcaRouterProvider, OutboundRequest, ProviderInner, RemoteHttpClient, RemoteProviderConfig,
    ReqwestTransport, RetryPolicy, StatusOverrides, StructuredAccumulator,
};
pub use request::{ChatMessage, CompletionRequest, EmbedRequest, ModelInfo, Role};
pub use secret_resolver::{SecretResolver, StaticSecretResolver};

pub use crate::privacy::secret::FileSecretResolver;
