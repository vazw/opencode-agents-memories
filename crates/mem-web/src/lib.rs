//! mem-web — axum web explorer backend: upstream REST contract + new session-history endpoints.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-web";

/// The header carrying the local API token (upstream `auth-token.ts`).
pub const AUTH_HEADER: &str = "x-opencode-mem-token";

pub mod api;
pub mod api_auth;
pub mod cors;
pub mod host_allowlist;
pub mod lifecycle;
pub mod server;
pub mod static_hosting;
pub mod web_auth;

mod error;
mod gate;
mod health;
mod settings;

pub use error::WebError;
pub use health::{
    HEALTH_PROBE_EXPECTED_STATUS, HEALTH_PROBE_STATUS_FIELD, HEALTH_PROBE_SUCCESS_FIELD,
    is_valid_health_probe_envelope,
};
pub use settings::WebServerSettings;
