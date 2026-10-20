//! Time helpers (W4.1). D-1: row ids are minted SQL-side via
//! `SELECT uuid7_str()`, so this module deliberately carries only
//! [`now_millis`] — no `new_agent_session_id`, no base36 counter, no new
//! dependency edge.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current unix time in milliseconds (`Date.now()`).
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
