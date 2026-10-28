//! The delete proof (issue 46): **no unlink of a database or its sidecars
//! without a proof of exclusivity**.
//!
//! ## The contract (46.1 / 46.10)
//!
//! A deletion is legal iff, **in this process, at the moment of the first
//! unlink**:
//!
//! 1. this process holds the storage root's [`crate::engine_lock::EngineLock`]
//!    (the structural half — `create_new` on `.engine.lock` proves no *other
//!    engine* owns the root), and
//! 2. [`crate::open_guard::probe_database_file`] answers `Free` or `Missing`
//!    (the liveness half — proves no *legacy* single-process holder has the
//!    file). `Held` and `Unknown` refuse.
//!
//! [`ExclusiveRoot`] is the proof of (1); [`FileClaim`] is the proof of (2) for
//! one path, and only a live [`ExclusiveRoot`] can mint it. The probe runs
//! **inside each mutating function**, before its first unlink, so every retry
//! attempt re-probes instead of trusting an answer taken before a Windows lock
//! released (up to 3.2 s).
//!
//! ## What the proof is not
//!
//! * A foreign **multiprocess** holder is not detected by the probe (issue 45
//!   shipped Probe B only; measured in this wave's G2 arm: the probe answers
//!   `Free` under a live multiprocess child). Exclusivity is therefore "the
//!   engine lock plus the absence of a *legacy* holder"; the multiprocess
//!   residual is documented, never claimed away.
//! * There is no identity re-check between the probe and the unlink: the probe
//!   answers liveness, not an inode witness. A read-only open degrades to a
//!   refusal (it cannot satisfy the probe's write requirement and must not
//!   pretend to).
//! * `-tshm` is **turso-owned**, format-versioned and resized at runtime; it is
//!   never a liveness signal, and the engine never unlinks it as one. That is
//!   why 45.A5's mode marker lives in `.wal-mode` — never inside `-tshm`.
//!
//! ## Why one probe covers the three sidecars
//!
//! Turso can only hold `<db>-tshm` through an open `Database` on `<db>`; the
//! sidecar has no independent opener. So one probe on the database is the sound
//! proxy for the whole file set, which is why the sidecar unlinks need no
//! separate probes. `Missing` is a **permit**, deliberately: a `-tshm` whose
//! database is gone cannot be found by the next opener, and refusing `Missing`
//! would turn the exists-guarded no-op deletions into errors.

use std::path::Path;
use std::sync::Arc;

use crate::db::assert_path_inside_storage;
use crate::engine_lock::{self, EngineLock};
use crate::error::StoreError;
use crate::open_guard::{ProbeOutcome, probe_database_file};

/// A live exclusivity proof: this process holds `storage_root`'s engine lock.
///
/// The private `Arc<EngineLock>` **is** the registry entry
/// ([`EngineLock::storage_root`]); taking the lock by value would need
/// `Arc::try_unwrap`, which frees the allocation the registered `Weak` points
/// at, and [`ExclusiveRoot::current`] would then answer `None` for the proof
/// just built.
#[derive(Debug)]
pub struct ExclusiveRoot {
    lock: Arc<EngineLock>,
}

impl ExclusiveRoot {
    /// The only constructor: the same `Arc` the engine-lock registry holds.
    pub fn new(lock: Arc<EngineLock>) -> Self {
        Self { lock }
    }

    /// The storage root this proof covers.
    pub fn root(&self) -> &Path {
        self.lock.storage_root()
    }

    /// Mints a claim on `path`, which must be inside [`Self::root`].
    ///
    /// Containment only: **no I/O**. The probe runs inside the mutating
    /// function, never here, so a claim is cheap and a stale one cannot outlive
    /// its proof.
    pub fn claim<'a>(&'a self, path: &'a Path) -> Result<FileClaim<'a>, StoreError> {
        assert_path_inside_storage(self.root(), path)?;
        Ok(FileClaim { root: self, path })
    }

    /// The live proof this process registered for `root`, matched **exactly**
    /// (never by prefix: `/root/u` must not satisfy `/root/users`).
    pub fn current(root: &Path) -> Option<Self> {
        engine_lock::registered_lock(root).map(Self::new)
    }
}

/// One path covered by an [`ExclusiveRoot`]. There is **no public constructor**:
/// only [`ExclusiveRoot::claim`] can build one, so a bare path cannot reach a
/// mutating function.
#[derive(Clone, Copy, Debug)]
pub struct FileClaim<'a> {
    root: &'a ExclusiveRoot,
    path: &'a Path,
}

impl<'a> FileClaim<'a> {
    /// The claimed path.
    pub fn path(&self) -> &'a Path {
        self.path
    }

    /// The proof that minted this claim.
    pub fn root(&self) -> &'a ExclusiveRoot {
        self.root
    }

    /// Probes the claimed database. `Free`/`Missing` permit; `Held`/`Unknown`
    /// refuse with [`StoreError::ExclusivityRefused`]. Called by each mutating
    /// function before its first unlink.
    pub(crate) fn assert_exclusive(&self) -> Result<(), StoreError> {
        match probe_database_file(self.path) {
            ProbeOutcome::Free | ProbeOutcome::Missing => Ok(()),
            ProbeOutcome::Held => Err(StoreError::ExclusivityRefused {
                db_path: self.path.to_path_buf(),
                reason: "a live legacy (single-process) holder holds the database file".to_string(),
            }),
            ProbeOutcome::Unknown { errno } => Err(StoreError::ExclusivityRefused {
                db_path: self.path.to_path_buf(),
                reason: format!("the database-file probe could not answer (errno {errno:?})"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_lock::EngineLockOutcome;
    use crate::platform::SystemProcessProbe;

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mem-store-delete-guard-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn acquire(root: &Path) -> Arc<EngineLock> {
        match engine_lock::acquire_engine_lock(root, Arc::new(SystemProcessProbe))
            .expect("acquire over a free root")
        {
            EngineLockOutcome::Acquired(lock) => lock,
            EngineLockOutcome::Lost { holder_pid } => {
                panic!("a free temp root must be acquired, holder pid {holder_pid}")
            }
        }
    }

    /// 46.2: a claim needs a live engine lock, and the registry matches the
    /// **exact** root.
    ///
    /// Inverted world: skip registration in `acquire_engine_lock` → the first
    /// `current` is `None` and this fails; match roots by prefix → the
    /// `/root-u` probe below answers `Some` and the last assertion fails.
    #[test]
    fn test_delete_claim_is_constructible_from_a_live_engine_lock_only() {
        let root = temp_dir("claim");
        let other = temp_dir("claim-other");
        let lock = acquire(&root);
        let proof = ExclusiveRoot::new(lock);

        assert_eq!(proof.root(), root.as_path());
        assert!(
            ExclusiveRoot::current(&root).is_some(),
            "a live lock must be visible to the registry"
        );
        assert!(
            ExclusiveRoot::current(&other).is_none(),
            "a different root must not satisfy the exact-path lookup"
        );

        let db = root.join("shard.db");
        std::fs::write(&db, b"main").expect("seed database");
        let claim = proof.claim(&db).expect("a path inside the root");
        assert_eq!(claim.path(), db.as_path());

        let outside = other.join("escape.db");
        let escape = proof
            .claim(&outside)
            .expect_err("a path outside the root must refuse");
        assert!(matches!(escape, StoreError::PathOutsideStorage { .. }));

        // A prefix-shaped sibling is not the root.
        let sibling = std::path::PathBuf::from(format!("{}-u", root.display()));
        std::fs::create_dir_all(&sibling).expect("sibling dir");
        assert!(
            ExclusiveRoot::current(&sibling).is_none(),
            "the registry never matches by prefix"
        );

        drop(proof);
        assert!(
            ExclusiveRoot::current(&root).is_none(),
            "dropping the last handle withdraws the proof"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
        let _ = std::fs::remove_dir_all(&sibling);
    }
}
