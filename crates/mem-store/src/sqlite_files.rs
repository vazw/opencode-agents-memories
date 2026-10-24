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

use crate::platform::{Platform, SleepFn};

// ---------------------------------------------------------------------------
// Sidecar moves
// ---------------------------------------------------------------------------

/// Turso/libSQL sidecar suffixes carried with every file operation.
pub const SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-tshm"];

/// Rename a SQLite/Turso database file together with its WAL/SHM/TSHM
/// sidecars. A stale sidecar at the target is removed first (leaving one
/// behind drops commits). `NotFound` on incidental unlink targets is
/// tolerated (upstream `existsSync` + `unlinkSync` semantics).
pub fn rename_sqlite_database(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)?;
    for sidecar in sidecar_paths(from, to) {
        if sidecar.to.exists() {
            std::fs::remove_file(&sidecar.to)?;
        }
        if sidecar.from.exists() {
            std::fs::rename(&sidecar.from, &sidecar.to)?;
        }
    }
    Ok(())
}

/// Copy a SQLite/Turso database file together with its WAL/SHM/TSHM
/// sidecars (source stays intact).
pub fn copy_sqlite_database(from: &Path, to: &Path) -> std::io::Result<()> {
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

/// Remove a SQLite/Turso database file together with WAL/SHM/TSHM sidecars
/// (`NotFound` tolerated either way — upstream unconditional-ish unlink).
pub fn remove_sqlite_database(db_path: &Path) -> std::io::Result<()> {
    let mut targets = vec![db_path.to_path_buf()];
    for suffix in SIDECAR_SUFFIXES {
        targets.push(sidecar_path(db_path, suffix));
    }
    for target in targets {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
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
