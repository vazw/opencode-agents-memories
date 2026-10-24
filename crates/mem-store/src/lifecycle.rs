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
use crate::error::StoreError;
use crate::ready::ReadyGate;
use crate::shard_manager::ShardManager;

/// Upstream `closeTursoAndInvalidateCaches` (store-side half).
///
/// All three stages are **attempted**: the contract is "close *and*
/// invalidate", so a failed close is precisely when the caches must still be
/// cleared. Every failure is either returned or reported — nothing is dropped
/// silently (ADR-0013 D-4).
pub async fn close_turso_and_invalidate_caches(
    connections: &ConnectionManager,
    shards: &ShardManager,
    ready: &ReadyGate,
) -> Result<(), StoreError> {
    let close_result = connections.close_all().await;
    let reset_result = shards.reset();
    let ready_result = ready.reset();
    if let Err(error) = &reset_result {
        tracing::error!(error = %error, "shard caches not reset");
    }
    if let Err(error) = &ready_result {
        tracing::error!(error = %error, "ready gate not reset");
    }
    close_result?;
    reset_result?;
    ready_result?;
    Ok(())
}
