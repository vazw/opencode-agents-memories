//! Privacy integration suite (blueprint W2.3 / issue 11).
//!
//! Mirrors the upstream behavioral contracts under a single test binary:
//! - `privacy.test.ts`                   → `privacy/privacy_tags.rs`
//! - `privacy-malformed-tags.test.ts`    → `privacy/malformed_tags.rs`
//! - (new) multi-byte slice safety       → `privacy/unicode_boundaries.rs`
//! - `injected-prompt-filter.test.ts`    → `privacy/injected_prompt_filter.rs`
//! - `secret-resolver-permissions.test.ts` → `privacy/secret_resolver_permissions.rs`
//! - (new) `SecretSource` / `SecretString` / shape → `privacy/secret_shape.rs`
//! - (new) middleware behavior           → `privacy/privacy_layer.rs`
//!
//! The directory layout mirrors the blueprint's `tests/privacy/` tree; this
//! top-level file is the Cargo test-target entry point that mounts it.

// The shared fixture module is deliberately instantiated once per mounting file
// (`#[path = "common/mod.rs"] mod common;`), exactly as the blueprint's layout
// requires; that intentional duplication is what `duplicate_mod` flags.
#![allow(clippy::duplicate_mod)]

#[path = "privacy/injected_prompt_filter.rs"]
mod injected_prompt_filter;

#[path = "privacy/injection_text_regions.rs"]
mod injection_text_regions;

#[path = "privacy/malformed_tags.rs"]
mod malformed_tags;

#[path = "privacy/privacy_layer.rs"]
mod privacy_layer;

#[path = "privacy/privacy_tags.rs"]
mod privacy_tags;

#[path = "privacy/secret_resolver_permissions.rs"]
mod secret_resolver_permissions;

#[path = "privacy/secret_shape.rs"]
mod secret_shape;

#[path = "privacy/unicode_boundaries.rs"]
mod unicode_boundaries;
