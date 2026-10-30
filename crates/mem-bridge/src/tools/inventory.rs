//! Issue 70 wave 1 — the `list-shards` and `migrate` wire methods over the
//! store's inventory and path-migration services.

use std::path::Path;
use std::sync::Arc;

use mem_profile::prompt::UserPromptManager;
use mem_store::error::StoreError;
use mem_store::platform::SystemProcessProbe;
use mem_store::prompts::PromptPathUpdater;
use mem_store::shard_inventory::ShardInventory;
use mem_store::shard_path_migration::{
    PathMigrationError, PathMigrationOptions, PathMigrationService,
};

use crate::composition::ComposedRuntime;
use crate::jsonrpc::Request;

use super::{ToolError, required_string};

/// The two inventory-tier methods' shared service.
pub struct InventoryService {
    inventory: Arc<ShardInventory>,
    migration: PathMigrationService,
}

impl InventoryService {
    /// Explicit constructor (the test seam: the two store services are built
    /// by the caller).
    pub fn new(inventory: Arc<ShardInventory>, migration: PathMigrationService) -> Self {
        Self {
            inventory,
            migration,
        }
    }

    /// Production: both services over the composed runtime's exact
    /// dependencies (D-9's argument lists).
    pub fn from_composed(composed: &ComposedRuntime) -> Self {
        let inventory = Arc::new(ShardInventory::new(
            composed.storage_root.clone(),
            composed.store_config.clone(),
            composed.connections.clone(),
            composed.shards.clone(),
            composed.tags.clone(),
        ));
        let migration = PathMigrationService::new(
            composed.storage_root.clone(),
            composed.store_config.clone(),
            composed.connections.clone(),
            composed.shards.clone(),
            inventory.clone(),
            composed.tags.clone(),
            Arc::new(PromptStorePathUpdater::new(composed.prompts.clone())),
            Arc::new(SystemProcessProbe),
        );
        Self::new(inventory, migration)
    }
}

impl InventoryService {
    /// `list-shards`: the ported DTO's own camelCase shape, no mapping layer.
    pub async fn list_shards(&self, request: &Request) -> Result<serde_json::Value, ToolError> {
        let project_path = required_string(request, "project_path", "project_path")?;
        let result = self
            .inventory
            .list_shards(Path::new(project_path))
            .await
            .map_err(|_error| ToolError::internal())?;
        // `to_value` is fallible (a non-UTF-8 `PathBuf`): internal, never a
        // panic.
        serde_json::to_value(&result).map_err(|_error| ToolError::internal())
    }
}

/// The prompt-path seam's first production impl: `migrate` rewrites the
/// `user_prompts.project_path` rows through the real prompt store.
pub struct PromptStorePathUpdater {
    prompts: Arc<UserPromptManager>,
}

impl PromptStorePathUpdater {
    /// Wraps the composed prompt store.
    pub fn new(prompts: Arc<UserPromptManager>) -> Self {
        Self { prompts }
    }
}

impl PromptPathUpdater for PromptStorePathUpdater {
    fn update_project_paths(
        &self,
        old_path: &str,
        new_path: &str,
    ) -> mem_store::db::BoxFuture<'_, Result<u64, StoreError>> {
        // The trait's elided future lifetime is `&self`'s: the path strings
        // are owned into the future instead.
        let old_path = old_path.to_string();
        let new_path = new_path.to_string();
        Box::pin(async move {
            self.prompts
                .update_project_path(&old_path, &new_path)
                .await
                .map_err(|error| StoreError::Internal {
                    message: error.to_string(),
                })
        })
    }
}

impl InventoryService {
    /// `migrate`: `project_path` is the target directory; `from_path`/
    /// `from_hash` select the source. The service holds the operation lock
    /// itself (never here) and refuses a source that still exists unless
    /// `allow_linked_source` is set.
    pub async fn migrate(&self, request: &Request) -> Result<serde_json::Value, ToolError> {
        let project_path = required_string(request, "project_path", "project_path")?;
        let from_path = super::optional_string(request, "from_path", "from_required")?;
        let from_hash = super::optional_string(request, "from_hash", "from_required")?;
        let dry_run = super::optional_bool(request, "dry_run", "dry_run")?.unwrap_or(false);
        let allow_linked_source =
            super::optional_bool(request, "allow_linked_source", "allow_linked_source")?
                .unwrap_or(false);
        let empty = |value: Option<&str>| value.map(|text| text.trim().is_empty()).unwrap_or(true);
        if empty(from_path) && empty(from_hash) {
            return Err(ToolError::invalid("from_required"));
        }

        let options = PathMigrationOptions {
            from_path: from_path.map(str::to_string),
            from_hash: from_hash.map(str::to_string),
            dry_run,
            allow_linked_source,
        };
        let result = match self
            .migration
            .migrate(Path::new(project_path), &options)
            .await
        {
            Ok(result) => result,
            Err(PathMigrationError::ResolveSource(inner)) => {
                let candidates = serde_json::to_value(&inner.candidates)
                    .map_err(|_error| ToolError::internal())?;
                return Err(ToolError::invalid("migrate_source")
                    .with_message(inner.message)
                    .with_extra("candidates", candidates));
            }
            Err(PathMigrationError::Aborted(message)) => {
                return Err(ToolError::invalid("migrate_refused").with_message(message));
            }
            Err(PathMigrationError::Store(_error)) => return Err(ToolError::internal()),
        };

        let mut actions = Vec::with_capacity(result.actions.len());
        for action in &result.actions {
            let mut value = serde_json::to_value(action).map_err(|_error| ToolError::internal())?;
            if let serde_json::Value::Object(map) = &mut value {
                map.insert(
                    "detail".to_string(),
                    serde_json::Value::String(action.detail()),
                );
            }
            actions.push(value);
        }
        Ok(serde_json::json!({
            "actions": actions,
            "dry_run": result.dry_run,
            "matched_by": result.matched_by,
            "migrated_memories": result.migrated_memories,
            "migrated_shards": result.migrated_shards,
            "new_hash": result.new_hash,
            "new_path": result.new_path,
            "old_hash": result.old_hash,
            "old_path": result.old_path,
        }))
    }
}
