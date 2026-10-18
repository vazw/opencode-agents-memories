//! Repo-wide typed error strategy.
//!
//! Every crate seam returns `Result<_, ConfigError/LoggingError/Error>` —
//! no stringly-typed errors, no `anyhow`. Structured fields stay structured
//! so callers can branch; `Display` mirrors the upstream user-facing messages.

use std::path::PathBuf;

/// Result alias pinned to the crate-wide [`Error`] type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The single error type crossing crate seams.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// "{$0}" keeps `Display` identical to the wrapped error while `source()`
    /// keeps the full chain hop to the typed variant (thiserror's
    /// `#[error(transparent)]` would forward `source()` past that hop).
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("{0}")]
    Logging(#[from] LoggingError),
    #[error("{0}")]
    Scope(#[from] ScopeError),
}

/// Errors from the memory-scope algebra (`crate::scope`): hash validation,
/// container-tag parsing, scope resolution. `Display` mirrors the upstream
/// user-facing messages byte-exactly.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScopeError {
    #[error("Invalid scope hash: expected 16 lowercase hex characters, got \"{hash}\"")]
    InvalidScopeHash { hash: String },

    #[error(
        "Invalid containerTag: expected format {{prefix}}_{{user|project}}_{{16hex}}, got \"{container_tag}\""
    )]
    InvalidContainerTag { container_tag: String },

    #[error("Invalid containerTag scope: \"{scope}\" in \"{container_tag}\"")]
    InvalidContainerTagScope {
        scope: String,
        container_tag: String,
    },

    #[error(
        "Invalid containerTag hash: expected 16 lowercase hex characters in \"{container_tag}\""
    )]
    InvalidContainerTagHash { container_tag: String },
}

/// Errors produced while loading, validating, or resolving configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error(
        "Project config cannot set remote provider fields: {}. Move them to the global config at {}.",
        fields.join(", "),
        global_config_path.display()
    )]
    ProjectRemoteProviderFields {
        fields: Vec<String>,
        global_config_path: PathBuf,
    },

    #[error("Invalid {key} config: {value}")]
    InvalidValue { key: &'static str, value: String },

    #[error("Invalid {key} config: {value} (expected {expected})")]
    InvalidValueExpected {
        key: &'static str,
        value: String,
        expected: &'static str,
    },

    #[error("invalid config value at {path}")]
    InvalidShape {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("Secret file not found: {path}")]
    SecretFileNotFound { path: PathBuf },

    #[error("Failed to read secret file {}: {}", path.display(), source)]
    SecretFileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Environment variable not found: {name}")]
    SecretEnvNotFound { name: String },

    #[error("invalid JSONC: {source}")]
    Jsonc {
        #[source]
        source: jsonc_parser::errors::ParseError,
    },

    #[error("cannot resolve home directory (HOME/USERPROFILE unset)")]
    MissingHomeDirectory,
}

/// Errors produced by the logging sink / layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoggingError {
    #[error("Failed to write log file {}: {}", path.display(), source)]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("global logging is already installed")]
    AlreadyInstalled,
}
