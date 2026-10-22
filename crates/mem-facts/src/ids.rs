//! The millisecond clock seam (mirrors `mem-episodic::ids`).

/// The current unix time in milliseconds (0 when the clock is before the
/// epoch — total, never panicking).
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
