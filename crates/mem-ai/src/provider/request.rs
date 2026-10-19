//! Request/response value types crossing the provider seam.
//!
//! Every provider call carries its own timeout budget and a
//! [`CancellationToken`] so cancellation propagates into the HTTP request and
//! fires when the caller (or a dropped stream guard) goes away.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Conversation role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    /// Wire label used by chat-completion providers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One conversation message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    /// System message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    /// User message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    /// Assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// A completion request.
///
/// `json_schema` is `Some` when the caller wants structured output (the
/// upstream `format: json_schema` path); providers pass the schema through
/// unchanged (validation is the caller's responsibility).
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub system: Option<String>,
    pub json_schema: Option<Value>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Per-request budget; providers clamp to their own configured ceiling.
    pub timeout: Option<Duration>,
    /// Cancellation signal propagated into the transport request.
    pub cancellation: CancellationToken,
    /// Provider-specific extras (already validated by the caller).
    pub extra_params: BTreeMap<String, Value>,
}

impl CompletionRequest {
    /// A request against `model` with an empty conversation.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            system: None,
            json_schema: None,
            temperature: None,
            max_tokens: None,
            timeout: None,
            cancellation: CancellationToken::new(),
            extra_params: BTreeMap::new(),
        }
    }

    /// Append a message.
    pub fn with_message(mut self, message: ChatMessage) -> Self {
        self.messages.push(message);
        self
    }

    /// Set the system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// Request structured output validating against `schema`.
    pub fn with_json_schema(mut self, schema: Value) -> Self {
        self.json_schema = Some(schema);
        self
    }

    /// Set the per-request budget.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Attach a cancellation token.
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// The last user message, if any.
    pub fn last_user_message(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|message| message.role == Role::User)
            .map(|message| message.content.as_str())
    }
}

/// A batch embedding request (single round-trip for all inputs).
#[derive(Debug, Clone)]
pub struct EmbedRequest {
    pub model: String,
    pub input: Vec<String>,
    pub timeout: Option<Duration>,
    pub cancellation: CancellationToken,
}

impl EmbedRequest {
    /// A request embedding every input in one batch.
    pub fn new(model: impl Into<String>, input: Vec<String>) -> Self {
        Self {
            model: model.into(),
            input,
            timeout: None,
            cancellation: CancellationToken::new(),
        }
    }

    /// Set the per-request budget.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Attach a cancellation token.
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }
}

/// A model exposed by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub display_name: Option<String>,
    /// Whether the model supports `format: json_schema` structured output.
    pub supports_structured_output: bool,
    /// Context window in tokens, when known.
    pub context_length: Option<u32>,
}

impl ModelInfo {
    /// A minimal model descriptor.
    pub fn new(id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            display_name: None,
            supports_structured_output: false,
            context_length: None,
        }
    }
}
