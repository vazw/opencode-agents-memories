//! The durable-fact seam the portability service reads and writes.
//!
//! Upstream's portability service reaches straight into the turso shard
//! manager, the connection manager and the vector search. The port keeps
//! the same boundary but makes it injectable: the service only ever sees
//! the [`FactStore`] trait, never a store type. The production adapter
//! lives in `mem-web`'s `store_adapter` (it needs the shard manager and the
//! vector primitives), which is why `mem-web` depends on `mem-portability`.

use async_trait::async_trait;

use crate::PortabilityError;
use crate::bundle::ExportedMemory;

/// The durable-fact seam (the tier-1 memory store).
#[async_trait]
pub trait FactStore: Send + Sync {
    /// Every memory of one container tag.
    async fn list_memories(
        &self,
        container_tag: &str,
    ) -> Result<Vec<ExportedMemory>, PortabilityError>;

    /// Does a memory with this id exist in the container?
    async fn memory_exists(&self, id: &str, container_tag: &str) -> Result<bool, PortabilityError>;

    /// Inserts one memory into the target container.
    ///
    /// The embedding and the tags embedding are the re-computed vectors
    /// (the bundle never carries a vector), and `metadata_json` is the
    /// already-provenance-stamped metadata object.
    async fn insert_memory(
        &self,
        target_tag: &str,
        memory: ExportedMemory,
        embedding: Vec<f32>,
        tags_embedding: Option<Vec<f32>>,
        metadata_json: String,
    ) -> Result<(), PortabilityError>;

    /// Deletes one memory from the container.
    async fn delete_memory(&self, id: &str, container_tag: &str) -> Result<(), PortabilityError>;
}
