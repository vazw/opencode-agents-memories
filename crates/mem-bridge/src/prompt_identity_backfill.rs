//! Issue 176 — the prompt-identity backfill (v4 → v5).
//!
//! Before issue 174 a `user_prompts` row carried no identity, so the learner
//! could only ever see the launch identity's rows. This routine stamps each
//! still-unstamped row with the identity of its OWN `project_path`, resolved
//! through the same composition resolver the capture path uses.
//!
//! Contract:
//! - **Fresh root**: [`maybe_backfill_prompt_identities`] returns `Ok(None)`
//!   BEFORE any store call when `user-prompts.db` does not exist — it never
//!   creates the file, so a fresh root's startup-audit inventory is unchanged.
//! - **Unresolvable path** (`UNKNOWN_USER_ID`): its rows stay NULL. Nothing is
//!   deleted and no profile is read or written.
//! - **Idempotent**: every UPDATE is guarded `user_id IS NULL`, so two
//!   concurrent boots converge.
//! - **Counts only**: [`backfill_event_detail`] carries `stamped` and
//!   `remainingNull`, never a path, an email or prompt text.

use std::path::Path;

use mem_core::config::Config;
use mem_profile::error::ProfileError;
use mem_profile::prompt::{USER_PROMPTS_DB_NAME, UserPromptManager};

use crate::identity::{UNKNOWN_USER_ID, resolve_user_id_with};
use crate::scope_resolver::ProjectScopeResolver;

/// One backfill run's counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptIdentityBackfillReport {
    /// Rows whose identity this run stamped.
    pub stamped: u64,
    /// Rows still carrying `NULL` afterwards (an unresolvable path's).
    pub remaining_null: i64,
}

/// The ONLY entry point the engine calls. Returns `Ok(None)` — touching
/// nothing, creating nothing — when the prompts file does not exist.
pub async fn maybe_backfill_prompt_identities(
    storage_root: &Path,
    prompts: &UserPromptManager,
    config: &Config,
    resolver: &dyn ProjectScopeResolver,
) -> Result<Option<PromptIdentityBackfillReport>, ProfileError> {
    // The precise predicate: a fresh root has no `user-prompts.db`, so the
    // backfill never calls `ready()`, never runs the migration and never
    // creates the file. `exists()` would admit a directory and then create one.
    if !storage_root.join(USER_PROMPTS_DB_NAME).is_file() {
        return Ok(None);
    }
    backfill_prompt_identities(prompts, config, resolver)
        .await
        .map(Some)
}

/// Inner routine: applies v5 (`ready()`), then stamps each path's own identity.
pub async fn backfill_prompt_identities(
    prompts: &UserPromptManager,
    config: &Config,
    resolver: &dyn ProjectScopeResolver,
) -> Result<PromptIdentityBackfillReport, ProfileError> {
    // The v5 column must exist before any UPDATE names it. On an existing v4
    // file this is the upgrade leg; the fresh-root gate above is what stops a
    // new file from ever being created.
    prompts.ready().await?;

    let mut stamped = 0u64;
    for project_path in prompts.project_paths_without_identity().await? {
        let user_id = resolve_user_id_with(config, resolver, Path::new(&project_path));
        if user_id == UNKNOWN_USER_ID {
            continue;
        }
        stamped += prompts
            .stamp_identity_for_project_path(&project_path, &user_id)
            .await?;
    }
    let remaining_null = prompts.count_without_identity().await?;
    Ok(PromptIdentityBackfillReport {
        stamped,
        remaining_null,
    })
}

/// Counts only: exactly `{"stamped": u64, "remainingNull": i64}`.
pub fn backfill_event_detail(report: &PromptIdentityBackfillReport) -> serde_json::Value {
    serde_json::json!({
        "stamped": report.stamped,
        "remainingNull": report.remaining_null,
    })
}
