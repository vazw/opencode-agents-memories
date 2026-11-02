//! The ONE web-side identity resolver (issue 115).
//!
//! Upstream's REST handlers tag a profile request with
//! `getTags(process.cwd()).user.userEmail || "unknown"`
//! (`example/opencode-mem/src/services/api-handlers.ts:1157-1159`). The web
//! surface therefore resolves the **launch cwd**; the inject path resolves the
//! **per-call session directory**. The resolver and the config fields are
//! shared, the directory is not — a host serving several directories may
//! legitimately show two identities, and no gate may assert a single one.
//!
//! Why this module exists at all: the composition root used to hardcode the
//! unknown identity at the `AppState` wiring site, so the profile page
//! reported "no profile" while the store held real rows. A second resolver in
//! the web layer would have been the same bug with more code, so the call goes
//! through [`mem_bridge::composition::EnvResolver`] — the exact
//! `ProjectScopeResolver` `composition.rs` hands to capture and inject.
//!
//! Total by construction: a missing `git`, a non-repo directory, an unreadable
//! config or a non-UTF-8 path yields [`UNKNOWN_USER_ID`]. A panic here would
//! take the web surface down with a stdio engine attached.

use std::path::Path;

use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_core::config::Config;

/// The identity used when nothing resolves (upstream's literal).
pub const UNKNOWN_USER_ID: &str = "unknown";

/// Resolves the web identity against an injected resolver.
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

/// The production wrapper: the inject path's own [`EnvResolver`] over the
/// process env and the `git` probe, built from the same three config fields
/// `composition.rs` copies.
///
/// [`EnvResolver`]: mem_bridge::composition::EnvResolver
pub fn resolve_user_id(config: &Config, directory: &Path) -> String {
    let resolver = mem_bridge::composition::EnvResolver {
        container_tag_prefix: config.container_tag_prefix.clone(),
        user_email_override: config.user_email_override.clone(),
        user_name_override: config.user_name_override.clone(),
    };
    resolve_user_id_with(config, &resolver, directory)
}

/// Trims, then drops an empty value: a whitespace-only email is not an
/// identity (upstream's `||` chain never sees `"   "` as truthy-but-blank).
fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
