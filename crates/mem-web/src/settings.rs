//! Configuration mapping for the web server.

/// The web-server slice of the resolved application configuration.
///
/// Secrets are already resolved by mem-core (`resolve_secret_value`); mem-web
/// never re-resolves `env://` / `file://` references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebServerSettings {
    /// Whether the web server may be started at all.
    pub enabled: bool,
    /// The bind host.
    pub host: String,
    /// The bind port.
    pub port: u16,
    /// The plain-text Basic-auth password (empty disables Basic auth).
    pub auth_password: Option<String>,
    /// The plain-text Basic-auth username.
    pub auth_username: Option<String>,
    /// The configured API token for the `/api/*` surface.
    pub api_token: Option<String>,
}

impl WebServerSettings {
    /// Copies the six `web_server_*` fields out of a resolved config.
    pub fn from_config(config: &mem_core::config::Config) -> Self {
        Self {
            enabled: config.web_server_enabled,
            host: config.web_server_host.clone(),
            port: config.web_server_port,
            auth_password: config.web_server_auth_password.clone(),
            auth_username: config.web_server_auth_username.clone(),
            api_token: config.web_server_api_token.clone(),
        }
    }
}
