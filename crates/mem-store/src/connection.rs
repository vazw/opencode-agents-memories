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

    fn global_close_signal(&self) -> Option<Arc<Signal>> {
        self.closing_all.lock().unwrap().as_ref().map(Arc::clone)
    }

    /// Upstream `getConnection` order, verbatim.
    pub async fn get_connection(&self, db_path: &Path) -> Result<DbHandle, StoreError> {
        // 1. await global close if in flight
        if let Some(closing) = self.global_close_signal() {
            Signal::wait_completion(&closing).await;
        }
        // 2. await per-path close if in flight
        let per_path_close = {
            self.closing_connections
                .lock()
                .unwrap()
                .get(db_path)
                .map(Arc::clone)
        };
        if let Some(closing) = per_path_close {
            Signal::wait_completion(&closing).await;
        }
        // 3. containment
        assert_path_inside_storage(&self.storage_path, db_path)?;
        // 4. cached handle
        if let Some(existing) = self.connections.lock().unwrap().get(db_path) {
            return Ok(existing.clone());
        }
        // 5. in-flight open: join the shared initializer
        let pending = {
            let mut pending = self.pending.lock().unwrap();
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
        self.finish_pending(db_path, &pending);
        match result {
            Ok(handle) => Ok(handle.clone()),
            Err(error) => {
                // Upstream catch: also drop any stale cached handle.
                self.connections.lock().unwrap().remove(db_path);
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
        self.connections
            .lock()
            .unwrap()
            .insert(db_path.to_path_buf(), handle.clone());
        Ok(handle)
    }

    /// Removes the pending entry once finished (upstream `finally`).
    fn finish_pending(&self, db_path: &Path, pending: &Arc<PendingOpen>) {
        let mut open = self.pending.lock().unwrap();
        if let Some(occupied) = open.get(db_path)
            && Arc::ptr_eq(occupied, pending)
        {
            open.remove(db_path);
        }
    }

    /// Upstream `closeConnection`: joins any in-flight open, removes and
    /// drops the handle (release), and dedupes concurrent closers through a
    /// shared per-path signal. A Rust `Drop` cannot fail, so the upstream
    /// close-error log branch is N-A.
    pub async fn close_connection(&self, db_path: &Path) {
        if let Some(closing) = self.global_close_signal() {
            Signal::wait_completion(&closing).await;
        }
        let existing = {
            self.closing_connections
                .lock()
                .unwrap()
                .get(db_path)
                .map(Arc::clone)
        };
        if let Some(existing) = existing {
            Signal::wait_completion(&existing).await;
            return;
        }
        let signal = Signal::new();
        self.closing_connections
            .lock()
            .unwrap()
            .insert(db_path.to_path_buf(), signal.clone());
        self.close_one(db_path).await;
        signal.complete();
        self.closing_connections.lock().unwrap().remove(db_path);
    }

    async fn close_one(&self, db_path: &Path) {
        // allSettled semantics: an open failing must not abort the close.
        let pending = self.pending.lock().unwrap().get(db_path).map(Arc::clone);
        if let Some(pending) = pending {
            Signal::wait_completion(&pending.completed).await;
        }
        let released = self.connections.lock().unwrap().remove(db_path);
        drop(released);
    }

    /// Upstream `closeAll`: one shared global close (concurrent callers
    /// join it), awaits every pending open/close, drains all connections.
    pub async fn close_all(&self) {
        let owned = {
            let mut closing = self.closing_all.lock().unwrap();
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
                return;
            }
            Err(signal) => signal,
        };
        self.close_everything().await;
        signal.complete();
        *self.closing_all.lock().unwrap() = None;
    }

    async fn close_everything(&self) {
        let pending: Vec<Arc<PendingOpen>> = self
            .pending
            .lock()
            .unwrap()
            .values()
            .map(Arc::clone)
            .collect();
        let closings: Vec<Arc<Signal>> = self
            .closing_connections
            .lock()
            .unwrap()
            .values()
            .map(Arc::clone)
            .collect();
        for pending_open in pending {
            Signal::wait_completion(&pending_open.completed).await;
        }
        for closing in closings {
            Signal::wait_completion(&closing).await;
        }
        let drained: Vec<DbHandle> = {
            let mut connections = self.connections.lock().unwrap();
            connections.drain().map(|(_, handle)| handle).collect()
        };
        drop(drained);
        self.pending.lock().unwrap().clear();
    }

    /// True when a live handle is registered for the path.
    pub fn has_open_connection(&self, db_path: &Path) -> bool {
        self.connections.lock().unwrap().contains_key(db_path)
    }
}
