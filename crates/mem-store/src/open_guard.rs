//! The mixed-version open guard (issue 45) — **all** platform knowledge in one
//! module: the WAL-mode classifier, the writability classifier, the probe's
//! syscalls, the decision table, the override, and the A5 mode marker.
//!
//! Why this module exists: turso's own mixed-mode oracle is unreachable from
//! this tree (issue 45 `A1` — `rsapi::open` always supplies `OpenOptions::storage`,
//! and `resolve_default_storage`, the only caller of either probe, runs only when
//! `storage` is unset). A writable open while a legacy holder is live therefore
//! succeeds and silently loses the opener's committed row with `integrity_check`
//! returning `ok` (`t1.log`). The guard is ours, and it sits at the one seam every
//! production open funnels through (`ConnectFactory::connect`).
//!
//! **Coverage (G4, verbatim):** *What remains uncovered is a holder that arrives
//! after the post-open re-probe and before our first write; the window is narrowed
//! twice and never closed, because holding the lock ourselves would make us the
//! holder the probe looks for.*
//!
//! **Issue 60 is the measured proof of that shape.** The old probe *took* the
//! exclusive byte-0 lock and released it, so for the 20–85 µs it held the byte two
//! conforming openers refused each other; the probe now queries
//! (`F_OFD_GETLK`/`F_GETLK`) and acquires nothing.
//!
//! **The measured rule that constrains any future probe:** `close()` on a database
//! file releases the process's own POSIX locks, for every fcntl variant (`t2
//! S4/S6/S7`) — it is `close()`, not the lock call.
//!
//! **The override is process-wide (issue 45b).** It has exactly two legs: the env var
//! (`OPENCODE_MEM_ALLOW_UNGUARDED_OPEN`, which wins) and the resolved config key
//! (`allowUnguardedOpen`). [`crate::db::ConnectOptions`] cannot be handed a third: its
//! `allow_unguarded_open` field is `pub(crate)` — invisible to every downstream crate
//! — with [`crate::db::ConnectOptions::for_config`] the only writer on a shipped
//! option set (the two in-crate `false` literals are test-only; AC-9's greps keep that
//! honest, not the compiler). Under one resolved config O3's precondition therefore
//! holds *with* the override too: either every open derived from that config is
//! overridden — no probe runs pre or post, so nothing in-process opens and closes a
//! file it holds — or no legacy option set can be built in-process (it is refused
//! with [`StoreError::UnsupportedOpenMode`]), so no probed file carries our lock and
//! the probe's own open+close is harmless.
//!
//! **The surviving residual needs two distinct `StoreConfig` values in one process** —
//! resolved from config files *or* built by a caller, which `StoreConfig`'s public
//! field and the public `for_config` permit. An overridden legacy open derived from
//! one value is a legitimate in-process legacy holder; the probe's own open+close
//! releases this process's lock on the file, after which an open derived from the
//! *other*, guarded config proceeds while the legacy handle is still live.
//! `test_hazard_requires_two_distinct_resolved_configs` pins that sequence. **Do not
//! reinstate a probe that opens and closes a database file this process holds a lock
//! on** unless design A §11's identity-keyed clearance registry lands first (still
//! open).
//!
//! **Platform matrix (45.10).** Linux: `F_OFD_GETLK` (queried in both directions;
//! the probe takes nothing). Other Unix: `F_GETLK` (the syscall path is exercised
//! on Linux through [`LockKind::Posix`]; the *platform selection* is untested — no
//! runner). Windows:
//! no probe exists there, because `ConnectOptions::for_config` never enables
//! multiprocess WAL on Windows and every open is therefore `WalMode::Legacy` ⇒
//! [`StoreError::UnsupportedOpenMode`] before any syscall. Non-Linux-Unix
//! *selection* and the Windows runtime are named as **untested**.
//!
//! **Marker footgun (Plan Gate residual 6).** An *overridden* legacy open writes
//! `{"mode":"legacy"}` into the storage root's [`.wal-mode`](WAL_MODE_MARKER_FILE)
//! marker. A later **guarded** writable multiprocess open **from another live
//! process** then refuses with `ModeMarkerConflict` for the lifetime of the process
//! that wrote it. Two scoping exceptions, both measured, which is why the sentence is
//! not "every later open": a later writable multiprocess open **in the same process**
//! reuses its own marker and returns `Ok`, and an **overridden** reader demotes the
//! conflict like any other marker failure. Reachable only when the override is used
//! (`OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` / `allowUnguardedOpen`) and the writing
//! process outlives the next writable open — which is why it is a footgun and not a
//! clearance input.
//!
//! **Reclaim residual (review round 2026-10-26).** The reclaim path is
//! last-writer-wins: a marker installed inside the `read_marker` → `rename` window
//! can be replaced. The marker is mode *truth*, never a clearance input — the
//! database-file probe is the enforcement.
//!
//! **Marker cost and failure mode.** A writable open that must **create or rewrite**
//! the marker, or **cannot read** it, is refused (typed
//! `OpenBlocked { Unknown { errno } }`) when this process cannot write the storage
//! root; the only escape is the override. The exception is the live-marker reuse path:
//! a root whose marker this process already wrote and can still read returns `Ok`, so
//! an unwritable root is fatal only to the opens that must touch the marker. The
//! marker write performs a blocking `sync_all` on the calling (runtime) thread, once
//! per marker write — the ≈12.8 µs figure does **not** include it.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::db::ConnectOptions;
use crate::error::{OpenDirection, StoreError};
use crate::platform::ProcessProbe;

/// The env-var leg of the O4/O5 override: when this variable is a truthy value the
/// guard is bypassed (both outcomes) and every bypass is logged.
pub const ALLOW_UNGUARDED_OPEN_ENV: &str = "OPENCODE_MEM_ALLOW_UNGUARDED_OPEN";

/// The A5 mode marker's file name, inside the storage root. Never inside `.tshm`
/// (turso-owned, version-tagged, runtime-resized), never in the database header,
/// never in the WAL.
pub const WAL_MODE_MARKER_FILE: &str = ".wal-mode";

/// The config-key leg of the override (top level, camelCase, boolean, default
/// `false`). The single source of truth for the spelling: `error.rs` imports it
/// rather than re-spelling the string.
#[must_use]
pub fn allow_unguarded_open_key() -> &'static str {
    "allowUnguardedOpen"
}

// ---------------------------------------------------------------------------
// The two classifiers (45.13: writability is part of the contract)
// ---------------------------------------------------------------------------

/// Which WAL mode an option set asks turso for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalMode {
    /// Single-process WAL — turso's default, and the only mode a Windows build of
    /// this tree can produce (`ConnectOptions::for_config` never pushes
    /// `MultiprocessWal` there).
    Legacy,
    /// `experimental_multiprocess_wal` — the shipping mode on non-Windows.
    Multiprocess,
}

impl WalMode {
    /// Classification is by the **token**, never by "some experimental feature is
    /// set": draft A con 5's rule (a future feature list that implies a mode
    /// without the token must not silently classify as legacy *or* as
    /// multiprocess).
    #[must_use]
    pub fn from_options(opts: &ConnectOptions) -> Self {
        if opts
            .experimental
            .iter()
            .any(|feature| matches!(feature, crate::db::ExperimentalFeature::MultiprocessWal))
        {
            Self::Multiprocess
        } else {
            Self::Legacy
        }
    }

    /// The marker file's spelling (`{"mode":"legacy"|"multiprocess"}`) and the
    /// refusal text's spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Multiprocess => "multiprocess",
        }
    }
}

/// Whether the open can write. Mirrors `turso::Builder::read_only` (45.13 / A3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writability {
    Writable,
    ReadOnly,
}

impl Writability {
    #[must_use]
    pub fn from_options(opts: &ConnectOptions) -> Self {
        if opts.read_only {
            Self::ReadOnly
        } else {
            Self::Writable
        }
    }
}

/// The lock flavour the probe queries with. Linux alone has
/// open-file-description locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    /// `F_OFD_GETLK` — Linux. An OFD lock carries no `l_pid`, and a foreign
    /// *POSIX* lock is *reported* as a conflict (that lock reads `Held`, `t2
    /// S7`), never replaced.
    Ofd,
    /// `F_GETLK` — every other Unix.
    Posix,
}

impl LockKind {
    /// Linux: OFD. Every other Unix: POSIX. (Non-Unix targets have no probe at
    /// all — see [`probe_database_file`].)
    #[must_use]
    pub fn platform() -> Self {
        if cfg!(target_os = "linux") {
            Self::Ofd
        } else {
            Self::Posix
        }
    }
}

/// `EAGAIN` / `EACCES` are the two contention answers a *lock attempt* can give;
/// the probe queries and never attempts (issue 60), but this arm is the shared
/// rule for any set-style caller and
/// `test_fcntl_result_classification_treats_contention_as_held` still pins it.
/// Everything else is "the probe could not ask".
#[cfg(unix)]
fn is_contention_errno(errno: i32) -> bool {
    errno == libc::EAGAIN || errno == libc::EACCES
}

/// Maps one **failed** `fcntl(…, &lock)` result (`result != 0`) onto a
/// [`ProbeOutcome`]: contention means held, anything else is unknown. The
/// `0 ⇒ Free` arm is unreachable from the probe after issue 60 (a successful
/// *query* carries its answer in `lock.l_type`, never in the result); it stays
/// for set-style callers and for the pure classifier test. Pure, so the rule is
/// testable without a filesystem race.
#[cfg(unix)]
pub(crate) fn classify_fcntl(result: i32, errno: Option<i32>) -> ProbeOutcome {
    if result == 0 {
        return ProbeOutcome::Free;
    }
    match errno {
        Some(code) if is_contention_errno(code) => ProbeOutcome::Held,
        other => ProbeOutcome::Unknown { errno: other },
    }
}

// ---------------------------------------------------------------------------
// The override (O4/O5)
// ---------------------------------------------------------------------------

/// The whole precedence rule, as a pure function of the injected env string and
/// the resolved config bool.
///
/// | env value | config | result |
/// |---|---|---|
/// | absent / empty / whitespace | `true` | `true` |
/// | absent / empty / whitespace | `false` | `false` |
/// | `1`, `true`, `yes`, `on` (any case) | either | `true` |
/// | `0`, `false`, `no`, `off` (any case) | either | `false` |
/// | anything else non-empty | either | `true` (presence semantics) |
///
/// The falsy strings are what makes "the env var wins" meaningful: they let the
/// env leg turn the override **off** against a config `true`.
#[must_use]
pub fn override_active(env: Option<&str>, configured: bool) -> bool {
    match env.map(str::trim).filter(|value| !value.is_empty()) {
        None => configured,
        // `OPENCODE_MEM_HANG_ON_SHUTDOWN` precedent (`engine.rs:501`): presence of
        // any other non-empty value means "on".
        Some(value) => !["0", "false", "no", "off"]
            .iter()
            .any(|falsy| value.eq_ignore_ascii_case(falsy)),
    }
}

// ---------------------------------------------------------------------------
// The decision core (45.13's table)
// ---------------------------------------------------------------------------

/// The outcome of the pure decision, before any I/O is attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The open may proceed.
    Clear {
        /// A recorded, deliberate divergence from the engine's own rule.
        divergence: Option<Divergence>,
    },
    /// Refuse with this direction.
    Refuse(OpenDirection),
    /// O3: a legacy option set is refused by construction.
    RefuseUnsupportedMode,
}

/// A place where this guard is deliberately *less* aggressive than a naive rule,
/// recorded as a value so it cannot rot into a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Divergence {
    /// A read-only legacy opener skips the `-tshm` probe: it can be a victim, never
    /// an aggressor (`turso_core-0.8.3/database.rs:991-993` — the guard there
    /// returns early on `flags.contains(ReadOnly)` whatever the holder's mode).
    ReadOnlySkipsLegacyProbe,
    /// This platform has no probe at all (non-Unix, i.e. Windows): the guard is
    /// **absent** there, not proven unnecessary. Recorded so the absence is a value,
    /// not a comment (ADR-0025 D-25-3).
    NoProbeOnPlatform,
}

/// `mode × writability × probe → outcome`. Pure: no filesystem, no env.
///
/// **Post-drop direction (issue 43 W6a).** `Legacy` is the production mode — the
/// multiprocess token is gone from [`crate::db::ConnectOptions::for_config`] — so
/// the writable `Legacy` cell is the one that reads the `-tshm` sensor, and
/// `Multiprocess` is refused by construction (the inverted O3: no option set this
/// tree builds carries the token). The probe still mirrors turso's direction
/// matrix: a **read-only** opener is a victim, never an aggressor
/// (`turso_core-0.8.3/database.rs:991-993`), so it skips the probe.
///
/// `probe_eligible` is whether this platform can answer the probe at all; `db.rs`
/// computes it (`#[cfg(unix)]`) and passes it in, so this function stays pure and
/// the Windows cell is a table row rather than a `cfg` fork.
#[must_use]
pub(crate) fn decide(
    mode: WalMode,
    writability: Writability,
    probe: Option<ProbeOutcome>,
    probe_eligible: bool,
) -> Decision {
    match mode {
        // O3 inverted: the first multiprocess opener to land silently would be a
        // silent mixed-WAL window, so a hand-built option set carrying the token is
        // refused whatever it would have seen.
        WalMode::Multiprocess => Decision::RefuseUnsupportedMode,
        WalMode::Legacy => match writability {
            Writability::ReadOnly => Decision::Clear {
                divergence: Some(Divergence::ReadOnlySkipsLegacyProbe),
            },
            // No probe on this platform: the guard is absent, and the absence is
            // recorded instead of being mistaken for a clean answer.
            Writability::Writable if !probe_eligible => Decision::Clear {
                divergence: Some(Divergence::NoProbeOnPlatform),
            },
            Writability::Writable => match probe {
                Some(ProbeOutcome::Free) | Some(ProbeOutcome::Missing) => {
                    Decision::Clear { divergence: None }
                }
                Some(ProbeOutcome::Held) => Decision::Refuse(OpenDirection::LegacyAuthority),
                Some(ProbeOutcome::Unknown { errno }) => {
                    Decision::Refuse(OpenDirection::Unknown { errno })
                }
                // Reachable only while the override is active (`preflight` passes
                // `None` because no probe runs); fails closed there, then demoted.
                None => Decision::Refuse(OpenDirection::Unknown { errno: None }),
            },
        },
    }
}

// ---------------------------------------------------------------------------
// The seam's two halves: preflight (before turso touches the file) and
// postflight (after it opened, before the handle is handed out)
// ---------------------------------------------------------------------------

/// The probe injection point. A plain `fn` pointer: it is not reachable from the
/// [`crate::db::ConnectFactory`] trait, so no call site can substitute an answer
/// (the unit tests inject one to make the post-open refusal deterministic).
pub(crate) type ProbeFn = fn(&Path) -> ProbeOutcome;

/// Everything `preflight` needs to decide, all injected — no hidden env or
/// filesystem reads (the env string is passed in, so the precedence stays pure).
pub(crate) struct Preflight<'a> {
    /// The storage root whose [`.wal-mode`](WAL_MODE_MARKER_FILE) marker the guard
    /// maintains. `None` for hand-built options ⇒ no marker (a caller with no root
    /// has no marker to own).
    pub storage_root: Option<&'a Path>,
    pub db_path: &'a Path,
    /// The file the probe queries: the target's `-tshm` sensor, never the database
    /// itself. Errors still name `db_path` (that is the file the user knows).
    pub probe_path: &'a Path,
    pub mode: WalMode,
    pub writability: Writability,
    /// Whether this platform can answer the probe (`#[cfg(unix)]` at the `db.rs`
    /// seam). `false` ⇒ no probe call and a recorded
    /// [`Divergence::NoProbeOnPlatform`], never an `Unknown` refusal.
    pub probe_eligible: bool,
    /// The config leg only, as copied by [`crate::db::ConnectOptions::for_config`];
    /// the env leg arrives separately in `env_override` and wins (issue 45b: nothing
    /// here is per-call).
    pub allow_unguarded_open: bool,
    pub env_override: Option<&'a str>,
    /// The liveness oracle the marker uses. Production passes
    /// [`crate::platform::SystemProcessProbe`] (which *is* `decide_liveness` +
    /// `observe_process` + `ProcObservability::current`); the marker's unit tests
    /// pass a fixed fake, so no env or `/proc` state is mutated to test it.
    pub process_probe: &'a dyn ProcessProbe,
}

/// The pass-through token a cleared open carries to `postflight`. `guarded` is
/// `false` exactly when the override is active; in that case `preflight` ran no
/// probe and `postflight` returns early, so the override skips the probe **both**
/// pre and post.
pub(crate) struct Clearance {
    pub guarded: bool,
    pub mode: WalMode,
    pub writability: Writability,
    pub probe_eligible: bool,
    pub override_active: bool,
}

/// The override's state **and** which leg decided it (for the one warn line).
fn override_state(preflight: &Preflight<'_>) -> (bool, &'static str) {
    match preflight
        .env_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => (
            override_active(Some(value), preflight.allow_unguarded_open),
            "env",
        ),
        None => (
            override_active(None, preflight.allow_unguarded_open),
            "config",
        ),
    }
}

fn direction_label(direction: &OpenDirection) -> &'static str {
    match direction {
        OpenDirection::LegacyAuthority => "legacy-authority",
        OpenDirection::Unknown { .. } => "unknown",
        OpenDirection::ModeMarkerConflict { .. } => "mode-marker-conflict",
    }
}

/// Refuses **before** any turso call, and never mutates the database file.
///
/// Order: classify → [`decide`] → if the decision refuses, apply the override (one
/// `tracing::warn!` carrying the path, the bypassed outcome and the override
/// source) → [`Clearance`].
pub(crate) fn preflight(
    preflight: &Preflight<'_>,
    probe: ProbeFn,
) -> Result<Clearance, StoreError> {
    let (override_on, override_source) = override_state(preflight);
    // No probe runs while the override is active — **neither** this one (the
    // pre-open probe) nor the post-open re-probe (`postflight` returns early on
    // `!guarded`, and `guarded == !override_on`). Blueprint Step 5 item 2: "when the
    // override is active no probe runs pre *or* post (`both outcomes`)".
    //
    // Without a probe answer `decide(.., None)` yields `Refuse(Unknown { None })`,
    // which the override rule below demotes — so the behaviour is unchanged and the
    // warn's `bypassed` label honestly reads `unknown`, because nothing was probed.
    //
    // Otherwise: only the writable-legacy cell reads a probe answer, so only that
    // cell pays for one (`45.12`: read-only opens and no-probe platforms cost 0).
    let probe_outcome = match (preflight.mode, preflight.writability) {
        (WalMode::Legacy, Writability::Writable) if !override_on && preflight.probe_eligible => {
            Some(probe(preflight.probe_path))
        }
        _ => None,
    };
    match decide(
        preflight.mode,
        preflight.writability,
        probe_outcome,
        preflight.probe_eligible,
    ) {
        Decision::Clear { divergence } => {
            if let Some(divergence) = divergence {
                // The divergence is recorded as a value (A3), and observed here so
                // it is not a field nobody reads. `Clearance` does not carry a
                // second copy: it is only the token `postflight` needs.
                tracing::debug!(
                    db_path = %preflight.db_path.display(),
                    ?divergence,
                    "open guard: recorded divergence"
                );
            }
        }
        Decision::RefuseUnsupportedMode => {
            if !override_on {
                return Err(StoreError::UnsupportedOpenMode {
                    db_path: preflight.db_path.to_path_buf(),
                    mode: preflight.mode.as_str(),
                });
            }
            tracing::warn!(
                db_path = %preflight.db_path.display(),
                bypassed = "unsupported-open-mode",
                mode = preflight.mode.as_str(),
                override_source,
                "open guard bypassed by the override"
            );
        }
        Decision::Refuse(direction) => {
            if !override_on {
                return Err(StoreError::OpenBlocked {
                    db_path: preflight.db_path.to_path_buf(),
                    direction,
                });
            }
            tracing::warn!(
                db_path = %preflight.db_path.display(),
                bypassed = direction_label(&direction),
                override_source,
                "open guard bypassed by the override"
            );
        }
    }

    // A5: the marker is attempted for **every writable** open, including an
    // overridden one. The override skips *the probe*, not the marker: the marker is
    // what makes `ModeMarkerConflict` reachable at all, and the contract says marker
    // failures are "refusals subject to the same override" — which requires the
    // failure to be produced. So: produced always, refused when the guard is active,
    // demoted to a warning when the override is on.
    if let Some(root) = preflight.storage_root
        && let Err(error) = ensure_mode_marker(
            root,
            preflight.mode,
            preflight.writability,
            preflight.process_probe,
        )
    {
        if !override_on {
            tracing::warn!(
                db_path = %preflight.db_path.display(),
                override_source,
                "open guard: the mode marker could not be recorded"
            );
            return Err(rebind_refusal(error, preflight.db_path));
        }
        tracing::warn!(
            db_path = %preflight.db_path.display(),
            override_source,
            "open guard: override active, continuing despite a mode-marker failure"
        );
    }

    Ok(Clearance {
        guarded: !override_on,
        mode: preflight.mode,
        writability: preflight.writability,
        probe_eligible: preflight.probe_eligible,
        override_active: override_on,
    })
}

/// A refusal raised while inspecting the **marker** names the marker path; the
/// caller asked about a database, so the path is rebound to it here.
fn rebind_refusal(error: StoreError, db_path: &Path) -> StoreError {
    match error {
        StoreError::OpenBlocked { direction, .. } => StoreError::OpenBlocked {
            db_path: db_path.to_path_buf(),
            direction,
        },
        other => other,
    }
}

/// The post-open re-probe (`45.5`): narrows the TOCTOU window that the pre-open
/// probe cannot close.
///
/// It re-probes only for a guarded writable legacy open on a platform that can
/// answer. A `Held` answer is a refusal and the caller drops the database before
/// returning it, so no owner is left behind.
pub(crate) fn postflight(
    clearance: &Clearance,
    db_path: &Path,
    probe_path: &Path,
    probe: ProbeFn,
) -> Result<(), StoreError> {
    if !(clearance.guarded
        && clearance.probe_eligible
        && clearance.mode == WalMode::Legacy
        && clearance.writability == Writability::Writable)
    {
        return Ok(());
    }
    match probe(probe_path) {
        ProbeOutcome::Free | ProbeOutcome::Missing => Ok(()),
        ProbeOutcome::Held => Err(StoreError::OpenBlocked {
            db_path: db_path.to_path_buf(),
            direction: OpenDirection::LegacyAuthority,
        }),
        ProbeOutcome::Unknown { errno } => {
            if clearance.override_active {
                // Defensive, unreachable: `guarded` (checked above) implies
                // `!override_active`, so no overridden clearance can reach this arm.
                // It exists only so the classifier stays total.
                tracing::warn!(
                    db_path = %db_path.display(),
                    bypassed = "unknown",
                    override_source = "override",
                    "open guard post-open probe could not answer; the override is active"
                );
                return Ok(());
            }
            Err(StoreError::OpenBlocked {
                db_path: db_path.to_path_buf(),
                direction: OpenDirection::Unknown { errno },
            })
        }
    }
}

// ---------------------------------------------------------------------------
// The A5 mode marker — a file we own, in the storage root
// ---------------------------------------------------------------------------

/// The marker's body. It reuses the shipped lock-record shape
/// ([`crate::operation_lock::LockState`] flattened + the start-time token) so no
/// new liveness format is invented; `operation` is `LockState`'s third field and
/// is always `null` here. `mode` is the only new field.
///
/// It lives in [`.wal-mode`](WAL_MODE_MARKER_FILE) in the storage root — never in
/// `.tshm` (turso-owned, version-tagged, resized at runtime), never in the
/// database header, never in the WAL.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ModeMarker {
    #[serde(flatten)]
    state: crate::operation_lock::LockState,
    #[serde(default)]
    start_time: Option<u64>,
    mode: WalMode,
}

/// `root/.wal-mode`.
#[must_use]
pub(crate) fn marker_path(root: &Path) -> PathBuf {
    root.join(WAL_MODE_MARKER_FILE)
}

/// Thread-unique suffix for the staging file (one process may open many databases
/// in one root concurrently).
static MARKER_STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many staged names one write may try before it gives up. Pids are reused and
/// `MARKER_STAGE` restarts at 0 in a new process, so a stale staged file left by a
/// crashed writer can occupy a name; each attempt advances past it.
const MARKER_STAGE_ATTEMPTS: u64 = 16;

/// `EEXIST`. 17 on every Unix and on the Windows CRT; spelled here so this function
/// needs no `cfg(unix)` and no `libc` on a target where none exists.
const ERRNO_EEXIST: i32 = 17;

/// The marker's **one** failure type: a typed refusal carrying the OS error.
///
/// A bare [`StoreError::Io`] here would be laundered by
/// `ConnectionManager::open_database` into `StoreError::Turso` ("Is a directory (os
/// error 21)"), naming neither the marker nor the override — and with encryption on
/// it would add the actively wrong "Check databaseEncryptionKey".
fn marker_refusal(path: &Path, error: Option<&std::io::Error>) -> StoreError {
    StoreError::OpenBlocked {
        db_path: path.to_path_buf(),
        direction: OpenDirection::Unknown {
            errno: error.and_then(std::io::Error::raw_os_error),
        },
    }
}

/// Writes the marker **atomically** with `O_EXCL`: the body goes to a staging file
/// in the same directory created with `create_new`, is synced, and is then
/// `rename`d over the marker.
///
/// Two properties, both measured:
///
/// * **No truncate primitive.** The first shape opened the staging file with
///   `create(true).truncate(true)`, which *follows* a symlink planted at the
///   predictable staging name: a 24-byte victim file was cut to the marker body and
///   the following `rename` installed the symlink as `.wal-mode`. `create_new` sets
///   `O_EXCL`, which never follows a symlink — a planted name answers `EEXIST`.
/// * **No empty-marker window.** `create_new` + write let a concurrent opener read
///   the file between creation and the body write, call it stale, unlink it and race
///   the recreation — measurably `EEXIST` (errno 17) and a spurious `OpenBlocked`
///   under parallel opens in one root. A `rename` is atomic, so a reader only ever
///   sees a complete marker.
///
/// Cleanup runs **only** for a staging file this call created: on `EEXIST` the file
/// belongs to someone else and must never be removed. Exhaustion is the *typed*
/// `EEXIST` refusal, never a bare io error.
fn write_marker(path: &Path, mode: WalMode) -> Result<(), StoreError> {
    let pid = std::process::id();
    let record = ModeMarker {
        state: crate::operation_lock::LockState {
            pid,
            timestamp: crate::operation_lock::now_rfc3339_millis(),
            operation: None,
        },
        start_time: crate::platform::process_start_time(pid),
        mode,
    };
    let body = serde_json::to_string(&record)
        .map_err(|error| marker_refusal(path, Some(&std::io::Error::other(error))))?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..MARKER_STAGE_ATTEMPTS {
        let staged = directory.join(format!(
            "{}.tmp.{pid}.{}",
            WAL_MODE_MARKER_FILE,
            MARKER_STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
        {
            Ok(file) => file,
            // Not ours — never remove another writer's file, just advance.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(marker_refusal(path, Some(&error))),
        };
        // From here the staging file IS ours, so every failure removes it.
        let written = file
            .write_all(body.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&staged);
            return Err(marker_refusal(path, Some(&error)));
        }
        return match std::fs::rename(&staged, path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&staged);
                Err(marker_refusal(path, Some(&error)))
            }
        };
    }
    Err(StoreError::OpenBlocked {
        db_path: path.to_path_buf(),
        direction: OpenDirection::Unknown {
            errno: Some(ERRNO_EEXIST),
        },
    })
}

/// `Ok(None)` for a missing, empty or unparsable marker — all three are "no live
/// claim", which the caller treats as reclaimable.
fn read_marker(path: &Path) -> Result<Option<ModeMarker>, StoreError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(marker_refusal(path, Some(&error))),
    };
    if contents.trim().is_empty() {
        return Ok(None);
    }
    Ok(serde_json::from_str::<ModeMarker>(&contents).ok())
}

/// Records (or validates) the A5 mode marker for a **writable** open.
///
/// Branches, all tested with an injected [`ProcessProbe`]:
///
/// 1. no marker ⇒ write it atomically (staging file + `sync_all` + `rename`, so
///    no reader can observe a half-written marker);
/// 2. `AlreadyExists` ⇒ read + parse and decide liveness with the shipped rule
///    (the probe *is* `decide_liveness` + `observe_process` +
///    `ProcObservability::current` in production):
///    * live **and ours** (same pid) ⇒ reuse, do not rewrite (the writer's token
///      stays stable);
///    * live, foreign, **same** mode ⇒ leave it alone;
///    * live, foreign, **different** mode ⇒ `ModeMarkerConflict` for a writable
///      open (the caller demotes it when the override is active);
///    * dead, zero-length or unparsable ⇒ overwrite atomically (`O_EXCL` staging
///      write); a write failure is `OpenBlocked { Unknown { errno } }` — the
///      marker's only failure type.
/// 3. read-only opens return immediately — a read-only open must not need write
///    access to the root, and it is "protected, not protective" (A3).
///
/// The marker's own liveness inherits issue 43's `hidepid=2` fail-open, so the
/// marker is mode *truth* and never a clearance input.
pub(crate) fn ensure_mode_marker(
    root: &Path,
    mode: WalMode,
    writability: Writability,
    probe: &dyn ProcessProbe,
) -> Result<(), StoreError> {
    if writability == Writability::ReadOnly {
        return Ok(());
    }
    // Best-effort (Step 7.1): a root we cannot create is not itself a marker
    // failure — `read_marker`/`write_marker` below will refuse with the typed error,
    // and the override still applies to *that*.
    if let Err(error) = std::fs::create_dir_all(root) {
        tracing::warn!(
            root = %root.display(),
            %error,
            "open guard: could not create the storage root for the mode marker"
        );
    }
    let path = marker_path(root);
    // Read first: a live marker is either reused, left alone, or refused. A
    // missing, dead, empty or unparsable marker falls through to the rewrite.
    if let Some(marker) = read_marker(&path)?
        && probe
            .liveness(marker.state.pid, marker.start_time)
            .is_alive()
    {
        if marker.state.pid == std::process::id() || marker.mode == mode {
            return Ok(());
        }
        return Err(StoreError::OpenBlocked {
            db_path: path,
            direction: OpenDirection::ModeMarkerConflict {
                recorded: marker.mode.as_str(),
            },
        });
    }
    // Absent, dead, empty or unparsable: write it atomically (a reader can never
    // observe a half-written marker, so no concurrent opener can mistake a live
    // marker for a stale one). `write_marker` already returns the typed refusal.
    write_marker(&path, mode)
}

// ---------------------------------------------------------------------------
// The probe
// ---------------------------------------------------------------------------

/// What the database-file probe found.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// No conflicting lock on byte 0: no legacy holder (nothing was taken).
    Free,
    /// A live legacy (single-process) opener holds the file.
    Held,
    /// The database file does not exist — a known answer (never created by a probe).
    Missing,
    /// The probe could not answer (a permission error, an exotic filesystem, a
    /// platform with no probe). `errno` is `None` when there is no OS error to name.
    Unknown { errno: Option<i32> },
}

/// Probes `path` with the platform's lock flavour ([`LockKind::platform`]).
#[cfg(unix)]
#[must_use]
pub fn probe_database_file(path: &Path) -> ProbeOutcome {
    probe_database_file_with(path, LockKind::platform())
}

/// Probes `path` for a live **legacy** (single-process) holder with an explicit
/// lock flavour — the injection point that lets the POSIX (`F_GETLK`) leg be
/// tested on Linux (45.10).
///
/// Shape, and why each part is what it is:
///
/// * `O_RDWR` **without** `O_CREAT`: a missing database is a *known* answer
///   ([`ProbeOutcome::Missing`]) and **no probe may create a database**.
/// * exactly one byte at offset 0, **queried** (`F_OFD_GETLK`/`F_GETLK`) as
///   `F_WRLCK`: the probe acquires nothing, cannot block, cannot alter a foreign
///   lock and cannot be seen by a peer probe.
/// * `l_pid: 0` on the request; the answer's `l_pid` is ignored — the refusal
///   never names a holder.
#[cfg(unix)]
#[must_use]
pub fn probe_database_file_with(path: &Path, kind: LockKind) -> ProbeOutcome {
    use std::os::unix::io::AsRawFd;

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProbeOutcome::Missing;
        }
        // A permission error, a directory, an exotic filesystem: the probe cannot
        // answer, and the guard refuses rather than guesses (overridable, O4).
        Err(error) => {
            return ProbeOutcome::Unknown {
                errno: error.raw_os_error(),
            };
        }
    };
    let fd = file.as_raw_fd();

    // SAFETY: `libc::flock` is plain old data (five integer fields); all-zero is a
    // valid bit pattern, and every field that carries meaning is overwritten
    // below. (Precedent: `mem-profile/src/learning_lock.rs:115`.)
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _; // the *requested* type — never F_UNLCK
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 0;
    lock.l_len = 1;
    lock.l_pid = 0;

    let command: libc::c_int = match kind {
        #[cfg(target_os = "linux")]
        LockKind::Ofd => libc::F_OFD_GETLK,
        // Non-Linux Unix has no OFD locks; `LockKind::platform()` never returns
        // `Ofd` there, and a caller that asks for it explicitly gets POSIX
        // semantics rather than a silently different answer.
        #[cfg(not(target_os = "linux"))]
        LockKind::Ofd => libc::F_GETLK,
        LockKind::Posix => libc::F_GETLK,
    };

    // SAFETY: `fd` is owned by `file` for the whole call (nothing closes it before
    // `file` drops at the end of this function). `lock` is a fully initialised
    // `libc::flock`, passed as `&mut` because the kernel writes the answer into
    // it. `F_GETLK`/`F_OFD_GETLK` never block and never acquire: they return a
    // conflicting lock (or F_UNLCK), or fail with an errno.
    let result = unsafe { libc::fcntl(fd, command, &mut lock) };
    if result != 0 {
        // A *failed* call only: a successful query is 0 and its answer is in
        // `lock.l_type`, so it must never be routed through `classify_fcntl`
        // (0 there means "took it").
        return classify_fcntl(result, std::io::Error::last_os_error().raw_os_error());
    }
    if lock.l_type == libc::F_UNLCK as libc::c_short {
        ProbeOutcome::Free
    } else {
        ProbeOutcome::Held
    }
    // No unlock: nothing was taken. Dropping `file` still closes the description,
    // which is the probe's only side effect.
}

/// No probe exists on non-Unix targets. Unreachable from production: every open
/// there is `WalMode::Legacy` ⇒ [`StoreError::UnsupportedOpenMode`] before a probe
/// would run.
#[cfg(not(unix))]
#[must_use]
pub fn probe_database_file(path: &Path) -> ProbeOutcome {
    let _ = path;
    ProbeOutcome::Unknown { errno: None }
}

/// No probe exists on non-Unix targets (see [`probe_database_file`]).
#[cfg(not(unix))]
#[must_use]
pub fn probe_database_file_with(path: &Path, kind: LockKind) -> ProbeOutcome {
    let _ = (path, kind);
    ProbeOutcome::Unknown { errno: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ConnectOptions, ExperimentalFeature};

    fn options(experimental: &[ExperimentalFeature], read_only: bool) -> ConnectOptions {
        ConnectOptions {
            experimental: experimental.to_vec(),
            encryption: None,
            read_only,
            allow_unguarded_open: false,
            storage_root: None,
        }
    }

    /// Draft A con 5's required test: a future feature list that *implies* a mode
    /// without the token must not silently classify as legacy — and today's
    /// encryption-only list must.
    ///
    /// Post-drop (issue 43 W6a) the shipping list is `[Encryption]` on every
    /// platform, so it must classify `Legacy`; the token still classifies
    /// `Multiprocess` when a caller hand-builds it (the refused-by-construction
    /// cell).
    ///
    /// Inverted world: if `from_options` keyed on "any experimental feature", the
    /// `[Encryption]` cell would read `Multiprocess` and this test fails.
    #[test]
    fn test_wal_mode_classifier_reads_the_multiprocess_feature() {
        assert_eq!(
            WalMode::from_options(&ConnectOptions::default()),
            WalMode::Legacy,
            "an empty feature list is the legacy (single-process) option set"
        );
        assert_eq!(
            WalMode::from_options(&options(&[ExperimentalFeature::Encryption], false)),
            WalMode::Legacy,
            "encryption alone does not enable multiprocess WAL"
        );
        assert_eq!(
            WalMode::from_options(&options(
                &[
                    ExperimentalFeature::Encryption,
                    ExperimentalFeature::MultiprocessWal
                ],
                false
            )),
            WalMode::Multiprocess,
            "a hand-built list carrying the token still classifies as multiprocess (the cell \
             `decide` refuses)"
        );
        // The shipping option set: the token is gone, so it is the legacy mode.
        let config = crate::config::StoreConfig {
            storage_path: PathBuf::from("/tmp/mem-store-classifier-unused"),
            embedding_dimensions: 2,
            embedding_model: "test-model".to_string(),
            max_vectors_per_shard: 1000,
            container_tag_prefix: "opencode".to_string(),
            database_encryption_enabled: false,
            database_encryption_cipher: "aes256gcm".to_string(),
            database_encryption_key: None,
            allow_unguarded_open: false,
        };
        for platform in [
            crate::platform::Platform::Unix,
            crate::platform::Platform::Windows,
        ] {
            let shipping = ConnectOptions::for_config(&config, platform);
            assert!(
                !shipping
                    .experimental
                    .contains(&ExperimentalFeature::MultiprocessWal),
                "post-drop `for_config` must not carry the multiprocess token on {platform:?}"
            );
            assert_eq!(
                WalMode::from_options(&shipping),
                WalMode::Legacy,
                "the shipping option set is the production (legacy) mode on {platform:?}"
            );
        }
        assert_eq!(WalMode::Legacy.as_str(), "legacy");
        assert_eq!(WalMode::Multiprocess.as_str(), "multiprocess");
    }

    /// 45.13 / issue 43 W6a: the **inverted** `mode × writability × probe` product,
    /// one assertion per cell. No filesystem race, no child process — the table is
    /// data.
    ///
    /// Post-drop the production mode is `Legacy` (the token is gone from
    /// `for_config`), so `Legacy` carries the probe and `Multiprocess` is refused by
    /// construction.
    ///
    /// Inverted world: any single arm swapped (e.g. `Held` ⇒ `Clear`) turns exactly
    /// that cell red while the rest stay green.
    #[test]
    fn test_decision_table_covers_mode_and_writability() {
        let probes = [
            None,
            Some(ProbeOutcome::Free),
            Some(ProbeOutcome::Missing),
            Some(ProbeOutcome::Held),
            Some(ProbeOutcome::Unknown { errno: Some(11) }),
            Some(ProbeOutcome::Unknown { errno: None }),
        ];
        // The new O3: no option set this tree builds carries the token, so a
        // multiprocess option set is refused whatever it would have seen.
        for writability in [Writability::Writable, Writability::ReadOnly] {
            for probe in probes {
                assert_eq!(
                    decide(WalMode::Multiprocess, writability, probe, true),
                    Decision::RefuseUnsupportedMode,
                    "post-drop O3: multiprocess is refused by construction \
                     (writability {writability:?}, probe {probe:?})"
                );
            }
        }
        // Read-only skips the probe whatever it would say.
        for probe in probes {
            assert_eq!(
                decide(WalMode::Legacy, Writability::ReadOnly, probe, true),
                Decision::Clear {
                    divergence: Some(Divergence::ReadOnlySkipsLegacyProbe)
                },
                "a read-only legacy opener never refuses a `-tshm` holder (probe {probe:?})"
            );
        }
        // Writable legacy: the probe decides.
        for probe in [Some(ProbeOutcome::Free), Some(ProbeOutcome::Missing)] {
            assert_eq!(
                decide(WalMode::Legacy, Writability::Writable, probe, true),
                Decision::Clear { divergence: None },
                "a writable legacy open clears on a known-free answer (probe {probe:?})"
            );
        }
        assert_eq!(
            decide(
                WalMode::Legacy,
                Writability::Writable,
                Some(ProbeOutcome::Held),
                true
            ),
            Decision::Refuse(OpenDirection::LegacyAuthority),
            "a live pre-drop (flag-on) holder refuses a writable legacy open"
        );
        for errno in [Some(11), None] {
            assert_eq!(
                decide(
                    WalMode::Legacy,
                    Writability::Writable,
                    Some(ProbeOutcome::Unknown { errno }),
                    true
                ),
                Decision::Refuse(OpenDirection::Unknown { errno }),
                "an unanswerable probe refuses rather than guesses"
            );
        }
        assert_eq!(
            decide(WalMode::Legacy, Writability::Writable, None, true),
            Decision::Refuse(OpenDirection::Unknown { errno: None }),
            "the override path's `None` fails closed (then the override demotes it)"
        );
        // No probe on this platform: the writable cell records the absence, and no
        // probe answer (even one handed in) can turn it into a refusal.
        for probe in probes {
            assert_eq!(
                decide(WalMode::Legacy, Writability::Writable, probe, false),
                Decision::Clear {
                    divergence: Some(Divergence::NoProbeOnPlatform)
                },
                "a platform without a probe records `NoProbeOnPlatform`, never `Unknown` \
                 (probe {probe:?})"
            );
        }
    }

    /// 45.13's divergence is a value, not a comment: the read-only row records
    /// *why* it clears, citing `turso_core-0.8.3/database.rs:990-993`.
    ///
    /// Re-derived for the inverted direction: that function returns `Ok(())` when
    /// `!enable_multiprocess_wal || flags.contains(ReadOnly)`, i.e. a read-only
    /// opener never rejects a holder, whichever mode the holder used — so a
    /// read-only legacy opener is a victim, never an aggressor.
    #[test]
    fn test_decision_table_read_only_skips_the_legacy_probe_and_records_the_divergence() {
        assert_eq!(
            decide(
                WalMode::Legacy,
                Writability::ReadOnly,
                Some(ProbeOutcome::Held),
                true
            ),
            Decision::Clear {
                divergence: Some(Divergence::ReadOnlySkipsLegacyProbe)
            },
            "0.8.3 :990-993 — a read-only opener is a victim, never an aggressor; the divergence \
             must be recorded"
        );
        assert_eq!(
            decide(
                WalMode::Legacy,
                Writability::Writable,
                Some(ProbeOutcome::Held),
                true
            ),
            Decision::Refuse(OpenDirection::LegacyAuthority),
            "the same probe answer refuses a writable open — the asymmetry is the point"
        );
    }

    /// O5: the env leg wins in both directions. Presence semantics for an
    /// unrecognised non-empty value (`OPENCODE_MEM_HANG_ON_SHUTDOWN` precedent).
    ///
    /// Inverted world: if the env leg only ever turned the override *on*, the
    /// `("0", true)` cell would answer `true` and this test fails.
    #[test]
    fn test_override_env_wins_over_the_config_value() {
        for env in [None, Some(""), Some("   ")] {
            assert!(
                !override_active(env, false),
                "config false, env absent: {env:?}"
            );
            assert!(
                override_active(env, true),
                "config true, env absent: {env:?}"
            );
        }
        for truthy in ["1", "true", "YES", "On", "anything-else"] {
            assert!(
                override_active(Some(truthy), false),
                "a truthy env value wins over config false: {truthy}"
            );
            assert!(
                override_active(Some(truthy), true),
                "a truthy env value keeps the override on: {truthy}"
            );
        }
        for falsy in ["0", "false", "FALSE", "No", " off "] {
            assert!(
                !override_active(Some(falsy), true),
                "a falsy env value turns the override OFF against config true: {falsy}"
            );
            assert!(
                !override_active(Some(falsy), false),
                "a falsy env value keeps it off: {falsy}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // A5 — the mode marker's four branches (fake process probe, no env mutation)
    // -----------------------------------------------------------------------

    use crate::platform::{Liveness, ProcessProbe};

    /// A process probe whose answer is fixed at construction: `alive` drives both
    /// `is_alive` and the token-aware `liveness`, so the marker's liveness branch
    /// is exercised without depending on this host's `/proc`.
    struct FixedProbe {
        alive: bool,
    }

    impl ProcessProbe for FixedProbe {
        fn is_alive(&self, _pid: u32) -> bool {
            self.alive
        }

        fn liveness(&self, _pid: u32, _recorded_start: Option<u64>) -> Liveness {
            if self.alive {
                Liveness::Alive
            } else {
                Liveness::Dead
            }
        }
    }

    fn marker_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mem-store-marker-{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create the marker temp dir");
        dir
    }

    fn write_foreign_marker(root: &Path, mode: WalMode) -> std::path::PathBuf {
        let path = root.join(WAL_MODE_MARKER_FILE);
        let body = serde_json::json!({
            "pid": std::process::id().wrapping_add(1),
            "timestamp": "2026-10-25T00:00:00.000Z",
            "operation": mem_store_json_null(),
            "start_time": 1,
            "mode": mode.as_str(),
        });
        std::fs::write(&path, body.to_string()).expect("write the foreign marker");
        path
    }

    fn mem_store_json_null() -> crate::serde_json::Value {
        crate::serde_json::Value::Null
    }

    /// The staging-name counter is process-global, so a test that predicts a
    /// staged name must own it. Every marker test takes this lock; poisoning is
    /// irrelevant here (the guarded value is `()`), so recover rather than fail
    /// (the `mem-episodic::gate` precedent).
    static MARKER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn marker_test_guard() -> std::sync::MutexGuard<'static, ()> {
        MARKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A writable `ProbeFn` answer for the preflight-level tests: the file is
    /// always free, so only the marker path is under test.
    fn always_free(_path: &Path) -> ProbeOutcome {
        ProbeOutcome::Free
    }

    /// Counts how often a probe actually ran — the instrument for F1 (round 2).
    static OVERRIDE_PROBE_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    /// A probe that reports a live legacy holder **and** records that it ran.
    fn counting_probe(_path: &Path) -> ProbeOutcome {
        OVERRIDE_PROBE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ProbeOutcome::Held
    }

    /// `EISDIR` / `ENOTDIR` on Linux (and every Unix): 21 / 20. Named here so the
    /// test module compiles on non-Unix targets too.
    const EISDIR: i32 = 21;

    /// This process already wrote the marker ⇒ reuse it, do not refuse.
    ///
    /// Inverted world: treating "a marker exists" as a conflict would refuse the
    /// second writable open in a root — i.e. every engine + `purge-session` pair.
    #[test]
    fn test_mode_marker_is_reused_when_this_process_already_wrote_it() {
        let _guard = marker_test_guard();
        let root = marker_dir("reuse");
        let probe = FixedProbe { alive: true };
        ensure_mode_marker(&root, WalMode::Multiprocess, Writability::Writable, &probe)
            .expect("first write");
        let first = std::fs::read(root.join(WAL_MODE_MARKER_FILE)).expect("read the marker");
        ensure_mode_marker(&root, WalMode::Multiprocess, Writability::Writable, &probe)
            .expect("reuse, not conflict");
        let second = std::fs::read(root.join(WAL_MODE_MARKER_FILE)).expect("read the marker");
        assert_eq!(
            first, second,
            "reusing our own marker must not rewrite it (the writer's token stays stable)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A marker left by a **dead** process is reclaimed, not obeyed.
    #[test]
    fn test_mode_marker_left_behind_by_a_dead_process_is_reclaimed() {
        let _guard = marker_test_guard();
        let root = marker_dir("reclaim");
        let path = write_foreign_marker(&root, WalMode::Legacy);
        let dead = FixedProbe { alive: false };
        ensure_mode_marker(&root, WalMode::Multiprocess, Writability::Writable, &dead)
            .expect("a dead writer's marker is stale and must be replaced");
        let body = std::fs::read_to_string(&path).expect("read the reclaimed marker");
        assert!(
            body.contains("\"multiprocess\""),
            "the reclaimed marker records the live mode: {body}"
        );
        assert!(
            body.contains(&std::process::id().to_string()),
            "the reclaimed marker names this process: {body}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A **live foreign** writer with a different mode ⇒ `ModeMarkerConflict` for a
    /// writable open.
    #[test]
    fn test_mode_marker_conflict_between_live_modes_is_refused_for_a_writable_open() {
        let _guard = marker_test_guard();
        let root = marker_dir("conflict");
        write_foreign_marker(&root, WalMode::Legacy);
        let live = FixedProbe { alive: true };
        let error = ensure_mode_marker(&root, WalMode::Multiprocess, Writability::Writable, &live)
            .expect_err("a live foreign writer with a different mode must refuse a writable open");
        match error {
            StoreError::OpenBlocked {
                direction: OpenDirection::ModeMarkerConflict { recorded },
                ..
            } => assert_eq!(recorded, "legacy"),
            other => panic!("the conflict must be the typed direction, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A read-only open skips the marker entirely: it must not need write access
    /// to the root, and it is "protected, not protective" (A3).
    #[test]
    fn test_mode_marker_is_not_written_for_a_read_only_open() {
        let _guard = marker_test_guard();
        let root = marker_dir("read-only");
        let live = FixedProbe { alive: true };
        ensure_mode_marker(&root, WalMode::Multiprocess, Writability::ReadOnly, &live)
            .expect("a read-only open never fails on the marker");
        assert!(
            !root.join(WAL_MODE_MARKER_FILE).exists(),
            "a read-only open must not write the marker"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The `fcntl` result rule, without syscalls: contention means held, an
    /// exotic errno means unknown.
    #[cfg(unix)]
    #[test]
    fn test_fcntl_result_classification_treats_contention_as_held() {
        assert_eq!(classify_fcntl(0, None), ProbeOutcome::Free);
        assert_eq!(
            classify_fcntl(-1, Some(libc::EAGAIN)),
            ProbeOutcome::Held,
            "EAGAIN is the OFD/POSIX contention answer"
        );
        assert_eq!(
            classify_fcntl(-1, Some(libc::EACCES)),
            ProbeOutcome::Held,
            "EACCES is the portable POSIX contention answer"
        );
        assert_eq!(
            classify_fcntl(-1, Some(libc::EBADF)),
            ProbeOutcome::Unknown {
                errno: Some(libc::EBADF)
            },
            "anything else is an unanswerable probe, not a holder"
        );
        assert_eq!(
            classify_fcntl(-1, None),
            ProbeOutcome::Unknown { errno: None }
        );
    }

    /// RC1 (security): a symlink planted at a predictable staging name must be
    /// refused, never truncated.
    ///
    /// The shipped shape opened the staging file with `create(true).truncate(true)`,
    /// which **follows** a symlink: the victim's bytes were cut to the marker body
    /// (`shipped truncate shape: victim bytes = b'X'`), and the following `rename`
    /// installed the symlink as `.wal-mode`. `O_EXCL` (`create_new`) never follows,
    /// so every planted name answers `EEXIST` and the write exhausts into a **typed**
    /// refusal.
    ///
    /// Inverted world: restore the truncate shape and this test fails on the victim
    /// byte assertion (measured before the fix: victim `b"ORIGINAL"` → the marker
    /// body) as well as on the error type.
    #[cfg(unix)]
    #[test]
    fn test_mode_marker_staging_never_truncates_a_symlink_target() {
        let _guard = marker_test_guard();
        let root = marker_dir("symlink-staging");
        let victim = root.join("victim.txt");
        std::fs::write(&victim, b"ORIGINAL").expect("write the victim file");

        // Plant a symlink at every staging name this process could possibly try.
        // The counter is process-global, so read its next value under the lock and
        // cover far more names than any implementation may attempt.
        let next = MARKER_STAGE.load(std::sync::atomic::Ordering::SeqCst);
        // Wide enough that no concurrent test in this binary (the `db.rs` guard
        // tests also advance the process-global counter) can move past the window
        // between this read and the retries. 4096 symlinks are cheap.
        let planted = 4096;
        for offset in 0..planted {
            let name = format!(
                "{}.tmp.{}.{}",
                WAL_MODE_MARKER_FILE,
                std::process::id(),
                next + offset
            );
            std::os::unix::fs::symlink(&victim, root.join(name)).expect("plant the symlink");
        }

        let live = FixedProbe { alive: true };
        let outcome =
            ensure_mode_marker(&root, WalMode::Multiprocess, Writability::Writable, &live);
        // Checked BEFORE the error shape: the truncate primitive is the security
        // defect, and a refusal with a truncated victim would still be a defect.
        assert_eq!(
            std::fs::read(&victim).expect("read the victim"),
            b"ORIGINAL",
            "RC1: the staging write must never follow a planted symlink and truncate its target"
        );
        let error =
            outcome.expect_err("every staging name is a planted symlink ⇒ the write must refuse");
        match error {
            StoreError::OpenBlocked {
                direction: OpenDirection::Unknown { errno },
                ..
            } => assert_eq!(
                errno,
                Some(libc::EEXIST),
                "exhaustion must be the typed EEXIST refusal, not a bare io error"
            ),
            other => panic!("the refusal must be typed `OpenBlocked`, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `Preflight` request for the marker-level tests.
    fn marker_request<'a>(
        root: &'a Path,
        db_path: &'a Path,
        mode: WalMode,
        allow_unguarded_open: bool,
        process_probe: &'a dyn ProcessProbe,
    ) -> Preflight<'a> {
        Preflight {
            storage_root: Some(root),
            db_path,
            probe_path: db_path,
            mode,
            writability: Writability::Writable,
            probe_eligible: true,
            allow_unguarded_open,
            env_override: None,
            process_probe,
        }
    }

    /// RC2 (contract): the override skips **the probe**, not the marker. An
    /// overridden legacy open still records `{"mode":"legacy"}`, which is what makes
    /// `ModeMarkerConflict` reachable in production at all (the documented footgun).
    ///
    /// Inverted world: the shipped `!override_on` gate on the marker leaves the file
    /// unwritten and this test fails on `missing = true`.
    #[test]
    fn test_override_still_records_the_legacy_marker() {
        let _guard = marker_test_guard();
        let root = marker_dir("override-legacy-marker");
        let db_path = root.join("legacy.db");
        let live = FixedProbe { alive: true };
        let request = marker_request(&root, &db_path, WalMode::Legacy, true, &live);

        let clearance = preflight(&request, always_free)
            .expect("the override clears the legacy refusal, and the marker is not the probe");
        assert!(
            !clearance.guarded,
            "the override still disables the probe (only the probe)"
        );
        let marker = root.join(WAL_MODE_MARKER_FILE);
        let body = std::fs::read_to_string(&marker).unwrap_or_default();
        assert!(
            body.contains("\"legacy\""),
            "RC2: an overridden legacy open must still record the mode marker (missing = {})",
            body.is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A3 (round 5): the footgun paragraph calls *both* scoping exceptions
    /// "measured", so the second one gets its own proof — a **foreign live legacy
    /// marker** is a `ModeMarkerConflict`, and under the override `preflight` demotes
    /// it to a warning (`Ok`). The same marker with the guard active is the typed
    /// conflict, so the pair is non-vacuous.
    ///
    /// Inverted world: without the RC2 demotion this arm would return
    /// `Err(OpenBlocked { ModeMarkerConflict })`, exactly like the guarded control.
    #[test]
    fn test_override_demotes_a_mode_marker_conflict() {
        let _guard = marker_test_guard();
        let root = marker_dir("override-marker-conflict");
        let db_path = root.join("db.db");
        // Post-drop this opener is `Legacy`, so the foreign (still-live) holder is
        // the pre-drop `multiprocess` one — the mixed-version window the marker names.
        write_foreign_marker(&root, WalMode::Multiprocess);
        let live = FixedProbe { alive: true };

        let overridden = preflight(
            &marker_request(&root, &db_path, WalMode::Legacy, true, &live),
            always_free,
        );
        assert!(
            overridden.is_ok(),
            "the override demotes a live foreign marker's mode conflict: {:?}",
            overridden.err()
        );

        let guarded = preflight(
            &marker_request(&root, &db_path, WalMode::Legacy, false, &live),
            always_free,
        );
        match guarded {
            Err(StoreError::OpenBlocked {
                direction: OpenDirection::ModeMarkerConflict { recorded },
                ..
            }) => assert_eq!(recorded, "multiprocess"),
            other => panic!(
                "the guarded control must refuse with the typed conflict, got {:?}",
                other.err()
            ),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// RC2: under the override a marker failure is **demoted to a warning**; with
    /// the override off it is still a refusal. (The same Mixed configuration, only
    /// the override bit differs — the inversion pair for the marker.)
    #[test]
    fn test_override_demotes_a_marker_failure_instead_of_refusing() {
        let _guard = marker_test_guard();
        let base = marker_dir("override-demotes");
        let root = base.join("not-a-directory");
        std::fs::write(&root, b"x").expect("a regular file where the root should be");
        let db_path = root.join("db.db");
        let live = FixedProbe { alive: true };

        let overridden = preflight(
            &marker_request(&root, &db_path, WalMode::Legacy, true, &live),
            always_free,
        );
        assert!(
            overridden.is_ok(),
            "RC2: with the override on, a marker failure is a warning, not a refusal: {:?}",
            overridden.err()
        );

        let guarded = preflight(
            &marker_request(&root, &db_path, WalMode::Legacy, false, &live),
            always_free,
        );
        assert!(
            matches!(guarded, Err(StoreError::OpenBlocked { .. })),
            "with the override off the same failure must refuse, got {:?}",
            guarded.err()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// RC3 (diagnostics): a marker failure is **one** typed refusal, never a bare
    /// `StoreError::Io` that the connection manager would launder into
    /// `StoreError::Turso` ("Is a directory (os error 21)", naming neither the
    /// marker nor the override).
    ///
    /// Inverted world: the shipped `StoreError::from(io)` path yields `Io { .. }`
    /// and this test fails on the variant.
    #[test]
    fn test_mode_marker_failure_is_a_typed_open_blocked_refusal() {
        let _guard = marker_test_guard();
        let root = marker_dir("typed-marker-failure");
        // A directory where the marker file belongs: `read_to_string` → EISDIR.
        std::fs::create_dir_all(root.join(WAL_MODE_MARKER_FILE)).expect("create the decoy dir");
        let live = FixedProbe { alive: true };
        let error = ensure_mode_marker(&root, WalMode::Multiprocess, Writability::Writable, &live)
            .expect_err("an unreadable marker must refuse the writable open");
        match error {
            StoreError::OpenBlocked {
                direction: OpenDirection::Unknown { errno },
                ..
            } => assert_eq!(
                errno,
                Some(EISDIR),
                "RC3: the marker failure keeps its OS error inside the typed refusal"
            ),
            other => {
                panic!("RC3: a marker failure must be `OpenBlocked`, never a bare `Io`: {other:?}")
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// F1 (round 2): blueprint Step 5 item 2 — *when the override is active no probe
    /// runs pre **or** post*. The override disables the guard, so a probe under it is
    /// pure cost, and the refusal it would produce is demoted anyway.
    ///
    /// The instrument is the counter, not a comment: a `fn`-pointer probe that bumps
    /// it. Inverted world (what round 2 measured):
    /// `REVIEWER-INSTRUMENT PROBE-CALLED override_on=true` — the pre-probe was
    /// selected before the override was consulted, so the override-on half read 1.
    #[test]
    fn test_override_skips_the_pre_probe_entirely() {
        let _guard = marker_test_guard();
        let root = marker_dir("override-no-probe");
        let db_path = root.join("db.db");
        let live = FixedProbe { alive: true };

        OVERRIDE_PROBE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let overridden = preflight(
            &marker_request(&root, &db_path, WalMode::Legacy, true, &live),
            counting_probe,
        );
        assert!(
            overridden.is_ok(),
            "the override still clears (the absent probe answer refuses, then is demoted): {:?}",
            overridden.err()
        );
        assert_eq!(
            OVERRIDE_PROBE_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no probe may run while the override is active"
        );

        OVERRIDE_PROBE_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let guarded = preflight(
            &marker_request(&root, &db_path, WalMode::Legacy, false, &live),
            counting_probe,
        );
        assert_eq!(
            OVERRIDE_PROBE_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a guarded open must probe exactly once"
        );
        assert!(
            matches!(
                guarded,
                Err(StoreError::OpenBlocked {
                    direction: OpenDirection::LegacyAuthority,
                    ..
                })
            ),
            "the guarded open refuses on the probe's `Held`: {:?}",
            guarded.err()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
