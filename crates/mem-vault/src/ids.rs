//! Store-owned clock seam (W8a).
//!
//! The vault's `created_at` is stamped by the store, not the caller, so a
//! caller cannot backdate or forward-date a row. The clock is not injectable:
//! vault tests assert "the store set it, not the caller", not a specific
//! value. This is a deliberate trade-off: it keeps the store's public surface
//! free of a clock parameter and matches the existing `mem-facts`/`mem-episodic`
//! `now_millis` convention, at the cost of not being able to pin the value
//! in unit tests (tests assert `> 0` and "not caller-supplied").

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
