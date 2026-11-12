//! Root composition **library** for the `opencode-agents-memory` package.
//!
//! The binary (`src/main.rs`) is the composition root; it owns the process
//! lifecycle and names no tokio type. This lib target exists so the
//! composition root's one piece of non-trivial decision logic — where the
//! built web UI lives — is a pure, testable function instead of a literal
//! buried in a `launch` future (W6.8).
//!
//! The crate root holds no state and no runtime: [`web_dist`] uses `std` plus
//! the `mem_store::serde_json` re-export the root package already depends on.
//!
//! Since issue 70 wave 1 the lib also carries the composition root's
//! portability adapter ([`portability_backend`]): the bridge's
//! `PortabilityFactory` seam can only be implemented where `mem-portability`
//! and `mem-web` meet, and root integration tests can only link the lib
//! target.

pub mod portability_backend;
pub mod shard_consolidation;
pub mod web_dist;
pub mod web_identity;
