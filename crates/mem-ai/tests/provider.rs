//! Provider seam integration suite (blueprint W2.1 / US 4.1).
//!
//! Mirrors the three upstream behavioural contracts under a single test
//! binary:
//! - `ai-provider-config.test.ts`  → `provider/ai_provider_config.rs`
//! - `internal-capture-sessions.test.ts` → `provider/internal_capture_sessions.rs`
//! - `opencode-provider.test.ts`   → `provider/opencode_provider.rs`
//!
//! The directory layout mirrors the blueprint's `tests/provider/` tree; this
//! top-level file is the Cargo test-target entry point that mounts it.

use std::sync::{Mutex, MutexGuard};

/// Process-wide serialization for the tests in this binary that touch the
/// global internal-capture set (issue 74): `clear()` wipes every id and
/// `untrack` leaves a 60s grace entry, so two such tests erase each other.
///
/// Hold it for the whole test body, bound as
/// `let _gate = crate::lock_global_capture();`. Acquisition points:
/// - `internal_capture_sessions::test_internal_session_set_global`
/// - `opencode_provider::test_opencode_host_complete_streams`
/// - `opencode_provider::test_opencode_host_structured_output_loop`
/// - `opencode_provider::test_opencode_host_timeout_config`
/// - `opencode_provider::test_2_2_5_create_envelope_yields_the_prompt_call`
/// - `opencode_provider::test_2_2_5_bare_session_id_still_parses`
/// - `opencode_provider::test_2_2_5_envelope_wins_over_a_bare_id`
/// - `opencode_provider::test_2_2_5_create_without_any_id_is_a_wire_shape_error`
/// - `opencode_provider::test_2_2_5_blank_session_id_is_a_wire_shape_error`
///
/// The `complete()` rows reach the global transitively: `OpenCodeHostProvider::new`
/// clones the global handle and every completion `track`s/`untrack`s it.
/// Constructing a provider alone only initialises the `OnceLock`;
/// `InternalCaptureSessions::new` and `with_grace` build their own
/// `Arc<Mutex<State>>`, and only `global()` is shared — so those tests stay
/// parallel.
static GLOBAL_CAPTURE_GATE: Mutex<()> = Mutex::new(());

/// Take the gate; bind it as `let _gate = crate::lock_global_capture();`.
#[must_use = "bind the guard for the whole test body: let _gate = crate::lock_global_capture();"]
pub(crate) fn lock_global_capture() -> MutexGuard<'static, ()> {
    // gate: `()` — a panicking holder loses nothing; recover per ADR-0013 D-2 (D2-R)
    mem_core::sync::lock_or_recover(&GLOBAL_CAPTURE_GATE)
}

#[path = "provider/ai_provider_config.rs"]
mod ai_provider_config;

#[path = "provider/internal_capture_sessions.rs"]
mod internal_capture_sessions;

#[path = "provider/opencode_provider.rs"]
mod opencode_provider;

#[path = "provider/remote_providers.rs"]
mod remote_providers;
