//! Deterministic id/token helpers (blueprint W3.1, D-V7).
//!
//! Upstream uses `randomUUID()` for owner tokens, tmp-file suffixes and
//! changelog ids, and `Math.random().toString(36).slice(2, 9)` for prompt /
//! profile / changelog ids. The CAS/release invariants are token-conditional
//! (`DELETE … WHERE owner_token = ?`), so uniqueness-in-practice is all that
//! matters — no `uuid`/`rand` crate is added (offline-resolve risk) and the
//! token stays human-judgeable in a coordination row.
//!
//! Shape: `{pid}-{unix_millis}-{counter}` — monotonic within a process, and
//! (pid, millis, counter) uniquely identifies it across cooperating
//! processes on one host.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Current unix time in milliseconds (`Date.now()`).
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Process-monotonic counter value (1-based).
fn next_counter() -> u64 {
    TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

/// Base-36 rendering of a counter (`Number.prototype.toString(36)`).
pub fn base36(value: u64) -> String {
    if value == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    // base36 digits: the String is built with push(char), so the Vec<u8> round-trip disappears (ADR-0013 D-3)
    let mut buffer = String::with_capacity(13);
    let mut remaining = value;
    while remaining > 0 {
        buffer.push(char::from(DIGITS[(remaining % 36) as usize]));
        remaining /= 36;
    }
    buffer.chars().rev().collect()
}

/// A unique owner token: `{pid}-{unix_millis}-{counter}`.
///
/// Zero-padded base36 keeps the millisecond component visually separated
/// while remaining sortable-enough for human inspection of a lock row.
pub fn next_token() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        now_millis(),
        base36(next_counter())
    )
}

/// A prompt id mirroring upstream
/// `prompt_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`.
pub fn next_prompt_id() -> String {
    format!("prompt_{}_{}", now_millis(), base36(next_counter()))
}

/// A profile id mirroring upstream
/// `profile_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`.
pub fn next_profile_id() -> String {
    format!("profile_{}_{}", now_millis(), base36(next_counter()))
}

/// A changelog id mirroring upstream
/// `changelog_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`.
pub fn next_changelog_id() -> String {
    format!("changelog_{}_{}", now_millis(), base36(next_counter()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_tokens_are_unique_within_a_process() {
        let tokens: HashSet<String> = (0..2_000).map(|_| next_token()).collect();
        assert_eq!(tokens.len(), 2_000);
    }

    #[test]
    fn test_token_shape_is_pid_millis_counter() {
        let token = next_token();
        let parts: Vec<&str> = token.split('-').collect();
        assert_eq!(parts.len(), 3, "token: {token}");
        assert_eq!(parts[0], std::process::id().to_string());
        assert!(parts[1].parse::<i64>().expect("millis") > 0);
        assert!(base36(1) == "1");
    }

    #[test]
    fn test_prompt_ids_are_prefixed_and_unique() {
        let ids: HashSet<String> = (0..1_000).map(|_| next_prompt_id()).collect();
        assert_eq!(ids.len(), 1_000);
        assert!(ids.iter().all(|id| id.starts_with("prompt_")));
    }

    #[test]
    fn test_base36_matches_javascript_rendering() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(base36(1_295), "zz");
    }
}
