//! Making the embedder's token loss visible on the host's diagnostics channel.
//!
//! `mem-embed` never prints; its per-input rate lives in the engine's file
//! sink (`embedding.input_truncated`, target `mem_core::logging`). The bridge
//! adds two host-facing surfaces on the existing `structured_stderr` NDJSON
//! channel:
//!
//! - once per engine process: `embedding.input_truncated` with the counters,
//!   the first time a tool/capture/inject path observes a truncation;
//! - once per CLI run: `reembed.input_truncated` with the run's totals.
//!
//! [`TruncationStats`](mem_embed::TruncationStats) behind
//! [`Embedder::truncation_stats`] is what both read; the remote lane returns
//! `None` and neither surface fires.

use std::sync::Once;

use mem_embed::embedder::{Embedder, TruncationSnapshot};

use crate::engine::structured_stderr;

/// The stderr payload for a snapshot, or `None` when nothing was truncated.
pub fn payload(snapshot: &TruncationSnapshot) -> Option<serde_json::Value> {
    if snapshot.truncated_inputs == 0 {
        return None;
    }
    Some(serde_json::json!({
        "cap_tokens": snapshot.cap_tokens,
        "truncated_inputs": snapshot.truncated_inputs,
        "dropped_tokens": snapshot.dropped_tokens,
    }))
}

/// The process-wide "announce the first truncation" latch.
pub struct TruncationReporter {
    fired: Once,
}

impl TruncationReporter {
    /// A fresh latch (the production one is [`REPORTER`]; tests use their own).
    pub fn new() -> Self {
        Self { fired: Once::new() }
    }

    /// Emit the first truncation this process observes, at most once per
    /// reporter, and return the payload it emitted (`None` when there was
    /// nothing to say or it was already said).
    pub fn report_first(
        &self,
        embedder: &dyn Embedder,
        component: &str,
    ) -> Option<serde_json::Value> {
        let payload = payload(&embedder.truncation_stats()?)?;
        let mut emitted = None;
        self.fired.call_once(|| {
            structured_stderr(
                "embedding.input_truncated",
                "warn",
                component,
                payload.clone(),
            );
            emitted = Some(payload.clone());
        });
        emitted
    }

    /// Emit the run's totals unconditionally when non-zero (the CLI path) and
    /// return the payload.
    pub fn report_totals(
        embedder: &dyn Embedder,
        component: &str,
        event: &str,
    ) -> Option<serde_json::Value> {
        let payload = payload(&embedder.truncation_stats()?)?;
        structured_stderr(event, "warn", component, payload.clone());
        Some(payload)
    }
}

impl Default for TruncationReporter {
    fn default() -> Self {
        Self::new()
    }
}

/// The engine's single reporter: one `embedding.input_truncated` line per
/// process (the per-input rate is in the file sink).
pub static REPORTER: TruncationReporter = TruncationReporter { fired: Once::new() };
