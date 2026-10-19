//! mem-ai — AI provider seam: `AiProvider` trait + provider clients + privacy filtering before anything is persisted or sent.
//!
//! [`privacy`] is the single seam guaranteeing that nothing persisted or sent to
//! an embedding/LLM API passes unfiltered: the `<private>` tag stripper, the
//! injected-prompt filter, the file-backed secret resolver, and the
//! [`privacy::layer::PrivacyFilterLayer`] provider middleware.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-ai";

pub mod privacy;
pub mod provider;
