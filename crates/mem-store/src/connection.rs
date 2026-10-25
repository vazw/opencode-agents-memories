//! `TursoConnectionManager` — port of upstream
//! `src/services/turso/connection-manager.ts` (blueprint W1.2 step 4).
//!
//! Upstream order preserved exactly: await a global close if one is in
//! flight → await a per-path close if one is in flight → containment check
//! → cached handle → join an in-flight open (shared-await init, no
//! `tokio::spawn` so current-thread test runtimes work) → open.
//!
//! Divergences (reviewer notes): node `Promise` bookkeeping becomes
//! `tokio::sync::OnceCell<Result<..>>` for open dedup (its `get_or_init`
//! shares one initializer) plus a `Notify`-based [`Signal`] for close
//! awaits (OnceCell cannot await a foreign init without racing to run its
//! own); std guards are never held across `.await`; `Promise.allSettled`
//! error-swallowing becomes awaiting signals and ignoring results;
//! `closeAllSync` is N-A (handles release on Drop and `Drop` cannot fail,
//! so the upstream close-catch/log branches have no failure case).

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, OnceCell};

use crate::config::StoreConfig;
use crate::db::{
    ConnectFactory, ConnectOptions, DbHandle, assert_path_inside_storage, wrap_turso_open_error,
};
use crate::error::StoreError;
use crate::platform::Platform;

// ---------------------------------------------------------------------------
// Shared-await signal (see module divergence note)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Signal {
    done: AtomicBool,
    notify: Notify,
}

impl Signal {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Resolves when [`Signal::complete`] has been observed; tolerant to
    /// completing before/after the waiter registers (the `Notify` permit is
    /// stored on `notify_one`).
    async fn wait(&self) {
        while !self.done.load(Ordering::Acquire) {
            self.notify.notified().await;
        }
    }

    fn complete(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait_completion(signal: &Arc<Signal>) {
        signal.wait().await;
    }
}

/// An in-flight open owning a shared-init [`OnceCell`] plus the completion
/// signal closers await.
struct PendingOpen {
    result: OnceCell<Result<DbHandle, StoreError>>,
    completed: Arc<Signal>,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// The connection lifecycle owner for one storage root.
#[derive(Clone)]
pub struct ConnectionManager {
    storage_path: PathBuf,
    config: StoreConfig,
    platform: Platform,
    factory: Arc<dyn ConnectFactory>,
    connections: Arc<Mutex<HashMap<PathBuf, DbHandle>>>,
    pending: Arc<Mutex<HashMap<PathBuf, Arc<PendingOpen>>>>,
    closing_connections: Arc<Mutex<HashMap<PathBuf, Arc<Signal>>>>,
    closing_all: Arc<Mutex<Option<Arc<Signal>>>>,
}

impl ConnectionManager {
    pub fn new(
        storage_path: PathBuf,
        config: StoreConfig,
        platform: Platform,
        factory: Arc<dyn ConnectFactory>,
    ) -> Self {
        Self {
            storage_path,
            config,
            platform,
            factory,
            connections: Arc::default(),
            pending: Arc::default(),
            closing_connections: Arc::default(),
            closing_all: Arc::default(),
        }
    }

    fn global_close_signal(&self) -> Result<Option<Arc<Signal>>, StoreError> {
        // closing_all global-close flag: a stale Some makes every later open await a signal whose closer died (ADR-0013 D-2 D2-a)
        let closing = self
            .closing_all
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "closing_all",
                operation: "global_close_signal",
            })?;
        Ok(closing.as_ref().map(Arc::clone))
    }

    /// Upstream `getConnection` order, verbatim.
    pub async fn get_connection(&self, db_path: &Path) -> Result<DbHandle, StoreError> {
        // 1. await global close if in flight
        if let Some(closing) = self.global_close_signal()? {
            Signal::wait_completion(&closing).await;
        }
        // 2. await per-path close if in flight
        let per_path_close = {
            // closing_connections: await a per-path close signal; stale entry = a wait that never completes (ADR-0013 D-2 D2-a)
            self.closing_connections
                .lock()
                .map_err(|_| StoreError::LockPoisoned {
                    lock: "closing_connections",
                    operation: "get_connection",
                })?
                .get(db_path)
                .map(Arc::clone)
        };
        if let Some(closing) = per_path_close {
            Signal::wait_completion(&closing).await;
        }
        // 3. containment
        assert_path_inside_storage(&self.storage_path, db_path)?;
        // 4. cached handle
        {
            // connections handle cache: half of the open/close protocol; a stale miss re-opens a live file (ADR-0013 D-2 D2-a)
            let connections = self
                .connections
                .lock()
                .map_err(|_| StoreError::LockPoisoned {
                    lock: "connections",
                    operation: "get_connection",
                })?;
            if let Some(existing) = connections.get(db_path) {
                return Ok(existing.clone());
            }
        }
        // 5. in-flight open: join the shared initializer
        let pending = {
            // pending in-flight opens: a joined pending whose initializer panicked never completes (ADR-0013 D-2 D2-a)
            let mut pending = self.pending.lock().map_err(|_| StoreError::LockPoisoned {
                lock: "pending",
                operation: "get_connection",
            })?;
            Arc::clone(pending.entry(db_path.to_path_buf()).or_insert_with(|| {
                Arc::new(PendingOpen {
                    result: OnceCell::new(),
                    completed: Signal::new(),
                })
            }))
        };
        let owned_path = db_path.to_path_buf();
        let result = pending
            .result
            .get_or_init(|| self.open_database(&owned_path))
            .await
            .clone();
        // 6. completed: signal closers, then the cell always leaves the map
        //    (retry after failure re-opens; the cache answers after success).
        pending.completed.complete();
        self.finish_pending(db_path, &pending)?;
        match result {
            Ok(handle) => Ok(handle.clone()),
            Err(error) => {
                // connections: failed-open cleanup, paired with pending; a lost removal serves a dead handle (ADR-0013 D-2 D2-a)
                self.connections
                    .lock()
                    .map_err(|_| StoreError::LockPoisoned {
                        lock: "connections",
                        operation: "get_connection",
                    })?
                    .remove(db_path);
                Err(error)
            }
        }
    }

    /// Upstream per-open body: mkdir, connect (wrapped), FK pragma.
    async fn open_database(&self, db_path: &Path) -> Result<DbHandle, StoreError> {
        if let Some(parent) = db_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let encryption = crate::db::resolve_database_encryption(&self.config)?;
        let mut opts = ConnectOptions::for_config(&self.config, self.platform);
        opts.encryption = encryption.clone();
        let handle = match self.factory.connect(db_path.to_path_buf(), opts).await {
            Ok(handle) => handle,
            // An open-guard refusal is a typed decision about *this* open; wrapping
            // it would relabel it as a generic turso failure (45.7 would then be
            // false in production while green at the factory).
            Err(error) if error.is_open_guard_refusal() => return Err(error),
            Err(error) => {
                return Err(wrap_turso_open_error(
                    db_path,
                    error.message().as_str(),
                    self.platform,
                    encryption.is_some(),
                ));
            }
        };
        handle
            .execute(crate::sql::PRAGMA_FOREIGN_KEYS_ON, Vec::new())
            .await?;
        // connections: insert after a successful open; the other half of pending (ADR-0013 D-2 D2-a)
        self.connections
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "connections",
                operation: "open_database",
            })?
            .insert(db_path.to_path_buf(), handle.clone());
        Ok(handle)
    }

    /// Removes the pending entry once finished (upstream `finally`).
    fn finish_pending(&self, db_path: &Path, pending: &Arc<PendingOpen>) -> Result<(), StoreError> {
        // pending: get + Arc::ptr_eq + remove — completes the open protocol (ADR-0013 D-2 D2-a)
        let mut open = self.pending.lock().map_err(|_| StoreError::LockPoisoned {
            lock: "pending",
            operation: "finish_pending",
        })?;
        if let Some(occupied) = open.get(db_path)
            && Arc::ptr_eq(occupied, pending)
        {
            open.remove(db_path);
        }
        Ok(())
    }

    /// Upstream `closeConnection`: joins any in-flight open, removes and
    /// drops the handle (release), and dedupes concurrent closers through a
    /// shared per-path signal. A Rust `Drop` cannot fail, so the upstream
    /// close-error log branch is N-A.
    pub async fn close_connection(&self, db_path: &Path) -> Result<(), StoreError> {
        if let Some(closing) = self.global_close_signal()? {
            Signal::wait_completion(&closing).await;
        }
        let existing = {
            // closing_connections: read before installing a closer (ADR-0013 D-2 D2-a)
            self.closing_connections
                .lock()
                .map_err(|_| StoreError::LockPoisoned {
                    lock: "closing_connections",
                    operation: "close_connection",
                })?
                .get(db_path)
                .map(Arc::clone)
        };
        if let Some(existing) = existing {
            Signal::wait_completion(&existing).await;
            return Ok(());
        }
        let signal = Signal::new();
        // closing_connections: install the per-path close signal (ADR-0013 D-2 D2-a)
        self.closing_connections
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "closing_connections",
                operation: "close_connection",
            })?
            .insert(db_path.to_path_buf(), signal.clone());
        let close_result = self.close_one(db_path).await;
        // the signal completes and the entry leaves the map even when the close failed:
        // a stale entry would make every later close of this path wait forever
        signal.complete();
        // closing_connections: remove after the close completes; a lost removal pins the path as closing (ADR-0013 D-2 D2-a)
        let removal = self
            .closing_connections
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "closing_connections",
                operation: "close_connection",
            })
            .map(|mut closing| {
                closing.remove(db_path);
            });
        close_result?;
        removal?;
        Ok(())
    }

    async fn close_one(&self, db_path: &Path) -> Result<(), StoreError> {
        // allSettled semantics: an open failing must not abort the close.
        // pending: join the in-flight open before releasing the handle (ADR-0013 D-2 D2-a)
        let pending = self
            .pending
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "pending",
                operation: "close_one",
            })?
            .get(db_path)
            .map(Arc::clone);
        if let Some(pending) = pending {
            Signal::wait_completion(&pending.completed).await;
        }
        // connections: release on close; handle dropped outside the guard (ADR-0013 D-2 D2-a)
        let released = self
            .connections
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "connections",
                operation: "close_one",
            })?
            .remove(db_path);
        drop(released);
        Ok(())
    }

    /// Upstream `closeAll`: one shared global close (concurrent callers
    /// join it), awaits every pending open/close, drains all connections.
    pub async fn close_all(&self) -> Result<(), StoreError> {
        let owned = {
            // closing_all: claim-or-install the global close signal, dropped before the await (ADR-0013 D-2 D2-a)
            let mut closing = self
                .closing_all
                .lock()
                .map_err(|_| StoreError::LockPoisoned {
                    lock: "closing_all",
                    operation: "close_all",
                })?;
            match closing.as_ref() {
                Some(signal) => {
                    let signal = Arc::clone(signal);
                    drop(closing);
                    Ok(signal)
                }
                None => {
                    let signal = Signal::new();
                    *closing = Some(signal.clone());
                    Err(signal)
                }
            }
        };
        let signal = match owned {
            Ok(in_flight) => {
                Signal::wait_completion(&in_flight).await;
                return Ok(());
            }
            Err(signal) => signal,
        };
        let close_result = self.close_everything().await;
        // the flag must clear even when the drain failed, or every later open
        // awaits the global signal forever
        signal.complete();
        // closing_all: clear the global flag once the close completed (ADR-0013 D-2 D2-a)
        let cleared = self
            .closing_all
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "closing_all",
                operation: "close_all",
            })
            .map(|mut closing| {
                *closing = None;
            });
        close_result?;
        cleared?;
        Ok(())
    }

    async fn close_everything(&self) -> Result<(), StoreError> {
        let pending: Vec<Arc<PendingOpen>> = {
            // pending: snapshot of in-flight opens to await (ADR-0013 D-2 D2-a)
            self.pending
                .lock()
                .map_err(|_| StoreError::LockPoisoned {
                    lock: "pending",
                    operation: "close_everything",
                })?
                .values()
                .map(Arc::clone)
                .collect()
        };
        let closings: Vec<Arc<Signal>> = {
            // closing_connections: snapshot of in-flight closes to await (ADR-0013 D-2 D2-a)
            self.closing_connections
                .lock()
                .map_err(|_| StoreError::LockPoisoned {
                    lock: "closing_connections",
                    operation: "close_everything",
                })?
                .values()
                .map(Arc::clone)
                .collect()
        };
        for pending_open in pending {
            Signal::wait_completion(&pending_open.completed).await;
        }
        for closing in closings {
            Signal::wait_completion(&closing).await;
        }
        let drained: Vec<DbHandle> = {
            // connections: drain; a half-drained cache with live handles = reuse after close (ADR-0013 D-2 D2-a)
            let mut connections =
                self.connections
                    .lock()
                    .map_err(|_| StoreError::LockPoisoned {
                        lock: "connections",
                        operation: "close_everything",
                    })?;
            connections.drain().map(|(_, handle)| handle).collect()
        };
        drop(drained);
        // pending: clear only after every waiter was signalled (ADR-0013 D-2 D2-a)
        self.pending
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "pending",
                operation: "close_everything",
            })?
            .clear();
        Ok(())
    }

    /// True when a live handle is registered for the path.
    pub fn has_open_connection(&self, db_path: &Path) -> Result<bool, StoreError> {
        // connections: the answer decides whether the close side effect has landed (ADR-0013 D-2 D2-a)
        Ok(self
            .connections
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                lock: "connections",
                operation: "has_open_connection",
            })?
            .contains_key(db_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn test_manager() -> ConnectionManager {
        let storage_path = PathBuf::from("/nonexistent/opencode-mem-test");
        let config = StoreConfig {
            storage_path: storage_path.clone(),
            embedding_dimensions: 2,
            embedding_model: "test-model".to_string(),
            max_vectors_per_shard: 1000,
            container_tag_prefix: "opencode".to_string(),
            database_encryption_enabled: false,
            database_encryption_cipher: "aes256gcm".to_string(),
            database_encryption_key: None,
            allow_unguarded_open: false,
        };
        ConnectionManager::new(
            storage_path,
            config,
            Platform::current(),
            Arc::new(crate::db::TursoConnectFactory),
        )
    }

    /// Poison `mutex` on the current thread, leaving `value` inside it.
    fn poison<T>(mutex: &Mutex<T>) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = mutex.lock().expect("fresh lock");
            panic!("poison");
        }));
        assert!(mutex.is_poisoned(), "lock must be poisoned");
    }

    #[tokio::test]
    async fn test_close_all_propagates_poisoned_connections_lock() {
        let manager = test_manager();
        poison(&manager.connections);
        let error = manager
            .close_all()
            .await
            .expect_err("poisoned connections must refuse the close");
        assert!(matches!(
            error,
            StoreError::LockPoisoned {
                lock: "connections",
                ..
            }
        ));
    }

    #[test]
    fn test_has_open_connection_propagates_poisoned_lock() {
        let manager = test_manager();
        poison(&manager.connections);
        let error = manager
            .has_open_connection(Path::new("/nonexistent/opencode-mem-test/shard.db"))
            .expect_err("poisoned connections must refuse the query");
        assert!(matches!(
            error,
            StoreError::LockPoisoned {
                lock: "connections",
                ..
            }
        ));
    }
}
