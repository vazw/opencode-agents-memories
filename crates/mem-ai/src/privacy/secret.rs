//! Secret resolution (upstream `src/services/secret-resolver.ts`) plus the
//! `mem-ai` hygiene types.
//!
//! [`FileSecretResolver`] implements the **W2.1** [`SecretResolver`] trait —
//! which is defined in `provider::secret_resolver` and consumed by
//! `provider::remote::auth` — rather than a competing interface (D1).
//!
//! # Residual exposure (D1)
//!
//! [`SecretString`] is this module's own API: it zeroizes its buffer on drop
//! and redacts `Debug`. The trait, however, returns `Option<String>`; the impl
//! therefore unconceals with [`SecretString::expose_secret`] and hands a plain
//! `String` to the HTTP header builder, which owns that copy for the duration
//! of the request. Retyping the trait would touch the already-closed W2.1/W2.2
//! waves, which issue 11 forbids.

use std::fmt;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use mem_core::config::{expand_path, is_placeholder_api_key};
use mem_core::env::{EnvSource, ProcessEnv, home_dir};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::provider::error::ProviderError;
use crate::provider::secret_resolver::SecretResolver;

/// A parsed secret reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// `env://NAME`.
    Env { var: String },
    /// `file://PATH` (already un-prefixed; `~` is expanded at resolution time).
    File { path: PathBuf },
}

impl SecretSource {
    /// Parses a reference; `None` means the value is a literal, not a
    /// reference (upstream's fall-through case).
    pub fn parse(reference: &str) -> Option<Self> {
        if let Some(var) = reference.strip_prefix("env://") {
            return Some(Self::Env {
                var: var.to_owned(),
            });
        }
        if let Some(path) = reference.strip_prefix("file://") {
            return Some(Self::File {
                path: PathBuf::from(path),
            });
        }
        None
    }

    /// The canonical reference string for this source (round-trips
    /// [`SecretSource::parse`]).
    pub fn reference(&self) -> String {
        match self {
            Self::Env { var } => format!("env://{var}"),
            Self::File { path } => format!("file://{}", path.display()),
        }
    }
}

/// A resolved secret value that zeroizes its buffer on drop and redacts
/// `Debug` (D1, upstream "never log a secret").
///
/// `zeroize`'s `derive` feature is unavailable in this offline registry (its
/// `zeroize_derive ^1.5` dependency cannot be resolved), so the two impls are
/// written by hand — same observable contract: zeroize-on-drop, redacting
/// `Debug`.
pub struct SecretString(String);

impl SecretString {
    /// Wrap a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the plaintext.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Zeroize the buffer in place; the value is empty afterwards.
    pub fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

impl Zeroize for SecretString {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl ZeroizeOnDrop for SecretString {}

/// Shape-validates a secret value (D7).
///
/// Conservative malformed-content rejection — never a vendor allow-list, since
/// Atlas Cloud / OrcaRouter accept arbitrary key shapes. The error names the
/// *reason* only, never the value.
pub fn validate_secret_shape(value: &str) -> Result<(), ProviderError> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        return Err(ProviderError::config("secret value is empty"));
    }
    if is_placeholder_api_key(Some(trimmed)) {
        return Err(ProviderError::config(
            "secret value is a placeholder, not a real key",
        ));
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ProviderError::config(
            "secret value contains whitespace or control characters",
        ));
    }
    if trimmed.len() < 8 {
        return Err(ProviderError::config(
            "secret value is shorter than 8 characters",
        ));
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with('"') {
        return Err(ProviderError::config(
            "secret value looks like a serialized document",
        ));
    }

    Ok(())
}

/// Whether file permission bits are enforced (D2).
///
/// Upstream returns early on `win32`; the port models that as an explicit,
/// testable policy so the skip path stays exercisable on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// Reject files whose group/other bits are set.
    Enforce,
    /// Skip the check (upstream's win32 early return).
    Skip,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        if cfg!(unix) {
            Self::Enforce
        } else {
            Self::Skip
        }
    }
}

/// Group/other permission bits of a POSIX mode.
///
/// The property is the *bitmask*, never the numeric value: `0o404`
/// (world-readable) sorts numerically below `0o600`, and `0o640` — strictly
/// less permissive than `0o644` — used to be the only one warned about.
/// File-type bits above `0o777` are masked off here, so this predicate is
/// correct on the mocked mode values upstream's suite uses.
pub fn group_or_other_bits(mode: u32) -> u32 {
    mode & 0o077
}

/// The permission bits of `metadata`, masked to `0o777`.
#[cfg(unix)]
fn permission_bits(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

/// Non-unix fallback: POSIX mode bits are not meaningful there.
#[cfg(not(unix))]
fn permission_bits(_metadata: &std::fs::Metadata) -> u32 {
    0
}

/// The file-backed [`SecretResolver`] (W2.3's production implementation).
///
/// Blocking file IO inside the async `resolve` is deliberate and bounded; see
/// [`FileSecretResolver::MAX_SECRET_BYTES`].
pub struct FileSecretResolver {
    env: Arc<dyn EnvSource + Send + Sync>,
    policy: PermissionPolicy,
}

impl FileSecretResolver {
    /// Maximum accepted secret-file size.
    pub const MAX_SECRET_BYTES: u64 = 64 * 1024;

    /// A resolver reading the real process environment, enforcing permissions.
    pub fn new() -> Self {
        Self {
            env: Arc::new(ProcessEnv),
            policy: PermissionPolicy::default(),
        }
    }

    /// Replace the environment seam (tests inject a map; production never
    /// mutates the process environment).
    pub fn with_env(mut self, env: Arc<dyn EnvSource + Send + Sync>) -> Self {
        self.env = env;
        self
    }

    /// Override the permission policy.
    pub fn with_permission_policy(mut self, policy: PermissionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The active permission policy.
    pub fn permission_policy(&self) -> PermissionPolicy {
        self.policy
    }

    /// Resolve a parsed source.
    pub fn resolve_source(
        &self,
        source: &SecretSource,
    ) -> Result<Option<SecretString>, ProviderError> {
        match source {
            SecretSource::Env { var } => {
                match self.env.get(var).filter(|value| !value.is_empty()) {
                    // D3: an absent variable is "not configured", not an error.
                    None => Ok(None),
                    Some(value) => {
                        validate_secret_shape(&value)?;
                        Ok(Some(SecretString::new(value)))
                    }
                }
            }
            SecretSource::File { path } => {
                let home = home_dir(self.env.as_ref());
                let needs_home =
                    path.to_string_lossy().starts_with("~/") || path.to_string_lossy() == "~";
                let expanded = match (&home, needs_home) {
                    (Some(home), _) => expand_path(&path.to_string_lossy(), home),
                    (None, true) => {
                        return Err(ProviderError::config(format!(
                            "cannot resolve home directory for secret file {}",
                            path.display()
                        )));
                    }
                    (None, false) => path.clone(),
                };

                self.resolve_file(&expanded)
            }
        }
    }

    /// The strict file pipeline (D2) — every message names the path and nothing
    /// else: never the value, never errno text.
    fn resolve_file(&self, expanded: &Path) -> Result<Option<SecretString>, ProviderError> {
        // 1. Open. The taxonomy is a fixed, tested mapping: `NotFound` gets
        //    upstream's exact sentence; everything else (EACCES, ELOOP,
        //    EISDIR, …) gets the path-only "could not be opened" error. The io
        //    error rides `with_source`, so it stays diagnosable without
        //    entering the message.
        let file = match File::open(expanded) {
            Ok(file) => file,
            Err(error) => {
                let message = if error.kind() == ErrorKind::NotFound {
                    format!("Secret file not found: {}", expanded.display())
                } else {
                    format!("Secret file could not be opened: {}", expanded.display())
                };
                return Err(ProviderError::config(message).with_source(error));
            }
        };

        // 2. Metadata from the *open handle*, never a separate `stat` by path —
        //    this closes the check/read TOCTOU. Symlinks are followed, matching
        //    upstream `statSync`.
        let metadata = file.metadata().map_err(|error| {
            let message = if error.kind() == ErrorKind::NotFound {
                format!("Secret file not found: {}", expanded.display())
            } else {
                format!("Secret file could not be opened: {}", expanded.display())
            };
            ProviderError::config(message).with_source(error)
        })?;

        // 3. Regular files only: directories, sockets and FIFOs are rejected
        //    deterministically, so `EISDIR` never has to be relied on.
        if !metadata.is_file() {
            return Err(ProviderError::config(format!(
                "Secret file is not a regular file: {}",
                expanded.display()
            )));
        }

        // 4. Permission check (D2): any group/other bit rejects the file.
        let mode = permission_bits(&metadata);
        if self.policy == PermissionPolicy::Enforce && group_or_other_bits(mode) != 0 {
            return Err(ProviderError::config(format!(
                "Secret file {} has permissive permissions ({mode:03o}) granting group/other access. Recommend chmod 600.",
                expanded.display()
            )));
        }

        // 5. Bounded read. `metadata.len()` is advisory — a file can grow
        //    between the stat and the read — so the cap is the take-bound, not
        //    a length pre-check: allocation stops at MAX + 1 bytes whatever the
        //    file does afterwards.
        let mut buffer = Vec::new();
        let mut limited = (&file).take(Self::MAX_SECRET_BYTES + 1);
        limited.read_to_end(&mut buffer).map_err(|error| {
            ProviderError::config(format!(
                "Secret file could not be read: {}",
                expanded.display()
            ))
            .with_source(error)
        })?;
        if buffer.len() as u64 > Self::MAX_SECRET_BYTES {
            return Err(ProviderError::config(format!(
                "Secret file exceeds the {} byte limit: {}",
                Self::MAX_SECRET_BYTES,
                expanded.display()
            )));
        }

        // 6. Lossy decode mirrors Node's replacement decoding; the value is
        //    trimmed of surrounding whitespace/newlines. An explicitly
        //    configured but empty value is a shape error, not "not configured"
        //    (only an absent env var means that — D3).
        let content = String::from_utf8_lossy(&buffer).trim().to_owned();
        validate_secret_shape(&content)?;
        Ok(Some(SecretString::new(content)))
    }

    /// Resolve a raw reference: `env://…`, `file://…`, or a literal value.
    pub fn resolve_reference(
        &self,
        reference: &str,
    ) -> Result<Option<SecretString>, ProviderError> {
        match SecretSource::parse(reference) {
            Some(source) => self.resolve_source(&source),
            None => {
                validate_secret_shape(reference)?;
                Ok(Some(SecretString::new(reference)))
            }
        }
    }
}

impl Default for FileSecretResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretResolver for FileSecretResolver {
    /// Resolve `reference` through the W2.1 trait.
    ///
    /// The plaintext is unconcealed for the caller (D1): see the residual
    /// exposure note in the module docs.
    async fn resolve(&self, reference: &str) -> Result<Option<String>, ProviderError> {
        Ok(self
            .resolve_reference(reference)?
            .map(|secret| secret.expose_secret().to_owned()))
    }
}
