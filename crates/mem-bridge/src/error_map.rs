//! JSON-RPC silicon error map (ADR-0007 D-44 + the -32002 amendment item).
//!
//! Every ordinary engine failure maps to a *specific* code:
//!
//! | code | kind | meaning |
//! |---|---|---|
//! | `-32700` | [`ErrorKind::Parse`] | the line is not valid JSON |
//! | `-32600` | [`ErrorKind::InvalidRequest`] | not a JSON-RPC 2.0 object / batched array / missing id-method |
//! | `-32601` | [`ErrorKind::UnknownMethod`] | unknown method |
//! | `-32602` | [`ErrorKind::MissingSessionId`]/[`ErrorKind::BlankSessionId`]/[`ErrorKind::OverlongSessionId`]/[`ErrorKind::InvalidParams`] | invalid params |
//! | `-32603` | [`ErrorKind::Internal`] | an unexpected engine failure |
//! | `-32001` | [`ErrorKind::Degraded`] | engine runs, memory disabled (`MEM_DEGRADED`) |
//! | `-32002` | [`ErrorKind::InboxFull`] | capture inbox full (`MEM_INBOX_FULL`, W5.3's number) |
//!
//! The bare -32603 is *never* substituted for a code a specific kind owns.

/// The specific failure kinds the engine maps to codes and messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The line was not valid JSON.
    Parse,
    /// Not a JSON-RPC 2.0 request object (or batched array, missing id/method).
    InvalidRequest,
    /// Unknown method.
    UnknownMethod,
    /// Missing top-level `session_id`.
    MissingSessionId,
    /// Whitespace-only top-level `session_id`.
    BlankSessionId,
    /// `session_id` over the 128-character bound.
    OverlongSessionId,
    /// Generic invalid params (none of the session-id specifics).
    InvalidParams,
    /// An unexpected engine failure.
    Internal,
    /// The engine runs but memory is disabled.
    Degraded,
    /// The capture inbox is full (never a silent drop).
    InboxFull,
}

/// The specific messages in `tests/fixtures/bridge/error-*.json`.
pub fn message_for(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Parse => "Parse error",
        ErrorKind::InvalidRequest => "Invalid Request",
        ErrorKind::UnknownMethod => "Method not found",
        ErrorKind::MissingSessionId
        | ErrorKind::BlankSessionId
        | ErrorKind::OverlongSessionId
        | ErrorKind::InvalidParams => "Invalid params",
        ErrorKind::Internal => "Internal error",
        ErrorKind::Degraded => "MEM_DEGRADED",
        ErrorKind::InboxFull => "MEM_INBOX_FULL",
    }
}

/// The specific codes in `tests/fixtures/bridge/error-*.json`.
pub fn code_for(kind: ErrorKind) -> i32 {
    match kind {
        ErrorKind::Parse => -32700,
        ErrorKind::InvalidRequest => -32600,
        ErrorKind::UnknownMethod => -32601,
        ErrorKind::MissingSessionId
        | ErrorKind::BlankSessionId
        | ErrorKind::OverlongSessionId
        | ErrorKind::InvalidParams => -32602,
        ErrorKind::Internal => -32603,
        ErrorKind::Degraded => -32001,
        ErrorKind::InboxFull => -32002,
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorKind, code_for, message_for};

    #[test]
    fn test_error_map_code_for_unknown_method() {
        assert_eq!(code_for(ErrorKind::UnknownMethod), -32601);
        assert_eq!(message_for(ErrorKind::UnknownMethod), "Method not found");
    }

    #[test]
    fn test_error_map_code_for_missing_session_id() {
        assert_eq!(code_for(ErrorKind::MissingSessionId), -32602);
        assert_eq!(message_for(ErrorKind::MissingSessionId), "Invalid params");
    }

    #[test]
    fn test_error_map_code_for_blank_session_id() {
        assert_eq!(code_for(ErrorKind::BlankSessionId), -32602);
        assert_eq!(message_for(ErrorKind::BlankSessionId), "Invalid params");
    }

    #[test]
    fn test_error_map_code_for_overlong_session_id() {
        assert_eq!(code_for(ErrorKind::OverlongSessionId), -32602);
        assert_eq!(message_for(ErrorKind::OverlongSessionId), "Invalid params");
    }

    #[test]
    fn test_error_map_code_for_degraded() {
        assert_eq!(code_for(ErrorKind::Degraded), -32001);
        assert_eq!(message_for(ErrorKind::Degraded), "MEM_DEGRADED");
    }

    #[test]
    fn test_error_map_code_for_inbox_full() {
        assert_eq!(code_for(ErrorKind::InboxFull), -32002);
        assert_eq!(message_for(ErrorKind::InboxFull), "MEM_INBOX_FULL");
    }

    #[test]
    fn test_error_map_never_returns_generic_32603() {
        // Every specific kind answers with its specific code — the generic
        // -32603 is reserved for Internal alone.
        for kind in [
            ErrorKind::UnknownMethod,
            ErrorKind::MissingSessionId,
            ErrorKind::BlankSessionId,
            ErrorKind::OverlongSessionId,
            ErrorKind::InvalidParams,
            ErrorKind::Degraded,
            ErrorKind::InboxFull,
            ErrorKind::Parse,
            ErrorKind::InvalidRequest,
        ] {
            assert_ne!(
                code_for(kind),
                -32603,
                "{kind:?} must not fall through to the generic internal error"
            );
        }
        assert_eq!(code_for(ErrorKind::Internal), -32603);
    }
}
