//! Issue 48 — the B2′ erasure vocabulary (47.1-B2′, W6/decision 6).
//!
//! The rule: a prompt delete must invalidate every profile the deleted prompt
//! could have contributed to. "Could have contributed" is a watermark test
//! over `user_profiles.last_analyzed_at`:
//!
//! - the watermark is unset/zero/negative ⇒ **contributes** (a migrated row
//!   with an unusable watermark must fail toward erasure, never retention);
//! - otherwise the prompt contributed iff `prompt_created_at <= watermark`.
//!
//! The watermark advances on every profile write (`create_profile` and
//! `update_profile` both bind `now` to `last_analyzed_at`, D-5), so older
//! prompts count as contributing. B2′ permits nothing else: the failure
//! direction is over-erasure, never under-erasure.
//!
//! Nothing here writes a profile: the eraser only deletes derived rows
//! (47.7 — a blueprint for 48 that creates a profile row has reversed Leg A).

use mem_store::db::BoxFuture;

use crate::error::ProfileError;
use crate::profile_store::ProfileStore;

/// The class of 47.1-B2′: could a prompt created at `prompt_created_at` have
/// contributed to a profile whose watermark is `watermark`?
///
/// Unset or zero watermark ⇒ YES: a migrated row with an unusable watermark
/// must fail toward erasure, never toward retention.
pub fn could_have_contributed(watermark: i64, prompt_created_at: i64) -> bool {
    watermark <= 0 || prompt_created_at <= watermark
}

/// What one erasure pass removed (`ExecResult.changes` sums, never estimates).
///
/// Deliberately **not** `#[must_use]`: `mem-portability`'s `compensate()`
/// discards the result with `let _ = …` (D-2 corollary), and a `must_use`
/// marker would break that rollback line for no benefit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EraseReport {
    /// Profile rows inspected by the scan (1 for a direct `delete_profile`).
    pub profiles_considered: u64,
    /// `user_profiles` rows actually deleted.
    pub profiles_deleted: u64,
    /// `user_profile_changelogs` rows actually deleted (the snapshots are
    /// full copies of the profile text, so leaving them erases nothing).
    pub changelogs_deleted: u64,
}

/// The seam `UserPromptManager::delete_prompt` calls before it deletes the
/// source row (E2: erase first, delete second).
///
/// Implemented directly on [`ProfileStore`] — one store, one trait; an adapter
/// struct would add ceremony without a seam. `Arc<ProfileStore>` coerces to
/// `Arc<dyn ProfileEraser>` at the composition root's one injection site.
pub trait ProfileEraser: Send + Sync {
    /// Erase every profile whose watermark says it could have been derived
    /// from a prompt created at `prompt_created_at`.
    fn erase_derived_from<'a>(
        &'a self,
        prompt_created_at: i64,
    ) -> BoxFuture<'a, Result<EraseReport, ProfileError>>;
}

impl ProfileEraser for ProfileStore {
    fn erase_derived_from<'a>(
        &'a self,
        prompt_created_at: i64,
    ) -> BoxFuture<'a, Result<EraseReport, ProfileError>> {
        Box::pin(async move { self.erase_profiles_derived_from(prompt_created_at).await })
    }
}

/// The observable outcome of one `delete_prompt` call.
///
/// `Absent` mirrors upstream's idempotent `DELETE` (0 rows changed is a
/// success, not an error); `Deleted` reports the erasure so a caller can see
/// whether an eraser was configured at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDeletion {
    /// The row existed and is gone. `erasure` is `None` only when the manager
    /// was built without an eraser (test/embedded construction; never production).
    Deleted { erasure: Option<EraseReport> },
    /// No row with that id (upstream's idempotent DELETE).
    Absent,
}
