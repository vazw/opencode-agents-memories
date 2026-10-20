//! API-surface auth — upstream `src/services/web-api-auth.ts` and
//! `src/services/auth-token.ts`.

use std::io::Write;
use std::path::{Path, PathBuf};

use subtle::ConstantTimeEq;

use crate::error::WebError;

/// The hosts that are considered loopback.
const LOOPBACK_HOSTS: [&str; 5] = ["127.0.0.1", "localhost", "::1", "[::1]", "0:0:0:0:0:0:0:1"];

/// The number of random bytes behind a local token.
const LOCAL_TOKEN_BYTES: usize = 32;

/// Whether the host is a loopback address/name.
pub fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().to_ascii_lowercase();
    LOOPBACK_HOSTS.contains(&normalized.as_str())
}

/// Refuses a network-exposed bind that has neither an API token nor a Basic
/// password.
///
/// The refusal happens before any socket is created, so a misconfigured
/// server never exposes the API.
pub fn assert_network_auth(
    host: &str,
    api_token: Option<&str>,
    basic_enabled: bool,
) -> Result<(), WebError> {
    if is_loopback_host(host) {
        return Ok(());
    }
    // Upstream truthiness: an empty token string counts as "not configured".
    let has_api_token = api_token.is_some_and(|token| !token.is_empty());
    if has_api_token || basic_enabled {
        return Ok(());
    }
    Err(WebError::NetworkAuthRequired {
        host: host.to_owned(),
    })
}

/// Whether the request carries the configured API token.
///
/// A missing (or empty) configured token means the API is not token-gated at
/// all, so every request passes. Otherwise the token may arrive as
/// `Authorization: Bearer <token>` or as the `x-opencode-mem-token` header.
pub fn authorize_api_request(
    authorization: Option<&str>,
    x_opencode_mem_token: Option<&str>,
    api_token: Option<&str>,
) -> bool {
    let Some(api_token) = api_token else {
        return true;
    };
    if api_token.is_empty() {
        return true;
    }

    let bearer = authorization.and_then(bearer_token);
    let alternative = x_opencode_mem_token.map(str::trim);
    let provided = match bearer {
        Some(token) => Some(token),
        None => alternative
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
    };
    let Some(provided) = provided else {
        return false;
    };
    bool::from(api_token.as_bytes().ct_eq(provided.as_bytes()))
}

/// Whether the `x-opencode-mem-token` header equals the local token
/// byte-for-byte (upstream `provided === token` — no trimming, and a
/// configured-but-unavailable token never authorizes anything).
pub(crate) fn local_token_matches(provided: Option<&str>, expected: Option<&str>) -> bool {
    match (provided, expected) {
        (Some(provided), Some(expected)) => {
            bool::from(expected.as_bytes().ct_eq(provided.as_bytes()))
        }
        _ => false,
    }
}

/// Extracts the bearer token from an `Authorization` header.
///
/// Mirrors upstream's `/^Bearer\s+(.+)$/i` match.
fn bearer_token(authorization: &str) -> Option<String> {
    let trimmed = authorization.trim();
    let scheme_end = trimmed.find(char::is_whitespace)?;
    if !trimmed[..scheme_end].eq_ignore_ascii_case("bearer") {
        return None;
    }
    let rest = trimmed[scheme_end..].trim_start();
    if rest.is_empty() {
        return None;
    }
    Some(rest.trim().to_owned())
}

/// The source of the local (per-installation) API token.
pub trait LocalTokenSource: Send + Sync {
    /// The current token value, or `None` when it cannot be produced.
    fn token(&self) -> Option<String>;
}

/// A local token backed by a file on disk.
#[derive(Debug, Clone)]
pub struct FileLocalToken {
    path: PathBuf,
}

impl FileLocalToken {
    /// Creates a file-backed local token source.
    pub fn new(path: PathBuf) -> FileLocalToken {
        FileLocalToken { path }
    }

    /// The path the token lives at.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl LocalTokenSource for FileLocalToken {
    /// The current token value.
    ///
    /// A file whose trimmed content is non-empty is reused verbatim and is
    /// never rewritten. An absent or blank file is (re)generated: 32 random
    /// bytes, hex-encoded, written with `create_new` and mode `0600`.
    fn token(&self) -> Option<String> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) if !content.trim().is_empty() => {
                return Some(content.trim().to_owned());
            }
            Ok(_) => {
                // Blank content: the file must be removed before the
                // `create_new` write below, which would otherwise fail with
                // `AlreadyExists`.
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Absent: create it below.
            }
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    kind = %error.kind(),
                    "cannot read the local token file"
                );
                return None;
            }
        }

        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                path = %self.path.display(),
                kind = %error.kind(),
                "cannot create the local token directory"
            );
            return None;
        }

        let mut bytes = [0_u8; LOCAL_TOKEN_BYTES];
        if let Err(error) = getrandom::fill(&mut bytes) {
            tracing::warn!(
                path = %self.path.display(),
                kind = %error,
                "cannot generate the local token"
            );
            return None;
        }
        let token = hex_encode(&bytes);

        if self.path.exists()
            && let Err(error) = std::fs::remove_file(&self.path)
        {
            tracing::warn!(
                path = %self.path.display(),
                kind = %error.kind(),
                "cannot remove the blank local token file"
            );
            return None;
        }

        match write_new_token_file(&self.path, &token) {
            Ok(()) => Some(token),
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    kind = %error.kind(),
                    "cannot create the local token file"
                );
                None
            }
        }
    }
}

/// Creates the token file with `create_new` and mode `0600` on unix.
fn write_new_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(token.as_bytes())?;
    file.flush()
}

/// Lowercase hex encoding of raw bytes.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}
