//! Poison-recovering lock accessors for ADR-0013 **D-2 recover** sites.
//!
//! A `std` lock is poisoned when a thread panics while holding it. At the
//! sites ADR-0013 D-2 classifies **R** (clause tag `D2-R`) the guard protects
//! one self-contained value whose own operations cannot leave it malformed and
//! nothing persisted, cross-field or side-effect-gating depends on it, so the
//! process continues with the value exactly as the panicking thread left it.
//! Recovery is silent by design — no log, no degraded flag — which is why the
//! per-site in-code comment, not this module, is the durable record.
//!
//! Every call site carries its one-line justification comment; this module
//! only supplies the mechanics (`match`, never `unwrap_or_else`, so the
//! recovered value is the `PoisonError::into_inner()` payload).
//!
//! # Gate (ADR-0013 D-5)
//!
//! ```text
//! cargo clippy --workspace --lib -- -A clippy::all \
//!     -D clippy::unwrap_used -D clippy::expect_used
//! ```
//!
//! Expected production `unwrap`/`expect` site count: **40** at Part A close,
//! **0** at wave close, with the command exiting `0` in the `-D` form.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Locks `mutex`, recovering the guard from a poisoned lock.
pub fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Acquires a shared read guard, recovering from a poisoned lock.
pub fn read_or_recover<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Acquires an exclusive write guard, recovering from a poisoned lock.
pub fn write_or_recover<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Mutex, RwLock};

    /// Poison `mutex` on the current thread after leaving `value` in it, so the
    /// recovered guard must expose exactly that value.
    fn poison_mutex(mutex: &Mutex<i32>, value: i32) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = mutex.lock().expect("fresh lock");
            *guard = value;
            panic!("poison");
        }));
        assert!(mutex.is_poisoned(), "lock must be poisoned");
    }

    fn poison_rwlock(lock: &RwLock<i32>, value: i32) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = lock.write().expect("fresh lock");
            *guard = value;
            panic!("poison");
        }));
        assert!(lock.is_poisoned(), "lock must be poisoned");
    }

    #[test]
    fn test_lock_or_recover_returns_guard_after_poison() {
        let mutex = Mutex::new(0);
        poison_mutex(&mutex, 41);
        let guard = lock_or_recover(&mutex);
        assert_eq!(*guard, 41);
    }

    #[test]
    fn test_read_or_recover_returns_guard_after_poison() {
        let lock = RwLock::new(0);
        poison_rwlock(&lock, 42);
        let guard = read_or_recover(&lock);
        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_write_or_recover_returns_guard_after_poison() {
        let lock = RwLock::new(0);
        poison_rwlock(&lock, 43);
        let mut guard = write_or_recover(&lock);
        assert_eq!(*guard, 43);
        *guard = 44;
        drop(guard);
        assert_eq!(*read_or_recover(&lock), 44);
    }
}
