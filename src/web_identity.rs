//! The web-side identity resolver (issue 115), now a thin re-export of the
//! hoisted [`mem_bridge::identity`] resolver plus the production wrapper (D-5).
//!
//! Upstream's REST handlers tag a profile request with
//! `getTags(process.cwd()).user.userEmail || "unknown"`
//! (`example/opencode-mem/src/services/api-handlers.ts:1157-1159`). The web
//! surface therefore resolves the **launch cwd**; the inject path resolves the
//! **per-call session directory**. The resolver and the config fields are
//! shared, the directory is not — a host serving several directories may
//! legitimately show two identities, and no gate may assert a single one.
//!
//! The precedence chain itself lives in [`mem_bridge::identity`] so the
//! composition root and this module call the *same* code. This module keeps
//! only [`resolve_user_id`], the production wrapper that composes the inject
//! path's own [`EnvResolver`].
//!
//! [`EnvResolver`]: mem_bridge::composition::EnvResolver

use std::path::Path;

use mem_core::config::Config;

pub use mem_bridge::identity::{UNKNOWN_USER_ID, resolve_user_id_with};

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
