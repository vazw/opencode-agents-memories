//! The production [`DedupStore`]: one [`ShardManager`] over the real memory
//! shards, plus the deterministic row order the pass depends on.
//!
//! W3 / ADR-0018: the shard's `shard_stats` count is the authority and the
//! registry is its cache. [`ShardDedupStore::delete`] removes the row and
//! writes the statement-measured ABSOLUTE count in the same transaction, then
//! adopts it into the registry — see [`DedupStore`]'s `delete` doc.

use async_trait::async_trait;
use mem_core::scope::Scope;
use mem_store::error::StoreError;
use mem_store::shard_manager::ShardManager;
use mem_store::vector;
use mem_store::vector_utils::blob_to_float32_array;

use super::{DedupMemory, DedupShard, DedupStore};
use crate::api::ports::PortError;

/// The production `DedupStore`: one `ShardManager` over the real memory
/// shards.
pub struct ShardDedupStore {
    shards: ShardManager,
}

impl ShardDedupStore {
    pub fn new(shards: ShardManager) -> Self {
        Self { shards }
    }
}

/// A store fault: the detail is logged by the caller, never echoed (D-9).
fn store_failure(error: StoreError) -> PortError {
    PortError::Store(error.message())
}

/// The deterministic order of hardening change (ii): `created_at DESC, then
/// `id ASC`.
///
/// A total order because `id` is the primary key. Public so the pure ordering
/// test in `tests/dedup_store.rs` can drive it with its own fixture order.
/// `GET_ALL_MEMORIES` already carries `ORDER BY created_at DESC`; what this
/// call supplies is the otherwise-unspecified **tie** order within one
/// `created_at`, which is what makes the reported grouping stable between
/// runs and between builds.
pub fn order_rows(rows: &mut [DedupMemory]) {
    rows.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[async_trait]
impl DedupStore for ShardDedupStore {
    async fn shards(&self) -> Result<Vec<DedupShard>, PortError> {
        // Upstream: `[...userShards, ...projectShards]`, each list from
        // `getAllShards(scope, "")` (the `None` hash is that wildcard), each
        // ordered by `shard_index ASC` by the registry SQL.
        let mut out = Vec::new();
        for scope in [Scope::User, Scope::Project] {
            let shards = self
                .shards
                .get_all_shards(scope, None)
                .await
                .map_err(store_failure)?;
            out.extend(shards.into_iter().map(|info| DedupShard {
                shard_id: info.id,
                db_path: info.db_path,
            }));
        }
        Ok(out)
    }

    async fn memories(&self, shard: &DedupShard) -> Result<Vec<DedupMemory>, PortError> {
        let db = self
            .shards
            .open_shard(&shard.db_path)
            .await
            .map_err(store_failure)?;
        // `get_all_memories` is `SELECT *` and fallible. A read failure is
        // mapped and the pass counts the shard as unreadable
        // (`nearDuplicateScan.unreadableShards`) and truncates its coverage
        // claim — it is never reported as an empty shard.
        let rows = vector::get_all_memories(db.as_ref())
            .await
            .map_err(store_failure)?;
        let mut memories: Vec<DedupMemory> = rows
            .iter()
            .map(|row| DedupMemory {
                id: row.text("id").unwrap_or_default(),
                content: row.text("content").unwrap_or_default(),
                container_tag: row.text("container_tag").unwrap_or_default(),
                created_at: row.i64("created_at").unwrap_or(0),
                // The raw `vector` column, not `vector_extract()`: the
                // decoder accepts both the BLOB and the JSON-Text shape, so
                // the stored representation cannot surprise the pass.
                vector: row.get("vector").and_then(blob_to_float32_array),
            })
            .collect();
        order_rows(&mut memories);
        Ok(memories)
    }

    async fn delete(&self, shard: &DedupShard, memory_id: &str) -> Result<(), PortError> {
        let db = self
            .shards
            .open_shard(&shard.db_path)
            .await
            .map_err(store_failure)?;
        // W3: delete + the statement-measured absolute count, one transaction.
        let count = vector::delete_memories(db.as_ref(), &[memory_id.to_string()])
            .await
            .map_err(store_failure)?;
        // The registry is a CACHE of that value, never a `-1`.
        self.shards
            .set_vector_count(shard.shard_id, count)
            .await
            .map_err(store_failure)?;
        Ok(())
    }
}
