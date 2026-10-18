//! Prompt-path update seam (blueprint W1.2 step 12): the migration layer
//! updates secondary prompt metadata (user_prompts rows) through one
//! injectable trait; the real user-prompts wiring arrives in W3.1.

#![allow(dead_code)]

use crate::db::BoxFuture;
use crate::error::StoreError;

pub trait PromptPathUpdater: Send + Sync {
    fn update_project_paths(
        &self,
        old_path: &str,
        new_path: &str,
    ) -> BoxFuture<'_, Result<u64, StoreError>>;
}

/// No-op updater (no prompt store wired yet).
pub struct NoopPromptPathUpdater;

impl PromptPathUpdater for NoopPromptPathUpdater {
    fn update_project_paths(
        &self,
        _old_path: &str,
        _new_path: &str,
    ) -> BoxFuture<'_, Result<u64, StoreError>> {
        Box::pin(async { Ok(0) })
    }
}
