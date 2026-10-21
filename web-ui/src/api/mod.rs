//! The API surface: the auth seam and the typed envelope.
//!
//! There are no Leptos server functions: this module only shapes browser
//! requests to the hand-written axum REST surface.

pub mod auth;
pub mod envelope;

pub use crate::api_request::build_url;
pub use auth::{AuthContext, BasicCredential};
pub use envelope::{ApiEnvelope, ApiError, parse_envelope};
