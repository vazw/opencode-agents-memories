//! mem-bridge — OpenCode host integration: V1/V2 plugin bridge, stdio
//! server, TUI companion. W5.1 lands the V1 stdio server, the JSON-RPC
//! codec, the error map, the DB lane and the engine lifetime wiring.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-bridge";

pub mod db_lane;
pub mod engine;
pub mod error_map;
pub mod jsonrpc;
