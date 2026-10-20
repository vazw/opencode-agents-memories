//! Typed errors for the web server skeleton.

/// Failures raised by the web server bootstrap and lifecycle.
///
/// The `NetworkAuthRequired` display text is byte-identical to the upstream
/// `assertWebServerNetworkAuth` message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebError {
    /// The configured bind host is not loopback and no API token / Basic
    /// password is configured, so the API would be exposed on the network.
    #[error(
        "webServerHost \"{host}\" exposes the API on the network. Set webServerApiToken in opencode-mem.jsonc, or bind to 127.0.0.1."
    )]
    NetworkAuthRequired {
        /// The offending host, as configured.
        host: String,
    },

    /// The requested address is already taken (fail-fast per D-1).
    #[error("address {host}:{port} is already in use")]
    AddrInUse {
        /// The host that was bound.
        host: String,
        /// The port that was bound.
        port: u16,
    },

    /// The listener could not be created.
    #[error("failed to bind {host}:{port}: {source}")]
    Bind {
        /// The host that was bound.
        host: String,
        /// The port that was bound.
        port: u16,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The server task failed while serving.
    #[error("web server failed: {source}")]
    Serve {
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Builds a JSON error envelope with the upstream key order.
///
/// `serde_json::Map` sorts its keys, so the envelope is assembled directly to
/// keep the upstream byte order (`success` before `error`).
pub(crate) fn error_envelope(message: &str) -> String {
    let escaped = serde_json::to_string(message).expect("a string is always serializable");
    format!(r#"{{"success":false,"error":{escaped}}}"#)
}
