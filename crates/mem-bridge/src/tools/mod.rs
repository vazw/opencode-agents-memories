//! Issue 70 wave 1 — the nine `memory` tool methods: the one composed service
//! struct behind `EngineRuntime.tools`, the closed failure surface, and the
//! handler-to-JSON-RPC map.
//!
//! The methods themselves live in their tier modules ([`memories`],
//! [`profile`], [`inventory`], [`portability`]); this module owns the seam the
//! engine talks to, so `dispatch` stays a thin adapter over store primitives.

pub mod inventory;
pub mod memories;
pub mod portability;
pub mod profile;

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::composition::ComposedRuntime;
use crate::error_map::{self, ErrorKind};
use crate::jsonrpc::{Request, Response};

/// The composed dispatch services for one runtime.
///
/// `build` is the production constructor (every dependency comes from
/// [`ComposedRuntime`] or the resolved config); `from_parts` is the test
/// constructor, so no oracle needs a whole `ComposedRuntime`.
pub struct ToolServices {
    pub(crate) memories: memories::MemoriesService,
    pub(crate) profile: profile::ProfileService,
    pub(crate) inventory: inventory::InventoryService,
    /// `None` = this build ships no portability backend; `export`/`import`
    /// then answer `-32001`.
    pub(crate) portability: Option<Arc<dyn portability::PortabilityBackend>>,
}

impl ToolServices {
    /// Production: the deps come from `composed` and the resolved `core`.
    pub fn build(
        composed: &ComposedRuntime,
        core: &mem_core::config::Config,
        portability: Option<Arc<dyn portability::PortabilityBackend>>,
    ) -> Self {
        Self {
            memories: memories::MemoriesService::from_composed(composed, core),
            profile: profile::ProfileService::new(
                composed.profile.clone(),
                composed.learning_lock.clone(),
                composed.scope_resolver.clone(),
            ),
            inventory: inventory::InventoryService::from_composed(composed),
            portability,
        }
    }

    /// Tests: the same struct from ready-made parts.
    pub fn from_parts(
        memories: memories::MemoriesService,
        profile: profile::ProfileService,
        inventory: inventory::InventoryService,
        portability: Option<Arc<dyn portability::PortabilityBackend>>,
    ) -> Self {
        Self {
            memories,
            profile,
            inventory,
            portability,
        }
    }
}

/// One tool method's failure, mapped by [`respond`] onto the three wire codes
/// the nine methods are allowed to answer with.
pub enum ToolError {
    /// `-32602`: a parameter (or a service refusal the client can fix) with
    /// the closed `data.reason` token.
    Invalid {
        reason: &'static str,
        message: Option<String>,
        extra: Map<String, Value>,
    },
    /// `-32001`: this build cannot serve the method (no backend).
    Degraded,
    /// `-32603`: an internal failure. Advisory data is client-safe by
    /// construction — never a store path, a key or transcript text.
    Internal { data: Option<Value> },
}

impl ToolError {
    /// A `-32602` failure carrying only its reason token.
    pub fn invalid(reason: &'static str) -> Self {
        Self::Invalid {
            reason,
            message: None,
            extra: Map::new(),
        }
    }

    /// Adds the client-safe service sentence.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        if let Self::Invalid { message: slot, .. } = &mut self {
            *slot = Some(message.into());
        }
        self
    }

    /// Adds one extra client-safe field to the reason object.
    pub fn with_extra(mut self, key: &'static str, value: Value) -> Self {
        if let Self::Invalid { extra, .. } = &mut self {
            extra.insert(key.to_string(), value);
        }
        self
    }

    /// A bare `-32603`.
    pub fn internal() -> Self {
        Self::Internal { data: None }
    }

    /// A `-32603` with advisory data (the `locked`/`learning_locked`/
    /// `profile_conflict`/`profile_unreadable` tokens ride here).
    pub fn internal_data(data: Value) -> Self {
        Self::Internal { data: Some(data) }
    }
}

/// Maps one handler outcome onto the JSON-RPC response the wire carries.
pub fn respond(request: &Request, outcome: Result<Value, ToolError>) -> Response {
    match outcome {
        Ok(result) => Response::result(request.id.clone(), result),
        Err(ToolError::Invalid {
            reason,
            message,
            mut extra,
        }) => {
            extra.insert("reason".to_string(), Value::String(reason.to_string()));
            if let Some(message) = message {
                extra.insert("message".to_string(), Value::String(message));
            }
            Response::error_with_data(
                request.id.clone(),
                error_map::code_for(ErrorKind::InvalidParams),
                error_map::message_for(ErrorKind::InvalidParams),
                Value::Object(extra),
            )
        }
        Err(ToolError::Degraded) => Response::error(
            request.id.clone(),
            error_map::code_for(ErrorKind::Degraded),
            error_map::message_for(ErrorKind::Degraded),
        ),
        Err(ToolError::Internal { data }) => match data {
            Some(data) => Response::error_with_data(
                request.id.clone(),
                error_map::code_for(ErrorKind::Internal),
                error_map::message_for(ErrorKind::Internal),
                data,
            ),
            None => Response::error(
                request.id.clone(),
                error_map::code_for(ErrorKind::Internal),
                error_map::message_for(ErrorKind::Internal),
            ),
        },
    }
}

/// The one scope resolution every method shares: a blank id is `-32602`
/// `project_id`; a derivation fault is internal (the store rejected a tag the
/// resolver produced).
pub(crate) fn resolve_scope(
    resolver: &dyn crate::scope_resolver::ProjectScopeResolver,
    project_id: &str,
) -> Result<crate::project_scope::ResolvedProjectScope, ToolError> {
    resolver.resolve(project_id).map_err(|error| match error {
        crate::project_scope::ProjectScopeError::BlankProjectId => ToolError::invalid("project_id"),
        crate::project_scope::ProjectScopeError::Core(_) => ToolError::internal(),
    })
}

/// A required, non-blank string parameter; a missing, non-string or
/// whitespace-only value is `-32602` with `reason`.
pub(crate) fn required_string<'a>(
    request: &'a Request,
    name: &str,
    reason: &'static str,
) -> Result<&'a str, ToolError> {
    match request.param(name).and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(ToolError::invalid(reason)),
    }
}

/// An optional string parameter: absent/`null` is `None`, a non-string is
/// `-32602` with `reason`.
pub(crate) fn optional_string<'a>(
    request: &'a Request,
    name: &str,
    reason: &'static str,
) -> Result<Option<&'a str>, ToolError> {
    match request.param(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(ToolError::invalid(reason)),
    }
}

/// An optional boolean parameter: absent/`null` is `None`, a non-boolean is
/// `-32602` with `reason`.
pub(crate) fn optional_bool(
    request: &Request,
    name: &str,
    reason: &'static str,
) -> Result<Option<bool>, ToolError> {
    match request.param(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ToolError::invalid(reason)),
    }
}

/// `scope` = `"project" | "all-projects"`; required on `search`, optional
/// elsewhere (the tool resolves the configured default itself).
pub(crate) fn parse_scope(
    request: &Request,
    required: bool,
    default: mem_core::scope::ScopeSelection,
) -> Result<mem_core::scope::ScopeSelection, ToolError> {
    match request.param("scope") {
        None | Some(Value::Null) => {
            if required {
                Err(ToolError::invalid("scope"))
            } else {
                Ok(default)
            }
        }
        Some(Value::String(text)) => match text.as_str() {
            "project" => Ok(mem_core::scope::ScopeSelection::Project),
            "all-projects" => Ok(mem_core::scope::ScopeSelection::AllProjects),
            _ => Err(ToolError::invalid("scope")),
        },
        Some(_) => Err(ToolError::invalid("scope")),
    }
}

/// `limit` must be an integer in `1..=1000`; absent uses the method default.
pub(crate) fn parse_limit(request: &Request, default: usize) -> Result<usize, ToolError> {
    match request.param("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(number)) => match number.as_u64() {
            Some(value) if (1..=1000).contains(&value) => Ok(value as usize),
            _ => Err(ToolError::invalid("limit")),
        },
        Some(_) => Err(ToolError::invalid("limit")),
    }
}
