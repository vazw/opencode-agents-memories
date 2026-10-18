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
pub mod error;
pub mod lifecycle;
pub mod operation_lock;
pub mod platform;
pub mod project_tags;
pub mod prompts;
pub mod ready;
pub mod schema_migrations;
pub mod shard_inventory;
pub mod shard_manager;
pub mod shard_path_migration;
pub mod sql;
pub mod sqlite_files;
pub use mem_core::scope;
pub mod vector;

pub use crate::db::{TursoConnectFactory, turso_connect_factory};
