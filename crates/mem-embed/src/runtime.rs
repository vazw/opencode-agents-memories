//! ONNX Runtime environment access.
//!
//! ort 2.0.0-rc.14 allows exactly one environment per process (`CURRENT_ENV`):
//! a second `ort::init().build()` fails with "only one environment is allowed
//! per process". Every session in this crate therefore goes through this
//! single `OnceLock` — the sole builder call site in the process. The cached
//! value is the `Result` itself, so a load failure is sticky (no retry loop)
//! and `Environment` clones are cheap (Arc-backed).
//!
//! Divergence from upstream (recorded): the `onnxruntime-resolve.ts` shim
//! machinery (binding-path probing, `require` redirection, pinned
//! `onnxruntime-node` root) is N-A here — `ort` statically links ONNX Runtime,
//! so there is no binding file to resolve. What is preserved is the actionable
//! failure text: an operator whose runtime cannot load is pointed at the
//! remote provider configuration.

use std::sync::OnceLock;

use crate::error::EmbedError;

/// The process-wide environment (initialized on first use).
static ENV: OnceLock<Result<ort::environment::Environment, EmbedError>> = OnceLock::new();

/// The one ONNX Runtime environment for this process.
///
/// Errors are cached: a process whose runtime cannot be initialized will not
/// re-attempt the (expensive, deterministic) load on every call.
pub fn ort_environment() -> Result<ort::environment::Environment, EmbedError> {
    ENV.get_or_init(|| {
        ort::init()
            .with_name("opencode-mem")
            .build()
            // The raw cause goes into `message`: `EmbedError::RuntimeInit`'s
            // Display already renders the actionable fallback around it.
            .map_err(|error| EmbedError::RuntimeInit {
                message: error.to_string(),
            })
    })
    .clone()
}

/// Render the user-facing runtime-init message for a raw runtime load cause
/// (upstream `formatOnnxruntimeInitError` shape: cause + actionable fallback).
pub fn format_runtime_init_error(message: &str) -> String {
    format!(
        "ONNX runtime failed to load: {message}. Set embeddingApiUrl and embeddingApiKey to use a remote embedding provider instead."
    )
}
