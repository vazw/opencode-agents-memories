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
const RESERVATION_FILE: &str = ".opencode-mem-maintenance.lock";

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// The storage root or its parent fails the F12 ownership/mode preflight
    /// (real dir, owner-owned, not group/other-writable).
    RootPermissionsInvalid,
    /// A live `.engine.lock` holder is not the expected pre-quiesce engine
    /// (F7 / F13 / F22a).
    ForeignEngineHolder,
    /// An active writer or open-fd holder owns the storage root.
    ActiveWriter,
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
            Self::RootPermissionsInvalid => {
                write!(f, "the storage root fails the ownership preflight")
            }
            Self::ForeignEngineHolder => {
                write!(f, "a live foreign engine holds the storage root")
            }
            Self::ActiveWriter => write!(f, "an active writer holds the storage root"),
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
fn reservation_path(storage_root: &Path) -> Option<PathBuf> {
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

/// F12 (rev-15 §G S1a): the parent of the storage root and the storage root
/// itself must be real directories owned by the running user and not
/// group/other-writable before a reservation is minted beside them.
#[cfg(unix)]
fn check_root_permissions(root: &Path) -> Result<(), ReservationError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = fs::symlink_metadata(root).map_err(ReservationError::RootUnreadable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ReservationError::RootPermissionsInvalid);
    }
    if metadata.uid() != unsafe { libc::geteuid() } as u32 {
        return Err(ReservationError::RootPermissionsInvalid);
    }
    if (metadata.permissions().mode() & 0o022) != 0 {
        return Err(ReservationError::RootPermissionsInvalid);
    }
    Ok(())
}

/// §E.2 S1a refusal 2: a live `.engine.lock` holder that fails the F7 minimal
/// identity test (NUL-split argv pair, byte-equal storage root, no inline form,
/// mode ∈ {stdio, standalone}) is a foreign engine and refuses the acquire.
fn check_foreign_engine_holder(
    lexical_root: &Path,
    storage_root: &Path,
) -> Result<(), ReservationError> {
    let contents = match fs::read_to_string(storage_root.join(crate::engine_lock::ENGINE_LOCK_FILE))
    {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(ReservationError::ForeignEngineHolder),
    };
    if contents.trim().is_empty() {
        return Err(ReservationError::ForeignEngineHolder);
    }
    let Ok(record) = serde_json::from_str::<crate::operation_lock::LockRecord>(&contents) else {
        return Err(ReservationError::ForeignEngineHolder);
    };
    if record.state.pid == 0 {
        return Err(ReservationError::ForeignEngineHolder);
    }
    if record.start_time.is_none_or(|token| token == 0) {
        // A record without a start-time token cannot be proven foreign-or-safe:
        // fail closed.
        return Err(ReservationError::ForeignEngineHolder);
    }
    let observed = crate::platform::observe_process(record.state.pid);
    let liveness = crate::platform::decide_liveness(
        record.state.pid,
        record.start_time,
        observed,
        crate::platform::ProcObservability::current(),
    );
    if liveness.is_alive()
        && !crate::platform::is_expected_live_engine(
            record.state.pid,
            record.start_time.unwrap_or(0),
            lexical_root,
            storage_root,
        )
    {
        return Err(ReservationError::ForeignEngineHolder);
    }
    Ok(())
}

/// §E.2 S1a refusal 3: an operation/migration lock or an open-fd holder under
/// the storage root means a writer is live and refuses the acquire.
fn check_active_writer(storage_root: &Path) -> Result<(), ReservationError> {
    for file in [
        crate::operation_lock::OPERATION_LOCK_FILE,
        crate::operation_lock::LEGACY_MIGRATION_LOCK_FILE,
    ] {
        if crate::operation_lock::read_live_lock(
            storage_root,
            file,
            &crate::platform::SystemProcessProbe,
        )
        .is_some()
        {
            return Err(ReservationError::ActiveWriter);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if live_process_holds_fd_under(storage_root, std::process::id()) {
            return Err(ReservationError::ActiveWriter);
        }
    }
    Ok(())
}

/// The `comm` value of OpenSSH's per-session leader — the one class exempted
/// from the unreadable-`fd/` refusal (see [`comm_is_sshd_session`]).
#[cfg(target_os = "linux")]
const SSHD_SESSION_COMM: &str = "sshd-session";

/// True when `/proc/<pid>/comm` is exactly `sshd-session`.
///
/// This is a **heuristic for one measured class, not an allowlist and not a
/// trust signal**: any same-uid process can become "exempt" with one
/// `prctl(PR_SET_NAME, "sshd-session")` call. An unreadable `comm` answers
/// `false` (refuse), by construction.
#[cfg(target_os = "linux")]
fn comm_is_sshd_session(proc: &Path, pid_text: &str) -> bool {
    std::fs::read_to_string(proc.join(pid_text).join("comm"))
        .is_ok_and(|comm| comm.trim_end_matches('\n') == SSHD_SESSION_COMM)
}

/// True when the task state parsed from an ALREADY-read `/proc/<pid>/status`
/// string is `Z` (zombie).
///
/// The parser anchors on the LAST `State:`-keyed line: task state is
/// kernel-owned and not settable by any `prctl`, while the only
/// user-controlled text in `status` is `comm` on the first `Name:` line
/// (`PR_SET_NAME` permits an embedded `\n`). A forged `State:` line injected
/// through `comm` therefore precedes the kernel's real one, and last-match
/// ignores it. A zombie's descriptor table was destroyed at exit, so this is
/// evidence it holds no descriptor — unlike the [`comm_is_sshd_session`]
/// name heuristic.
#[cfg(target_os = "linux")]
fn status_is_zombie(status: &str) -> bool {
    status
        .lines()
        .rev()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| *k == "State")
                .map(|(_, v)| v)
        })
        .is_some_and(|v| v.trim_start().starts_with('Z'))
}

/// §E.2 S1a refusal 3 (open descriptors): walk `/proc/<pid>/fd` across
/// same-uid live processes and refuse when one holds a descriptor resolving
/// under the storage root. Fail closed on any unreadable directory, with two
/// measured exceptions, both skipped: state-`Z` zombies (provably holder-free
/// — MEASURED kernel 6.18.39_1 — a zombie's descriptor table is destroyed at
/// exit) and processes whose `comm` is exactly `sshd-session` (OpenSSH's
/// per-session leader; a heuristic, see [`comm_is_sshd_session`]).
#[cfg(target_os = "linux")]
fn live_process_holds_fd_under(storage_root: &Path, skip_pid: u32) -> bool {
    let canonical =
        std::fs::canonicalize(storage_root).unwrap_or_else(|_| storage_root.to_path_buf());
    let proc = std::path::PathBuf::from("/proc");
    let entries = match std::fs::read_dir(&proc) {
        Ok(entries) => entries,
        Err(_) => return true,
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid_text) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        if pid == skip_pid || pid == std::process::id() {
            continue;
        }
        // Same-uid only: never judge another user's descriptors.
        let Ok(status) = std::fs::read_to_string(proc.join(pid_text).join("status")) else {
            continue;
        };
        let euid = unsafe { libc::geteuid() }.to_string();
        let same_uid = status.lines().any(|line| {
            let Some((key, value)) = line.split_once(':') else {
                return false;
            };
            key == "Uid"
                && value
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|field| field == euid.as_str())
        });
        if !same_uid {
            continue;
        }
        let fd_dir = proc.join(pid_text).join("fd");
        let Ok(fds) = std::fs::read_dir(&fd_dir) else {
            // Invariant at this loop top: every not-yet-skipped same-uid pid
            // scanned so far either held no descriptor under the canonical
            // root or was refused — `true` is returned only for a pid that is
            // same-uid, non-exempt, and not provably holder-free. Each
            // `/proc` entry is visited once and every branch returns or
            // continues, so termination is unchanged.
            //
            // A state-`Z` zombie's descriptor table was destroyed at exit, so
            // it cannot hold a descriptor under the root; skipped, not
            // refused. State is kernel-owned (not settable by any prctl), and
            // this parses the SAME `status` snapshot the uid check read — no
            // second read, no new TOCTOU window.
            //
            // The live-nondumpable class is still refused: OpenSSH sets
            // PR_SET_DUMPABLE=0 on its per-session leader, which makes that
            // process's fd/ unreadable even to its own uid; the class was
            // MEASURED holding no descriptor under the root, so it is skipped
            // instead of refused. Every other unreadable process still fails
            // closed.
            //
            // This is not a security boundary and must not be sold as one: a
            // same-uid process can write the storage root directly, with or
            // without a descriptor this function can see. The arm exists to
            // catch *accidental* writers. The exemptions are also blind by
            // construction (an sshd-session leader that DID hold a descriptor
            // would be skipped too) and the name half self-assignable by any
            // same-uid process via prctl(PR_SET_NAME, "sshd-session"): the
            // name match is a heuristic for one measured class, not an
            // allowlist and not a trust signal.
            if status_is_zombie(&status) {
                continue;
            }
            if comm_is_sshd_session(&proc, pid_text) {
                continue;
            }
            return true;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            if target.starts_with(&canonical) {
                return true;
            }
        }
    }
    false
}

/// Acquires the reservation with the pinned L1 ordering: exclusive-create at
/// 0600 → write body → `fsync(file)` → `fsync(parent dir)`. The reservation is
/// not considered held until the parent fsync returns.
///
/// `lexical_root` is the `--storage-root` spelling as typed (before
/// canonicalize); F7 admits the holder's argv only if it spells that exact
/// string or the canonical root (O3).
pub fn acquire(
    lexical_root: &Path,
    storage_root: &Path,
) -> Result<AcquiredReservation, ReservationError> {
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
    // F12 (rev-15 §G S1a): the parent of the storage root and the root itself
    // must be real, owner-owned, not group/other-writable directories before a
    // reservation is minted into the parent.
    #[cfg(unix)]
    check_root_permissions(parent)?;
    #[cfg(unix)]
    check_root_permissions(storage_root)?;
    // §E.2 S1a acquire refusals, before the exclusive-create: a live foreign
    // engine holder (F7) or an active writer never gets a reservation minted.
    check_foreign_engine_holder(lexical_root, storage_root)?;
    check_active_writer(storage_root)?;
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

/// The admission verdict for one consolidation window (§E.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowAdmissionVerdict {
    /// No reservation file at the parent: the window is not held (FS-3 → 77).
    Absent,
    /// The reservation names THIS process (pid + start-time token): proceed as
    /// the holder.
    HeldBySelf(ReservationBody),
    /// The env nonce matches the reservation body: proceed under the recovery
    /// flock (the custodian's admit path).
    AdmittedByNonce(ReservationBody),
    /// Any other state — a foreign holder, a corrupt body, a 0-byte body, an
    /// unset/empty/mismatched nonce: refuse (76).
    HeldRefused,
}

/// Holder liveness for release and adopt (§E.2): dead when the pid is gone or
/// its start-time token differs; a `start_time`-less body with a present pid is
/// never releasable. Off Linux nothing is verifiable, so fail closed.
#[cfg(target_os = "linux")]
pub fn holder_is_dead(body: &ReservationBody) -> bool {
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

/// Off Linux the holder can never be proven dead (fail closed).
#[cfg(not(target_os = "linux"))]
pub fn holder_is_dead(_body: &ReservationBody) -> bool {
    false
}

/// The reservation at `storage_root` with its raw bytes: `None` when absent,
/// `Err` when corrupt or unreadable (fail closed downstream).
pub fn reservation_bytes(
    storage_root: &Path,
) -> Result<Option<(PathBuf, ReservationBody, Vec<u8>)>, std::io::Error> {
    let Some(path) = reservation_path(storage_root) else {
        return Ok(None);
    };
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let text = std::str::from_utf8(&raw)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let body = serde_json::from_str::<ReservationBody>(text)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(Some((path, body, raw)))
}

/// The window admission verdict for one storage root under the recovery flock
/// (§E.2): `Absent` → the window is not held (FS-3 → 77); `HeldRefused` → a
/// foreign/empty/mismatched holder (76); `AdmittedByNonce` / `HeldBySelf` → the
/// caller may proceed while holding the flock.
pub fn check_for_window(storage_root: &Path, env_nonce: Option<&str>) -> WindowAdmissionVerdict {
    match reservation_bytes(storage_root) {
        Ok(Some((_, body, _raw))) => {
            if body.nonce.is_empty() {
                return WindowAdmissionVerdict::HeldRefused;
            }
            if let Some(nonce) = env_nonce
                && !nonce.is_empty()
                && nonce == body.nonce
            {
                return WindowAdmissionVerdict::AdmittedByNonce(body);
            }
            let own_pid = std::process::id();
            let own_token = crate::platform::process_start_time(own_pid);
            if body.pid == own_pid && body.start_time.is_some() && body.start_time == own_token {
                return WindowAdmissionVerdict::HeldBySelf(body);
            }
            WindowAdmissionVerdict::HeldRefused
        }
        Ok(None) => WindowAdmissionVerdict::Absent,
        Err(_) => WindowAdmissionVerdict::HeldRefused,
    }
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

    /// A live holder (this process, its real start-time token) is never dead.
    /// Mutant: `holder_is_dead` returning `true` for a live holder goes RED here.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_holder_is_dead_is_false_for_live_holder() {
        let pid = std::process::id();
        let body = ReservationBody {
            pid,
            start_time: crate::platform::process_start_time(pid),
            operation: RESERVATION_OPERATION.to_string(),
            root_basename: "data".to_string(),
            nonce: "AAAAAAAAAAAAAAAAAAAAAA==".to_string(),
            created_utc: "2026-11-14T00:00:00+00:00".to_string(),
            retake_seq: None,
            s_dev: None,
            s_ino: None,
        };
        assert!(body.start_time.is_some(), "live token readable");
        assert!(!holder_is_dead(&body), "a live holder is not dead");
    }

    /// Mutants separated by these vectors: key matched as `"state"` (case
    /// miss) reds on vector 1; `trim_start` dropped reds on vector 1 (leading
    /// `\t`); inverted comparator reds on vectors 2/4/5/6; first-match instead
    /// of last-match reds on vector 3 (comm-injection via `Name:`, forged `Z`
    /// before the kernel's real `State:` line — must stay false).
    #[cfg(target_os = "linux")]
    #[test]
    fn test_status_is_zombie_parses_proc_status_state_line() {
        // Real-shaped zombie status: kernel State line, later keys follow.
        assert!(status_is_zombie(
            "Name:\tsh\nState:\tZ (zombie)\nTgid:\t42\n"
        ));
        // Sleeping live process.
        assert!(!status_is_zombie("State:\tS (sleeping)\n"));
        // comm-injection vector: forged `Z` line precedes the real State line;
        // last-match must answer false.
        assert!(!status_is_zombie(
            "Name:\tx\nState:\tZ (forged)\nState:\tS (sleeping)\nTgid:\t1\n"
        ));
        // No State: line at all.
        assert!(!status_is_zombie("Uid:\t1000\t1000\t1000\t1000\n"));
        // Empty status.
        assert!(!status_is_zombie(""));
        // Empty State value (no tab, no state char).
        assert!(!status_is_zombie("State:\n"));
    }
}
