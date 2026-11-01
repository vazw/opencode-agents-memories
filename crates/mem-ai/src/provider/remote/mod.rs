//! Remote provider clients (W2.2).
//!
//! Seven shape-identical adapters — Anthropic Messages, OpenAI chat-completion,
//! OpenAI responses, Google Gemini, MiniMax, Atlas Cloud, OrcaRouter — behind
//! the W2.1 [`crate::provider::AiProvider`] / [`crate::provider::AiProviderStructured`]
//! seam. Every adapter is a thin shell over the shared plumbing in this module:
//! retries, per-request timeouts, auth injection, status mapping, and
//! structured-output parsing are written once and reused.

pub mod anthropic;
pub mod atlas_cloud;
pub mod auth;
pub mod error_map;
pub mod gemini;
pub mod minimax;
pub mod openai_chat;
pub mod openai_responses;
pub mod orcarouter;
pub mod plumbing;
pub mod retry;
pub mod structured;
pub mod timeout;

pub use anthropic::{ANTHROPIC_DEFAULT_BASE_URL, AnthropicConfig, AnthropicProvider};
pub use atlas_cloud::{
    ATLAS_CLOUD_API_URL, ATLAS_CLOUD_DEFAULT_MODEL, AtlasCloudConfig, AtlasCloudProvider,
};
pub use auth::AuthStrategy;
pub use error_map::{StatusOverrides, map_status, map_status_with};
pub use gemini::{GEMINI_DEFAULT_BASE_URL, GEMINI_DEFAULT_MODEL, GeminiConfig, GeminiProvider};
pub use minimax::{MINIMAX_PROVIDER_NAME, MinimaxConfig, MinimaxProvider};
pub use openai_chat::{OPENAI_CHAT_DEFAULT_BASE_URL, OpenAiChatConfig, OpenAiChatProvider};
pub use openai_responses::{
    OPENAI_RESPONSES_DEFAULT_BASE_URL, OpenAiResponsesConfig, OpenAiResponsesProvider,
};
pub use orcarouter::{
    ORCAROUTER_API_URL, ORCAROUTER_DEFAULT_MODEL, OrcaRouterConfig, OrcaRouterProvider,
};
pub use plumbing::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, OutboundRequest, ProviderInner,
    RemoteHttpClient, RemoteProviderConfig, ReqwestTransport, apply_safe_extra_params,
    completion_from_sse, conversation, decode_json, effective_system_prompt, sse_data_events,
};
pub use retry::RetryPolicy;
pub use structured::{
    StructuredAccumulator, parse_structured, parse_structured_lenient, strip_code_fences,
};
