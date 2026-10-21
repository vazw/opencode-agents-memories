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

pub mod web_dist;
