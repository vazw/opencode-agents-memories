//! mem-facts — the durable five-tier facts DB (`facts.db`).
//!
//! # What this crate is
//!
//! One `facts.db` per storage root (Q1), with `project_id`-scoped rows and a
//! `tier` discriminator (`UNIQUE (project_id, tier, key_normalized)`). Atomic
//! facts live here; the legacy tier-1 `memories` shard stays upstream-golden
//! and untouched.
//!
//! # Name collision (M30)
//!
//! [`store::FactsStore`] — this crate's durable facts DB — is **not**
//! `mem_portability::facts::FactStore` (the tier-1 memory seam portability
//! already owns). The names differ by design and must not be conflated.
//!
//! # The Sound Memory invariant (verbatim, ADR-0010 D-1 as amended in effect)
//!
//! Raw context is never destroyed by consolidation or by any lifecycle stage;
//! it is destroyed only by an explicit, owner-initiated purge
//! (`purge_session`).
//!
//! This crate owns the facts half of that one erasure path
//! ([`store::FactsStore::purge_sources`], `src/purge.rs` only) and no
//! automatic caller; consolidation writes facts, it never purges them.
//!
//! # Erasure: no tombstone
//!
//! The ONLY hard delete of a fact is [`store::FactsStore::purge_sources`], an
//! explicit, owner-initiated erasure. It writes **no tombstone**: the erased
//! fact is simply absent, so a later capture/consolidation can encode the
//! same knowledge again as a normal write. `soft_delete` is the opposite tool
//! — it keeps the row and hides it from reads. The erasure promise is about
//! stored bytes, not a permanent deny-list.
//!
//! A purge deletes multi-source facts on purpose: the shared milestone is
//! deleted with them, so the surviving contributor's summary is lost
//! permanently; recovery is an explicit re-capture of the surviving session
//! (see the bridge's pipeline docs).

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-facts";

pub mod purge;
pub mod schema;
pub mod store;

pub(crate) mod ids;

pub use store::{Fact, FactInput, FactSource, FactWrite, FactsStore};

use mem_store::error::StoreError;

/// The facts-tier error surface.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FactsError {
    /// Storage-layer pass-through.
    #[error("{0}")]
    Store(#[from] StoreError),
    /// Rejected input; `field` names the offending field. The message never
    /// echoes the rejected content.
    #[error("invalid {field}: {message}")]
    Validation {
        /// Machine-readable field name (`tier`, `content`, `sources`, …).
        field: String,
        /// Fixed human-readable reason.
        message: String,
    },
    /// A serialization/IO failure outside the store surface.
    #[error("{message}")]
    Internal {
        /// Human-readable reason.
        message: String,
    },
}

impl FactsError {
    /// Builds a validation error.
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }
}
