//! mem-bridge — OpenCode host integration: V1/V2 plugin bridge, stdio
//! server, TUI companion. W5.1 lands the V1 stdio server, the JSON-RPC
//! codec, the error map, the DB lane and the engine lifetime wiring.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-bridge";

pub mod capture;
pub mod capture_context;
pub mod capture_prompt;
pub mod capture_provider;
pub mod compaction;
pub mod composition;
pub mod cross_tier;
pub mod db_lane;
pub mod engine;
pub mod error_map;
pub mod host_endpoint;
pub mod host_provider;
pub mod inject;
pub mod jsonrpc;
pub mod manual_provider;
pub mod project_scope;
pub mod purge;
pub mod reap;
pub mod recall;
pub mod reembed;
pub mod scope_resolver;
pub mod skills;
pub(crate) mod startup_audit;
pub mod sweep;
pub mod tools;
pub mod truncation_report;
pub mod vault_feeder;
pub mod vault_source_ref;
