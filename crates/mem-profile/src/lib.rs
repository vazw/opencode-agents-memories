//! mem-profile — passive user-profile learning: prompt cold buffer, extraction, confidence/decay, AI cleanup.
//!
//! W3.1 (US 3.1, locking half) port layout:
//! - [`prompt`] — `UserPromptManager` (user-prompts.db claim state machine) +
//!   the privacy-gated `save_prompt` write path.
//! - [`cold_buffer`] — `cold-buffer.json` snapshot store: strict fail-closed
//!   loads, atomic tmp+rename short-write-hardened saves.
//! - [`profile_store`] — `ProfileStore` over `user-profiles.db` +
//!   `merge_profile_data` with the `EmbedBoundary` seam.
//! - [`learning_lock`] — cross-process coordination-DB CAS lock
//!   (`.profile-learning-coordination.db`).
//! - [`learning_service`] — the lock → count → select → analyze → merge →
//!   update → mark round behind injectable traits.
//! - [`idle_scheduler`] — pure, clock-injected 10 s idle debounce.
//! - [`erasure`] — the B2′ vocabulary: the watermark predicate, the
//!   `ProfileEraser` seam `delete_prompt` calls before it deletes the source
//!   row, and the two-table `EraseReport`.
//!
//! Upstream reference: `example/opencode-mem/src/services/user-prompt/*`,
//! `src/services/user-profile/*`, `src/services/user-memory-learning.ts`
//! (READ-ONLY behavioural contract).

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-profile";

pub mod ai_cleanup;
pub mod analysis_prompt;
pub mod cleanup_apply;
pub mod cold_buffer;
pub mod decay;
pub mod erasure;
pub mod error;
pub mod idle_scheduler;
pub mod ids;
pub mod incremental_cleanup;
pub mod language_detector;
pub mod learning_lock;
pub mod learning_service;
pub mod profile_store;
pub mod profile_utils;
pub mod prompt;
pub mod provider_gateway;
pub mod validation;

pub use error::ProfileError;
