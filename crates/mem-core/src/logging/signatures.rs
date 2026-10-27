//! The nine structured-log signatures `GET /api/stats` scrapes, as ONE
//! public spelling for the writer side.
//!
//! The reader (`crates/mem-web/src/api/stats.rs`) keeps its own literals and is
//! frozen; these constants and that classifier are two spellings of one
//! contract, and `crates/mem-web/tests/stats_signature_contract.rs` is the
//! mandatory drift guard between them (it renders each constant through
//! [`crate::logging::LogSink::log`] and asserts the intended stats cell).
//!
//! Three signatures are emitted by the capture path today; the other six are
//! **structurally dormant** — no production code can reach them until their
//! subsystem exists. Each dormant constant documents the blocking subsystem and
//! is recorded as a gated follow-up in
//! `docs/adr/0014-stats-log-signature-emission.md`.
//!
//! Payload rule: identifiers and filesystem paths only. Never raw context,
//! transcript text, the rejected `agent_role` value, container tags, scope
//! hashes, keys, or tokens.

// ---------------------------------------------------------------------------
// Daily counters (4)
// ---------------------------------------------------------------------------

/// `captures` counter. Emitted by `mem_bridge::capture` on a
/// `Tier1WriteOutcome::Written` tier-1 result — both in
/// `process_entry` (the drain path) and in
/// `flush_entry_with_supplied_summary` (the shutdown `db_flush` path).
///
/// A `Replayed` result emits nothing: the deterministic row already existed,
/// so counting it would double-count a retry after a lost `mark_done`.
pub const AUTO_CAPTURE_MEMORY_PERSISTED: &str = "Auto-capture memory persisted";

/// `captureSkipped` counter. Emitted by `mem_bridge::capture::process_entry`
/// in the `CaptureSummaryOutput::Skip` arm (no supplied summary and no
/// auto-capture context, so neither tier is written).
pub const AUTO_CAPTURE_SKIPPED: &str = "Auto-capture skipped";

/// `profileInjections` counter. Emitted by
/// `crates/mem-bridge/src/inject.rs` (`InjectService::assemble`, the
/// `ProfileInjection::Rendered` arm) — one event per rendered
/// `<user_profile>` block. `data` is `None`: the payload is model-derived
/// profile text and the identity is a git email, so neither is ever logged.
///
/// The `chat.message` hook does not exist in this port; the wrapper applies
/// the returned block, so the injector is the emit site.
pub const PROFILE_INJECT: &str = "profile inject";

/// `compactionInjections` counter. **Dormant:** the engine has no compaction
/// hook (`mem_bridge::engine` exposes capture, inject, recall, health and
/// shutdown); only the pure attribution helper
/// `mem_episodic::attribution::compaction_injection_agent` exists. Follow-up
/// gated on the compaction hook.
pub const COMPACTION_MEMORY_INJECTED: &str = "Compaction memory injected";

// ---------------------------------------------------------------------------
// Named error buckets (5)
// ---------------------------------------------------------------------------

/// `Turso ready gate failed` bucket. **Dormant:** `mem_store::ready` builds
/// `ReadyGate` (`build_ready_gate`, `build_ready_gate_with`,
/// `build_ready_gate_with_encryption`) but has no production caller — the
/// composition opens stores lazily. Follow-up gated on wiring (or formally
/// retiring) the ready pipeline.
pub const TURSO_READY_GATE_FAILED: &str = "Turso ready gate failed";

/// `Failed to load skills for OpenCode registration` bucket. **Dormant:** no
/// host-registration path exists, and `mem-bridge` does not depend on
/// `mem-skills`. Follow-up gated on the skills host-registration subsystem.
pub const FAILED_TO_LOAD_SKILLS: &str = "Failed to load skills for OpenCode registration";

/// `Auto-capture final error` bucket. Emitted by `mem_bridge::capture` for BOTH
/// permanent capture failures: `retryable_or_dead` on the fifth consumed
/// attempt (message carries `after N attempts`, payload `attempts`/`outcome`)
/// and `dead_letter_now` (invalid agent role, invalid or required raw context;
/// payload `reason` is one of the existing fixed constants).
pub const AUTO_CAPTURE_FINAL_ERROR: &str = "Auto-capture final error";

/// `Compaction handler error` bucket. **Dormant:** the same missing compaction
/// handler as [`COMPACTION_MEMORY_INJECTED`]. Follow-up gated on the
/// compaction hook.
pub const COMPACTION_HANDLER_ERROR: &str = "Compaction handler error";

/// `Idle processing error` bucket. **Dormant:**
/// `mem_profile::idle_scheduler`'s `poll_due`/`on_session_idle` have no
/// production caller (the module is deliberately pure). Follow-up gated on a
/// host idle loop.
pub const IDLE_PROCESSING_ERROR: &str = "Idle processing error";
