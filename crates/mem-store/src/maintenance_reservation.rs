//! Maintenance reservation — the startup fence (rev-11 §E.2, ADR-0033).
//!
//! A consolidation window owns the storage root exclusively while its
//! reservation file exists at the PARENT of the root
//! (`<parent>/.opencode-mem-maintenance.lock`). The file lives OUTSIDE the
//! root so it survives the whole-root rename, and every process that could
//! write the root consults it BEFORE its first write:
//!
//! * the engine — one check at the top of `run_in_runtime`, before
//!   `acquire_engine_lock`'s `create_dir_all` (the ordering hazard the fence
//!   exists for: a refused startup creates NOTHING);
//! * the four CLI writer modes (purge-session, reembed-scope, reap-shards,
//!   sweep) — a check before their first store open.
//!
//! The dispatch is three-way (F9): `Absent` proceeds; `HeldBySelf` (the
//! recorded pid is this process AND its start-time token is this process's
//! token) proceeds as the holder; anything else refuses with
//! [`RESERVATION_HELD`] (76). Presence alone bars startup — a corrupt or
//! 0-byte body is `Held`, never "no reservation", and nothing ever
//! auto-reclaims a reservation (deliberately unlike `.engine.lock`, whose
//! reclaim-by-startup is exactly the whole-root-rename gap this fence
//! closes).
//!
//! The reservation body is L1-durable: exclusive-create at mode 0600, write,
//! `fsync(file)`, `fsync(parent dir)`. The exit-76 structured line carries
//! ONLY `operation` + `root_basename` (F11): nonce, start-time token and pid
//! never leave the 0600 body.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The reservation file name, at the PARENT of the storage root.
pub const RESERVATION_FILE: &str = ".opencode-mem-maintenance.lock";

/// The reserved operation recorded in every body this build writes.
pub const RESERVATION_OPERATION: &str = "maintenance";

/// Exit code: a maintenance reservation is held by a foreign holder.
pub const RESERVATION_HELD: i32 = 76;

/// Nonce width in bytes (`getrandom::fill`), base64 in the body.
const NONCE_BYTES: usize = 16;

/// The on-disk reservation body.
///
/// `start_time` is the holder's process start-time token (`None` where the
/// platform cannot observe one); `retake_seq` exists only after a retake;
/// `s_dev`/`s_ino` pin the storage root identity recorded AT ACQUIRE (the
/// evidence the pre-journal adopt row compares against, §H.1 EV-S).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReservationBody {
    /// The holder's pid.
    pub pid: u32,
    /// The holder's process start-time token (defeats pid reuse when present).
    #[serde(default)]
    pub start_time: Option<u64>,
    /// Always [`RESERVATION_OPERATION`] for this build.
    pub operation: String,
    /// The storage root's basename (one reservation filename per parent).
    pub root_basename: String,
    /// 16 CSPRNG bytes, standard base64 (window identity, not a secret).
    pub nonce: String,
    /// RFC 3339 UTC creation stamp.
    pub created_utc: String,
    /// The retake counter after an atomic-replace retake (§H/L7).
    #[serde(default)]
    pub retake_seq: Option<u64>,
    /// The storage root's `dev` at acquire.
    #[serde(default)]
    pub s_dev: Option<u64>,
    /// The storage root's `ino` at acquire.
    #[serde(default)]
    pub s_ino: Option<u64>,
}

/// The three-way dispatch verdict (F9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationVerdict {
    /// No reservation file at the parent: proceed.
    Absent,
    /// The reservation names THIS process (pid + start-time token): proceed as
    /// the holder.
    HeldBySelf,
    /// Any other state — a foreign holder, a corrupt body, a 0-byte body, an
    /// unreadable file: refuse. Never auto-reclaimed.
    Held,
}

/// A reservation this process just acquired.
#[derive(Debug)]
pub struct AcquiredReservation {
    path: PathBuf,
    body: ReservationBody,
}

impl AcquiredReservation {
    /// The reservation file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The durable body exactly as written.
    pub fn body(&self) -> &ReservationBody {
        &self.body
    }
}

/// Why an acquire failed. Callers choose the exit family: the consolidator
/// answers an existing reservation with 77 (it never retakes), while a
/// startup writer answers a Held verdict with 76.
#[derive(Debug)]
pub enum ReservationError {
    /// The reservation file already exists. Never overwritten here.
    AlreadyExists,
    /// The storage root is not an inspectable directory, so its `dev`/`ino`
    /// identity cannot be pinned.
    RootUnreadable(std::io::Error),
    /// Any other filesystem failure.
    Io(std::io::Error),
}

impl std::fmt::Display for ReservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyExists => write!(f, "reservation already exists"),
            Self::RootUnreadable(error) => {
                write!(f, "storage root cannot be inspected: {error}")
            }
            Self::Io(error) => write!(f, "reservation io failure: {error}"),
        }
    }
}

impl std::error::Error for ReservationError {}

/// The reservation path for `storage_root`: the parent directory joined with
/// [`RESERVATION_FILE`]. A root with no parent has none.
pub fn reservation_path(storage_root: &Path) -> Option<PathBuf> {
    storage_root
        .parent()
        .map(|parent| parent.join(RESERVATION_FILE))
}

/// The three-way dispatch check. Never mutates and never unlinks: this is the
/// startup fence, not a lock reclaimer.
pub fn check(storage_root: &Path) -> ReservationVerdict {
    let Some(path) = reservation_path(storage_root) else {
        return ReservationVerdict::Absent;
    };
    let body_text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ReservationVerdict::Absent;
        }
        // Unreadable-but-present is fail-closed: presence alone bars startup.
        Err(_) => return ReservationVerdict::Held,
    };
    let Ok(body) = serde_json::from_str::<ReservationBody>(&body_text) else {
        // A corrupt or 0-byte body is Held, never "no reservation".
        return ReservationVerdict::Held;
    };
    let own_pid = std::process::id();
    let own_token = crate::platform::process_start_time(own_pid);
    if body.pid == own_pid && body.start_time.is_some() && body.start_time == own_token {
        return ReservationVerdict::HeldBySelf;
    }
    ReservationVerdict::Held
}

/// Acquires the reservation with the pinned L1 ordering: exclusive-create at
/// 0600 → write body → `fsync(file)` → `fsync(parent dir)`. The reservation is
/// not considered held until the parent fsync returns.
pub fn acquire(storage_root: &Path) -> Result<AcquiredReservation, ReservationError> {
    let Some(path) = reservation_path(storage_root) else {
        return Err(ReservationError::RootUnreadable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the storage root has no parent directory",
        )));
    };
    let metadata = fs::metadata(storage_root).map_err(ReservationError::RootUnreadable)?;
    if !metadata.is_dir() {
        return Err(ReservationError::RootUnreadable(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "the storage root is not a directory",
        )));
    }
    let parent = path.parent().ok_or_else(|| {
        ReservationError::RootUnreadable(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the reservation path has no parent directory",
        ))
    })?;
    let body = ReservationBody {
        pid: std::process::id(),
        start_time: crate::platform::process_start_time(std::process::id()),
        operation: RESERVATION_OPERATION.to_string(),
        root_basename: storage_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        nonce: fresh_nonce()?,
        created_utc: chrono::Utc::now().to_rfc3339(),
        retake_seq: None,
        s_dev: Some(platform_dev(&metadata)),
        s_ino: Some(platform_ino(&metadata)),
    };
    let encoded = serde_json::to_string(&body).map_err(|error| {
        ReservationError::Io(std::io::Error::other(format!(
            "reservation body serialization failed: {error}"
        )))
    })?;

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ReservationError::AlreadyExists);
        }
        Err(error) => return Err(ReservationError::Io(error)),
    };
    file.write_all(encoded.as_bytes())
        .map_err(ReservationError::Io)?;
    file.sync_all().map_err(ReservationError::Io)?;
    drop(file);
    // The reservation is not held until the directory entry itself is durable.
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(ReservationError::Io)?;
    Ok(AcquiredReservation { path, body })
}

/// The effective journal phase a release or adopt is judged against (§E.2).
/// Only the two terminal phases may release the reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasePhase {
    /// Terminal success: the installed tree is settled.
    Verified,
    /// Terminal rollback: the original tree is restored.
    RolledBack,
    /// Any non-terminal phase: never releasable.
    Installing,
}

/// The one-record journal a pre-journal adopt synthesizes (§E.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedRecord {
    kind: &'static str,
    reason: &'static str,
}

impl AdoptedRecord {
    /// The record kind (`RolledBack`).
    #[must_use]
    pub fn record_kind(&self) -> &'static str {
        self.kind
    }

    /// The rollback reason (`pre_journal_abandon`).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

/// Reads the reservation body at the parent, fail-closed on any unreadable or
/// corrupt state. Returns `None` when no reservation exists.
fn read_body(storage_root: &Path) -> Result<Option<(PathBuf, ReservationBody)>, std::io::Error> {
    let Some(path) = reservation_path(storage_root) else {
        return Ok(None);
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let body = serde_json::from_str::<ReservationBody>(&text)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(Some((path, body)))
}

/// Holder liveness for release and adopt (§E.2): dead when the pid is gone or
/// its start-time token differs; a `start_time`-less body with a present pid
/// is never releasable.
fn holder_is_dead(body: &ReservationBody) -> bool {
    let observed = crate::platform::observe_process(body.pid);
    match observed {
        crate::platform::ObservedProcess::Absent => true,
        crate::platform::ObservedProcess::Present { start_time } => {
            match (body.start_time, start_time) {
                (Some(recorded), Some(live)) => recorded != live,
                // Unverifiable or token-less body with a present pid: alive.
                _ => false,
            }
        }
    }
}

/// Releases the reservation: nonce match, terminal journal phase, dead holder,
/// then unlink after a byte-equal re-read (defense in depth, §E.2).
pub fn release(storage_root: &Path, nonce: &str, phase: ReleasePhase) -> Result<(), String> {
    if !matches!(phase, ReleasePhase::Verified | ReleasePhase::RolledBack) {
        return Err("journal phase is not terminal".to_string());
    }
    let Some((path, body)) = read_body(storage_root).map_err(|error| error.to_string())? else {
        return Err("no reservation".to_string());
    };
    if body.nonce != nonce {
        return Err("nonce mismatch".to_string());
    }
    if !holder_is_dead(&body) {
        return Err("holder is alive".to_string());
    }
    let first = fs::read(&path).map_err(|error| error.to_string())?;
    let second = fs::read(&path).map_err(|error| error.to_string())?;
    if first != second {
        return Err("reservation changed during release".to_string());
    }
    fs::remove_file(&path).map_err(|error| error.to_string())
}

/// Adopts an orphaned reservation whose journal was never written (§E.2): the
/// nonce must match, the holder must be dead, and the caller guarantees the
/// journal is ABSENT. The synthesized record is one `RolledBack` with reason
/// `pre_journal_abandon`; nothing is written to the storage root.
pub fn adopt_pre_journal(storage_root: &Path, nonce: &str) -> Result<AdoptedRecord, String> {
    let Some((_, body)) = read_body(storage_root).map_err(|error| error.to_string())? else {
        return Err("no reservation".to_string());
    };
    if body.nonce != nonce {
        return Err("nonce mismatch".to_string());
    }
    if !holder_is_dead(&body) {
        return Err("holder is alive".to_string());
    }
    Ok(AdoptedRecord {
        kind: "RolledBack",
        reason: "pre_journal_abandon",
    })
}

/// Emits the exit-76 structured line and exits. The payload carries ONLY
/// `operation` + `root_basename` — never the nonce, start-time token or pid
/// (F11 leak rule).
pub fn refuse_held_exit(operation: &str, storage_root: &Path) -> ! {
    let root_basename = storage_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    mem_core::diagnostics::structured_stderr(
        "maintenance.reservation_held",
        "error",
        "maintenance",
        crate::serde_json::json!({
            "operation": operation,
            "root_basename": root_basename,
        }),
    );
    std::process::exit(RESERVATION_HELD);
}

/// 16 CSPRNG bytes, standard base64 (the token-mint source, §E.2).
fn fresh_nonce() -> Result<String, ReservationError> {
    let mut bytes = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| {
        ReservationError::Io(std::io::Error::other(format!(
            "reservation nonce source failed: {error}"
        )))
    })?;
    Ok(base64_encode(&bytes))
}

#[cfg(unix)]
fn platform_dev(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.dev()
}

#[cfg(not(unix))]
fn platform_dev(_metadata: &fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn platform_ino(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

#[cfg(not(unix))]
fn platform_ino(_metadata: &fs::Metadata) -> u64 {
    0
}

/// Standard base64 (RFC 4648 alphabet, `=` padding). Hand-rolled: the
/// workspace's `base64` crate is not a `mem-store` dependency and the
/// blueprint sanctions exactly one new consumer edge here (`getrandom`).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(third & 0b11_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            base64_encode(&[0_u8; NONCE_BYTES]),
            "AAAAAAAAAAAAAAAAAAAAAA=="
        );
    }
}
