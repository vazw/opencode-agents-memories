//! mem-core — shared kernel: memory scopes, tags, config loading, typed errors, shared utilities.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-core";

pub mod agent;
pub mod config;
pub mod dedup;
pub mod env;
pub mod error;
pub mod logging;
pub mod scope;
pub mod sha256;
pub mod tags;
pub mod tier;
pub mod utils;

pub use error::{Error, Result};
