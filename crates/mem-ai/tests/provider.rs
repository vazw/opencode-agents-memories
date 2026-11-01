//! Provider seam integration suite (blueprint W2.1 / US 4.1).
//!
//! Mirrors the upstream behavioural contracts under a single test binary:
//! - `ai-provider-config.test.ts`  → `provider/ai_provider_config.rs`
//! - `opencode-provider.test.ts`   → `provider/opencode_provider.rs`
//!
//! The directory layout mirrors the blueprint's `tests/provider/` tree; this
//! top-level file is the Cargo test-target entry point that mounts it.

#[path = "provider/ai_provider_config.rs"]
mod ai_provider_config;

#[path = "provider/opencode_provider.rs"]
mod opencode_provider;

#[path = "provider/remote_providers.rs"]
mod remote_providers;
