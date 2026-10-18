//! Root composition binary for the `opencode-agents-memory` workspace.
//!
//! The root package is the composition root: it depends on all eight
//! workspace crates but owns no logic of its own.

use mem_ai::CRATE_NAME as MEM_AI;
use mem_bridge::CRATE_NAME as MEM_BRIDGE;
use mem_core::CRATE_NAME as MEM_CORE;
use mem_embed::CRATE_NAME as MEM_EMBED;
use mem_episodic::CRATE_NAME as MEM_EPISODIC;
use mem_profile::CRATE_NAME as MEM_PROFILE;
use mem_store::CRATE_NAME as MEM_STORE;
use mem_web::CRATE_NAME as MEM_WEB;

/// The workspace crates in composition order.
fn workspace_crates() -> [&'static str; 8] {
    [
        MEM_CORE,
        MEM_STORE,
        MEM_EMBED,
        MEM_AI,
        MEM_PROFILE,
        MEM_EPISODIC,
        MEM_WEB,
        MEM_BRIDGE,
    ]
}

fn main() {
    for name in workspace_crates() {
        println!("{name}");
    }
}
