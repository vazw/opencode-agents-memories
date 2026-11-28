//! The ONE identity resolver shared by the web surface and the composition
//! root (hoisted from the root crate's `web_identity`, D-5).
//!
//! Upstream's REST handlers tag a profile request with
//! `getTags(process.cwd()).user.userEmail || "unknown"`
//! (`example/opencode-mem/src/services/api-handlers.ts:1157-1159`). The web
//! surface resolves the **launch cwd**; the inject path resolves the
//! **per-call session directory**. The resolver and the config fields are
//! shared, the directory is not — a host serving several directories may
//! legitimately show two identities, and no gate may assert a single one.
//!
//! Why this module exists: a second precedence chain in `composition.rs` (or a
//! hardcoded `"unknown"` at a wiring site) is the same bug with more code. Both
//! the composition root and the root crate call [`resolve_user_id_with`].
//!
//! Total by construction: a missing `git`, a non-repo directory, an unreadable
//! config or a non-UTF-8 path yields [`UNKNOWN_USER_ID`]. A panic here would
//! take the web surface down with a stdio engine attached.

use std::path::Path;

use mem_core::config::Config;

use crate::scope_resolver::ProjectScopeResolver;

/// The identity used when nothing resolves (upstream's literal).
pub const UNKNOWN_USER_ID: &str = "unknown";

/// Resolves a user identity against an injected resolver.
///
/// Precedence, exactly the inject path's:
/// 1. `config.user_email_override`, trimmed and non-empty;
/// 2. the resolver's `user_email` for `directory`, trimmed and non-empty;
/// 3. [`UNKNOWN_USER_ID`].
///
/// The resolver is asked about `directory` verbatim (as a UTF-8 string, the
/// form `ProjectScopeResolver` takes); it is never asked about `.`, the cwd or
/// the storage root.
pub fn resolve_user_id_with(
    config: &Config,
    resolver: &dyn ProjectScopeResolver,
    directory: &Path,
) -> String {
    if let Some(email) = trimmed(config.user_email_override.as_deref()) {
        return email;
    }
    // `to_string_lossy` is total: a non-UTF-8 path becomes a (non-matching)
    // lossy string rather than a panic, and the store simply finds no scope.
    let directory = directory.to_string_lossy();
    match resolver.resolve(&directory) {
        Ok(scope) => {
            trimmed(scope.user_email.as_deref()).unwrap_or_else(|| UNKNOWN_USER_ID.to_string())
        }
        Err(_) => UNKNOWN_USER_ID.to_string(),
    }
}

/// Trims, then drops an empty value: a whitespace-only email is not an
/// identity (upstream's `||` chain never sees `"   "` as truthy-but-blank).
fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
