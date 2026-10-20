//! The in-process consolidation gate (W4.3, D-24).
//!
//! Two consolidation runs over the same storage root would write duplicate
//! milestones over the same rows, so a run takes BOTH locks:
//!
//! - THIS gate — a process-wide registry keyed by the CANONICAL storage
//!   path. It is **std only** (`OnceLock` + `Mutex<HashMap<…>>` +
//!   `compare_exchange`): the gate is a try-acquire (busy ⇒ skip), so an
//!   async mutex would add a dependency for no benefit, and
//!   `mem-episodic` carries no tokio edge;
//! - the cross-process operation lock (`mem_store::operation_lock`) — the
//!   job of the cross-process case.
//!
//! The registry mutex is held only long enough to clone the `Arc<AtomicBool>`
//! and is NEVER held across an await.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The process-wide registry: canonical storage path → the busy flag.
static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<AtomicBool>>>> = OnceLock::new();

/// The registry (initialized on first use).
fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<AtomicBool>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The canonical key of a storage path.
///
/// The canonical path when it exists; otherwise the nearest existing
/// ancestor's canonical path with the missing tail re-appended; otherwise
/// the raw path.
pub fn canonical_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    let mut tail: Vec<&OsStr> = Vec::new();
    let mut current = path;
    while let Some(name) = current.file_name() {
        tail.push(name);
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
        if let Ok(canonical) = std::fs::canonicalize(current) {
            let mut key = canonical;
            for part in tail.iter().rev() {
                key.push(part);
            }
            return key;
        }
    }
    path.to_path_buf()
}

/// The guard that holds the gate.
///
/// It owns the shared flag and releases it on drop, so every path — normal
/// return, early return or panic — releases the gate.
pub struct GateGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// Tries to acquire the gate for `storage_path`.
///
/// `Some(guard)` when the gate was free, `None` when another run in THIS
/// process already holds it (busy).
pub fn try_acquire(storage_path: &Path) -> Option<GateGuard> {
    let key = canonical_key(storage_path);
    let flag = {
        // The registry mutex is released before the guard is returned: it is
        // never held across an await.
        let mut registry = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            registry
                .entry(key)
                .or_insert_with(|| Arc::new(AtomicBool::new(false))),
        )
    };
    match flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => Some(GateGuard { flag }),
        Err(_) => None,
    }
}
