//! SQLite/Turso database file sidecar operations + Windows file-lock retry
//! (port of upstream `src/services/turso/sqlite-handle-release.ts`).
//!
//! Upstream rules kept byte-for-byte in behavior:
//! - renaming only the main file would leave a stale `-wal` behind and drop
//!   commits -> main + `-wal`/`-shm`/`-tshm` move/copy/clear together;
//! - `-tshm` is Turso multiprocess-WAL shared memory;
//! - the retry actively waits at most `maxRetries` tries with the upstream
//!   delay table; non-lock errors and non-Windows platforms fail immediately.
//!
//! `collectReleasedSqliteHandles` (libsql 0.5.x GC pass, win32-only) is
//! N-A in Rust: handles release deterministically on `Drop`. The
//! observation point survives as [`RetryContext::on_retry`].

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use crate::delete_guard::FileClaim;
use crate::error::StoreError;
use crate::platform::{Platform, SleepFn};

// ---------------------------------------------------------------------------
// Sidecar moves
// ---------------------------------------------------------------------------

/// Turso/libSQL sidecar suffixes carried with every file operation.
pub const SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-tshm"];

/// Rename a SQLite/Turso database file together with its WAL/SHM/TSHM
/// sidecars — **only under exclusivity proofs for both ends**.
///
/// Both claims are probed **before the first mutation** (the main-file rename),
/// so a refusal leaves the main file and the destination's stale sidecars
/// untouched. Then the main file moves, and per sidecar the destination's stale
/// file is removed and the source's renamed in.
///
/// A claim covers its database and the three sidecars; the same argument as for
/// [`remove_sqlite_database`] makes one probe per end sound.
///
/// # Examples
///
/// Bare paths have no proof, so this call must not compile:
///
/// ```compile_fail
/// use std::path::Path;
/// use mem_store::sqlite_files::rename_sqlite_database;
///
/// let _ = rename_sqlite_database(Path::new("/tmp/from.db"), Path::new("/tmp/to.db"));
/// ```
///
/// Two claims from a live proof (the adjacent positive control):
///
/// ```
/// use std::sync::Arc;
///
/// use mem_store::delete_guard::ExclusiveRoot;
/// use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
/// use mem_store::platform::SystemProcessProbe;
/// use mem_store::sqlite_files::rename_sqlite_database;
///
/// let root = std::env::temp_dir().join(format!(
///     "mem-store-doctest-rename-{}",
///     std::process::id()
/// ));
/// std::fs::create_dir_all(&root).expect("create the root");
/// let lock = match acquire_engine_lock(&root, Arc::new(SystemProcessProbe)).expect("acquire") {
///     EngineLockOutcome::Acquired(lock) => lock,
///     EngineLockOutcome::Lost { .. } => panic!("a fresh temp root is free"),
/// };
/// let proof = ExclusiveRoot::new(lock);
/// let from = root.join("from.db");
/// let to = root.join("to.db");
/// std::fs::write(&from, b"main").expect("seed the source");
/// let from_claim = proof.claim(&from).expect("inside the root");
/// let to_claim = proof.claim(&to).expect("inside the root");
/// rename_sqlite_database(from_claim, to_claim).expect("both ends are free");
/// assert!(!from.exists());
/// assert_eq!(std::fs::read(&to).expect("target bytes"), b"main");
/// drop(proof);
/// let _ = std::fs::remove_dir_all(&root);
/// ```
pub fn rename_sqlite_database(from: FileClaim<'_>, to: FileClaim<'_>) -> Result<(), StoreError> {
    from.assert_exclusive()?;
    to.assert_exclusive()?;
    std::fs::rename(from.path(), to.path())?;
    for sidecar in sidecar_paths(from.path(), to.path()) {
        if sidecar.to.exists() {
            std::fs::remove_file(&sidecar.to)?;
        }
        if sidecar.from.exists() {
            std::fs::rename(&sidecar.from, &sidecar.to)?;
        }
    }
    Ok(())
}

/// Copy a SQLite/Turso database file together with its WAL/SHM/TSHM sidecars
/// (source stays intact) — **only the destination is claimed**.
///
/// The destination is probed **before its stale sidecars are dropped**, so a
/// refusal leaves them and the main file untouched. The source is a
/// **documented snapshot**, never mutated: a copy of a live source is a
/// point-in-time read, not an exclusive operation. Every production copy
/// destination is minted in the same call (`{db}.schema-v{v}-{millis}.bak`,
/// `{db}.pre-encrypt-*.bak`, `{db}.path-migrate-{nonce}.tmp`).
pub fn copy_sqlite_database(from: &Path, to: FileClaim<'_>) -> Result<(), StoreError> {
    to.assert_exclusive()?;
    copy_sqlite_files(from, to.path())
}

/// The copy body behind the claimed function.
fn copy_sqlite_files(from: &Path, to: &Path) -> Result<(), StoreError> {
    std::fs::copy(from, to)?;
    for sidecar in sidecar_paths(from, to) {
        if sidecar.to.exists() {
            std::fs::remove_file(&sidecar.to)?;
        }
        if sidecar.from.exists() {
            std::fs::copy(&sidecar.from, &sidecar.to)?;
        }
    }
    Ok(())
}

/// Remove a SQLite/Turso database file together with its WAL/SHM/TSHM
/// sidecars — **only under an exclusivity proof**.
///
/// # The contract (46.1 / 46.10)
///
/// A deletion is legal iff this process holds the storage root's engine lock
/// (the [`FileClaim`] only an [`crate::delete_guard::ExclusiveRoot`] can mint)
/// **and** the database-file probe inside this call answers `Free` or
/// `Missing`. `Held`/`Unknown` return [`StoreError::ExclusivityRefused`]
/// **before** the first `remove_file`, so a refused deletion leaves the
/// database and every sidecar untouched.
///
/// The mapping is cached and the suffixes fixed: the claimed path covers the
/// database and its three sidecars (see the module docs for why one probe on
/// the database is the sound proxy for all four). There is **no identity
/// re-check** between the probe and the unlink — the probe answers liveness,
/// not an inode witness — and the probe sits at the top of this function so
/// every retry attempt re-probes.
///
/// `Missing` is a **permit**, deliberately: a sidecar whose database is gone
/// cannot be found by the next opener, and refusing `Missing` would turn the
/// exists-guarded no-op deletions into errors. `NotFound` on any individual
/// unlink is still tolerated (upstream `existsSync` + `unlinkSync` semantics).
///
/// `-tshm` is turso-owned, format-versioned and runtime-resized; it is never a
/// liveness signal, and the engine never unlinks it as one — 45.A5's marker
/// lives in `.wal-mode`, never inside `-tshm`.
///
/// # Examples
///
/// A bare path has no proof, so this call must not compile — 46.2's structural
/// half:
///
/// ```compile_fail
/// use std::path::Path;
/// use mem_store::sqlite_files::remove_sqlite_database;
///
/// let _ = remove_sqlite_database(Path::new("/tmp/not-a-claim.db"));
/// ```
///
/// The proof is minted from a live engine lock (the adjacent positive control,
/// so a rename of the parameter cannot make the `compile_fail` pass vacuously):
///
/// ```
/// use std::sync::Arc;
///
/// use mem_store::delete_guard::ExclusiveRoot;
/// use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
/// use mem_store::platform::SystemProcessProbe;
/// use mem_store::sqlite_files::remove_sqlite_database;
///
/// let root = std::env::temp_dir().join(format!(
///     "mem-store-doctest-remove-{}",
///     std::process::id()
/// ));
/// std::fs::create_dir_all(&root).expect("create the root");
/// let lock = match acquire_engine_lock(&root, Arc::new(SystemProcessProbe)).expect("acquire") {
///     EngineLockOutcome::Acquired(lock) => lock,
///     EngineLockOutcome::Lost { .. } => panic!("a fresh temp root is free"),
/// };
/// let proof = ExclusiveRoot::new(lock);
/// let db = root.join("shard.db");
/// std::fs::write(&db, b"main").expect("seed the database");
/// let claim = proof.claim(&db).expect("the database is inside the root");
/// remove_sqlite_database(claim).expect("a free database is removed");
/// assert!(!db.exists());
/// drop(proof);
/// let _ = std::fs::remove_dir_all(&root);
/// ```
pub fn remove_sqlite_database(claim: FileClaim<'_>) -> Result<(), StoreError> {
    claim.assert_exclusive()?;
    unlink_sqlite_files(claim.path())
}

/// The unlink body behind the claimed function: database plus the three
/// sidecars, `NotFound` tolerated either way.
fn unlink_sqlite_files(db_path: &Path) -> Result<(), StoreError> {
    let mut targets = vec![db_path.to_path_buf()];
    for suffix in SIDECAR_SUFFIXES {
        targets.push(sidecar_path(db_path, suffix));
    }
    for target in targets {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

struct SidecarPaths {
    from: std::path::PathBuf,
    to: std::path::PathBuf,
}

fn sidecar_path(base: &Path, suffix: &str) -> std::path::PathBuf {
    let mut text = base.as_os_str().to_os_string();
    text.push(suffix);
    std::path::PathBuf::from(text)
}

fn sidecar_paths(from: &Path, to: &Path) -> Vec<SidecarPaths> {
    SIDECAR_SUFFIXES
        .iter()
        .map(|suffix| SidecarPaths {
            from: sidecar_path(from, suffix),
            to: sidecar_path(to, suffix),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Windows file-lock retry
// ---------------------------------------------------------------------------

/// The retryable win32 lock shapes (upstream `RETRYABLE_FILE_LOCK_CODES` =
/// `EBUSY | EPERM | EACCES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLockCode {
    Busy,
    Permission,
    Access,
}

/// Upstream `FILE_LOCK_RETRY_DELAYS_MS`, order preserved.
pub const FILE_LOCK_RETRY_DELAYS_MS: [u64; 8] = [25, 50, 100, 200, 400, 800, 1600, 3200];

/// The error side of the retry gate.
pub trait FileLockError {
    /// `ErrnoException.code` equivalent.
    fn lock_code(&self) -> Option<FileLockCode>;
}

/// `raw_os_error` translation. On Unix the upstream win32 gate means these
/// never retry, but the code mapping is platform-independent and tested.
/// Other platforms (non-unix, non-windows runners) are documented
/// divergence: upstream keys off `NodeJS.ErrnoException.code` strings; this
/// port maps raw os codes only where a stable libc constant exists.
impl FileLockError for std::io::Error {
    fn lock_code(&self) -> Option<FileLockCode> {
        // cfg(unix) raw codes: EBUSY = 16, EPERM = 1, EACCES = 13.
        #[cfg(unix)]
        {
            match self.raw_os_error() {
                Some(16) => Some(FileLockCode::Busy),
                Some(1) => Some(FileLockCode::Permission),
                Some(13) => Some(FileLockCode::Access),
                _ => None,
            }
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

impl FileLockError for crate::error::StoreError {
    fn lock_code(&self) -> Option<FileLockCode> {
        match self {
            crate::error::StoreError::Io {
                os_code: Some(code),
                ..
            } => {
                let io_error = std::io::Error::from_raw_os_error(*code);
                io_error.lock_code()
            }
            _ => None,
        }
    }
}

/// Retry plumbing: platform gate + injected sleep + the GC-pass observation
/// point (`collectReleasedSqliteHandles` has no Rust analog — Drop releases
/// handles — but the upstream call site order is preserved by invoking
/// `on_retry` before `sleep`, which keeps single-step test recordings
/// faithful).
pub struct RetryContext {
    pub platform: Platform,
    pub sleep: SleepFn,
    pub on_retry: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl RetryContext {
    /// Production context; `closeTursoAndInvalidateCaches` + GC are N-A.
    pub fn system() -> Self {
        Self {
            platform: Platform::current(),
            sleep: Arc::new(std::thread::sleep),
            on_retry: None,
        }
    }
}

/// Public alias used by the test helper (avoids exposing `RetryContext::system`
/// internals through the test crate).
pub fn system_retry_context() -> RetryContext {
    RetryContext::system()
}

/// Retries file mutations while native handles release. The operation runs
/// at most `max_retries + 1` times: the initial attempt plus up to
/// `max_retries` retries. Non-lock errors and non-Windows platforms fail
/// immediately. The default covers every entry in the delay table, matching
/// the pre-parameter behavior exactly (callers pass
/// `FILE_LOCK_RETRY_DELAYS_MS.len()`).
///
/// Upstream `maxRetries` validation (`Number.isInteger && >= 0`) is
/// type-enforced here: `usize` makes negatives and fractions
/// unrepresentable (recorded N-A).
pub fn with_sqlite_file_lock_retry<T, E>(
    mut operation: impl FnMut() -> Result<T, E>,
    max_retries: usize,
    ctx: &RetryContext,
) -> Result<T, E>
where
    E: FileLockError,
{
    for attempt in 0..=usize::MAX {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let retry_delay = FILE_LOCK_RETRY_DELAYS_MS.get(attempt).copied();
                if ctx.platform != Platform::Windows
                    || error.lock_code().is_none()
                    || attempt >= max_retries
                {
                    return Err(error);
                }
                // the delay table is exhausted: the transient lock error is final (ADR-0013 D-3)
                let Some(retry_delay) = retry_delay else {
                    return Err(error);
                };
                if let Some(on_retry) = &ctx.on_retry {
                    on_retry();
                }
                (ctx.sleep)(std::time::Duration::from_millis(retry_delay));
            }
        }
    }
    unreachable!("attempt counter bounded by the delay-table check")
}
