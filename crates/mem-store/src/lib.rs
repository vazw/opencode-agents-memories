//! mem-store — storage engine on the `turso` crate: shards, connection
//! lifecycle, migrations, vector search, encryption.
//!
//! W1.2 layout (blueprint): `error`/`config`/`platform` are the store
//! kernel; `sql` is the verbatim upstream SQL locus; `db` is the connection
//! seam; the lifecycle/manager modules land across the wave.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-store";

pub mod config;
pub mod connection;
pub mod db;
pub mod encryption_key;
pub mod encryption_migrator;
pub mod engine_lock;
pub mod engine_migrator;
pub mod episodic_sql;
pub mod error;
pub mod legacy_migrator;
pub mod lifecycle;
pub mod migration_service;
pub mod open_guard;
pub mod operation_lock;
pub mod platform;
pub mod project_tags;
pub mod prompts;
pub mod ready;
pub mod schema_migrations;
pub mod shard_facts;
pub mod shard_inventory;
pub mod shard_manager;
pub mod shard_path_migration;
pub mod sql;
pub mod sqlite_files;
pub mod vector;
pub mod vector_utils;

pub use mem_core::scope;

pub use crate::db::{TursoConnectFactory, turso_connect_factory};

// Workspace-stack re-exports consumed by downstream crates that build on the
// storage layer (W4.1 `mem-episodic`): keeps those crates free of new
// dependency edges while the very same crate versions remain in use.
pub use serde;
pub use serde_json;
pub use tokio;
pub use tracing;
