//! THE container-tag traversal guard.
//!
//! Every scoped endpoint resolves its container tag here, once:
//! query filters (`?tag=`), the `POST /api/memories` body, the skills body
//! tag and the skill id check all go through the same resolution. The guard
//! **rejects** a malformed tag, never sanitises it, never widens the scope to
//! the unfiltered listing and never echoes the offending tag (D-10).
//!
//! The real strength and the real limit are upstream's, and are deliberate
//! (D-A): [`mem_core::scope::extract_scope_from_container_tag`] validates the
//! **scope segment** (`user`/`project`) and the **hash segment** (exactly 16
//! lowercase hex) but not the prefix. A traversal-shaped prefix therefore
//! parses — and is harmless, because the container tag only ever travels as a
//! bound SQL parameter and the shard path is derived from the validated
//! `Scope` + `ScopeHash`, so it never becomes a path segment.

use mem_core::scope::{Scope, extract_scope_from_container_tag};

/// The fixed rejection message (D-10 — no echo of the offending tag).
pub const INVALID_CONTAINER_TAG: &str = "Invalid container tag";

/// A resolved scoped request: exactly one scope + one hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedRequest {
    pub scope: Scope,
    pub hash: String,
}

/// Why a container tag was refused. The reason is logged; the tag text is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeRejection {
    /// The tag is not `{prefix}_{user|project}_{16 lowercase hex}`.
    InvalidContainerTag,
}

impl ScopeRejection {
    /// The stable log label (never the tag text).
    pub fn reason(self) -> &'static str {
        match self {
            Self::InvalidContainerTag => "invalid_container_tag",
        }
    }
}

/// Whether the tag resolves to exactly one scope + one 16-hex hash.
pub fn is_valid_container_tag(container_tag: &str) -> bool {
    resolve_scoped_request(container_tag).is_ok()
}

/// Resolves a container tag to its single scope reference.
///
/// A rejection is logged with the tag's **length** and the reason only — the
/// tag is attacker-controlled and must never reach a log line or a response
/// body.
pub fn resolve_scoped_request(container_tag: &str) -> Result<ScopedRequest, ScopeRejection> {
    match extract_scope_from_container_tag(container_tag) {
        Ok(resolved) => {
            let hash = resolved
                .hash
                .expect("extract_scope_from_container_tag always carries a hash")
                .as_str()
                .to_string();
            Ok(ScopedRequest {
                scope: resolved.scope,
                hash,
            })
        }
        Err(error) => {
            tracing::warn!(
                reason = %ScopeRejection::InvalidContainerTag.reason(),
                tag_length = container_tag.len(),
                error = %error,
                "container tag rejected"
            );
            Err(ScopeRejection::InvalidContainerTag)
        }
    }
}
