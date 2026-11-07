//! The shared structured-stderr diagnostics emitter (ADR-0031).
//!
//! One function, one byte format, every process direction that answers on the
//! engine's stderr NDJSON seam: the wrapper fans engine stderr lines into the
//! host log and its message-slot grammar admits only event names matching
//! `[A-Za-z0-9_.-]` (a refused name falls back to the generic `engine.stderr`
//! slot), so emitters keep the dot-separated house style
//! (`capture.refused`, `profile.decay_failed`, `api.handler_failed`).
//!
//! Privacy posture (ADR-0015 D-1; issue 129's fixed-token precedent): callers
//! pass identifiers and fixed tokens only — never raw fault text, paths,
//! request params or secrets. `capture` params, for one, include the
//! transcript, and a store fault's message is engine IO text.

use std::io::Write;

/// The fixed fallback emitted when the diagnostics payload cannot be
/// serialized: still one valid JSON object, so the NDJSON framing survives
/// (ADR-0013 D-3).
const STDERR_SERIALIZE_FALLBACK: &str =
    r#"{"component":"bridge","event":"stderr.serialize_failed","level":"error"}"#;

/// Writes one structured-JSON diagnostics line to stderr.
///
/// Only the named level/component/event and the explicit extras are emitted:
/// no request params ever reach here (capture params include the transcript).
/// Infallible: a diagnostics failure must never fail the operation it
/// reports.
pub fn structured_stderr(event: &str, level: &str, component: &str, extras: serde_json::Value) {
    let mut payload = serde_json::json!({
        "component": component,
        "event": event,
        "level": level,
    });
    if let (serde_json::Value::Object(base), serde_json::Value::Object(more)) =
        (&mut payload, &extras)
    {
        for (key, value) in more {
            base.insert(key.clone(), value.clone());
        }
    }
    // stderr logger: a serialization failure emits a fixed valid JSON line, never breaking NDJSON framing (ADR-0013 D-3)
    let mut line = match serde_json::to_string(&payload) {
        Ok(line) => line,
        Err(error) => {
            tracing::error!(error = %error, "structured stderr payload failed to serialize");
            STDERR_SERIALIZE_FALLBACK.to_string()
        }
    };
    line.push('\n');
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.flush();
}
