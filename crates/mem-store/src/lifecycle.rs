//! Lifecycle — port of upstream `src/services/turso/lifecycle.ts`
//! (blueprint W1.2 step 10): close every connection, then invalidate the
//! caches so the next access reopens + revalidates.
//!
//! Divergence (reviewer note): upstream additionally resets the prompt /
//! profile / session / skills caches — those crates arrive in later
//! waves; the store-side trio (connections, shard manager, ready gate)
//! is complete for this crate's state.

#![allow(dead_code)]

use crate::connection::ConnectionManager;
use crate::ready::ReadyGate;
use crate::shard_manager::ShardManager;

/// Upstream `closeTursoAndInvalidateCaches` (store-side half).
pub async fn close_turso_and_invalidate_caches(
    connections: &ConnectionManager,
    shards: &ShardManager,
    ready: &ReadyGate,
) {
    connections.close_all().await;
    shards.reset();
    ready.reset();
}
