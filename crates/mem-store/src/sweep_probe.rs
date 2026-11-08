//! Issue 43 W6c — the sweep's preflight cross-check.
//!
//! The sweep is the second process that can open the three auxiliary databases
//! while an engine may still be live. It must refuse when a **pre-drop** holder
//! (a binary that still enables multiprocess WAL) could be running, and it must
//! fail closed when it cannot tell.
//!
//! Two inputs decide, in this order (R3):
//!
//! 1. A **live** `.wal-mode` marker recording `mode: "multiprocess"` refuses —
//!    regardless of the `-tshm` probe. A live multiprocess marker means a
//!    post-issue-45 pre-drop binary wrote it and its process may still run; a
//!    missing `-tshm` under it is precisely the unlink/restore case, so trusting
//!    the sensor alone would be a fail-open.
//! 2. Otherwise the `-tshm` sensor decides: `Held` refuses, `Unknown` refuses
//!    (fail closed), `Free`/`Missing` clears.
//!
//! Mode *truth* is not clearance: an absent, dead, empty or unparsable marker
//! carries no live claim (`ModeMarkerView::live_mode == None`), so the probe
//! still decides — the same direction [`crate::open_guard::ensure_mode_marker`]
//! takes.

use std::path::Path;

use crate::open_guard::{ProbeFn, ProbeOutcome, WalMode};

/// What the caller read from the root's `.wal-mode` marker, reduced to the one
/// fact the decision needs: the mode a **live** marker claims.
///
/// `live_mode == None` covers absent, dead, empty and unparsable markers alike —
/// all four are "no live claim", and the probe (not the marker) decides clearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModeMarkerView {
    pub live_mode: Option<WalMode>,
}

/// Why the sweep must not proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepBlocked {
    /// A live pre-drop holder could be running (a live multiprocess marker, or a
    /// held `-tshm` sensor).
    LegacyPeerLive,
    /// The sensor could not answer; fail closed rather than sweep under an
    /// unknown holder.
    ProbeUnknown { errno: Option<i32> },
}

/// The pass token a clear preflight returns. Unit type: clearance carries no
/// data, so it cannot be forged by reading a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SweepClear;

/// The public block reason the `mem-bridge` sweep runner maps to exit 75.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepPreflightBlock {
    /// A live pre-drop holder could be running.
    LegacyPeerLive,
    /// The sensor could not answer; fail closed.
    ProbeUnknown { errno: Option<i32> },
}

/// The I/O edge the sweep runner calls: reads the root's live marker and probes
/// `db_path`'s `-tshm` sensor, then delegates to [`sweep_preflight`].
///
/// One call per sweep target (the marker read is idempotent); a caller that
/// wants "any target blocked" calls it for each existing database.
pub fn sweep_preflight_database(root: &Path, db_path: &Path) -> Result<(), SweepPreflightBlock> {
    let marker = ModeMarkerView {
        live_mode: crate::open_guard::live_marker_mode(root, &crate::platform::SystemProcessProbe),
    };
    let tshm_path = crate::sqlite_files::sidecar_path(db_path, "-tshm");
    match sweep_preflight(
        root,
        marker,
        crate::open_guard::probe_database_file,
        &tshm_path,
    ) {
        Ok(SweepClear) => Ok(()),
        Err(SweepBlocked::LegacyPeerLive) => Err(SweepPreflightBlock::LegacyPeerLive),
        Err(SweepBlocked::ProbeUnknown { errno }) => {
            Err(SweepPreflightBlock::ProbeUnknown { errno })
        }
    }
}
/// The pure decision core: no filesystem read, no env, no liveness query — the
/// caller injects both the marker verdict and the sensor answer.
///
/// `root` is the storage root the caller is about to sweep; `tshm_path` must be a
/// sensor path inside it (a debug assertion pins that contract, so a caller cannot
/// point the sweep at one root while probing another).
pub(crate) fn sweep_preflight(
    root: &Path,
    marker: ModeMarkerView,
    probe: ProbeFn,
    tshm_path: &Path,
) -> Result<SweepClear, SweepBlocked> {
    debug_assert_eq!(
        tshm_path.parent(),
        Some(root),
        "the -tshm sensor must live in the storage root being swept"
    );
    // R3: a live multiprocess marker refuses regardless of the probe outcome.
    if marker.live_mode == Some(WalMode::Multiprocess) {
        return Err(SweepBlocked::LegacyPeerLive);
    }
    match probe(tshm_path) {
        ProbeOutcome::Held => Err(SweepBlocked::LegacyPeerLive),
        ProbeOutcome::Unknown { errno } => Err(SweepBlocked::ProbeUnknown { errno }),
        ProbeOutcome::Free | ProbeOutcome::Missing => Ok(SweepClear),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(_path: &Path) -> ProbeOutcome {
        ProbeOutcome::Held
    }
    fn missing(_path: &Path) -> ProbeOutcome {
        ProbeOutcome::Missing
    }
    fn free(_path: &Path) -> ProbeOutcome {
        ProbeOutcome::Free
    }
    fn unknown(_path: &Path) -> ProbeOutcome {
        ProbeOutcome::Unknown { errno: Some(13) }
    }

    fn paths() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join("sweep-probe-unit");
        let tshm = root.join("user-prompts.db-tshm");
        (root, tshm)
    }

    /// A live multiprocess marker refuses **even when `-tshm` is missing** — the
    /// unlink/restore fail-open this rule closes.
    #[test]
    fn test_live_multiprocess_marker_refuses_with_a_missing_tshm() {
        let (root, tshm) = paths();
        let marker = ModeMarkerView {
            live_mode: Some(WalMode::Multiprocess),
        };
        assert_eq!(
            sweep_preflight(&root, marker, missing, &tshm),
            Err(SweepBlocked::LegacyPeerLive)
        );
    }

    /// A dead marker is no live claim: the missing sensor clears.
    #[test]
    fn test_dead_marker_clears_with_a_missing_tshm() {
        let (root, tshm) = paths();
        let marker = ModeMarkerView { live_mode: None };
        assert_eq!(
            sweep_preflight(&root, marker, missing, &tshm),
            Ok(SweepClear)
        );
    }

    /// A live **legacy** marker is not a pre-drop holder: the probe decides.
    #[test]
    fn test_live_legacy_marker_clears_with_a_missing_tshm() {
        let (root, tshm) = paths();
        let marker = ModeMarkerView {
            live_mode: Some(WalMode::Legacy),
        };
        assert_eq!(
            sweep_preflight(&root, marker, missing, &tshm),
            Ok(SweepClear)
        );
    }

    #[test]
    fn test_held_sensor_refuses() {
        let (root, tshm) = paths();
        let marker = ModeMarkerView { live_mode: None };
        assert_eq!(
            sweep_preflight(&root, marker, held, &tshm),
            Err(SweepBlocked::LegacyPeerLive)
        );
    }

    #[test]
    fn test_unknown_sensor_refuses_fail_closed() {
        let (root, tshm) = paths();
        let marker = ModeMarkerView { live_mode: None };
        assert_eq!(
            sweep_preflight(&root, marker, unknown, &tshm),
            Err(SweepBlocked::ProbeUnknown { errno: Some(13) })
        );
    }

    /// An unparsable marker is "no live claim", and the probe still decides — both
    /// arms, so neither direction can be vacuous.
    #[test]
    fn test_unparsable_marker_falls_through_to_the_probe() {
        let (root, tshm) = paths();
        let marker = ModeMarkerView { live_mode: None };
        assert_eq!(
            sweep_preflight(&root, marker, held, &tshm),
            Err(SweepBlocked::LegacyPeerLive),
            "a garbled marker must not clear a held sensor"
        );
        assert_eq!(
            sweep_preflight(&root, marker, free, &tshm),
            Ok(SweepClear),
            "a garbled marker with a free sensor clears (mode truth is not clearance)"
        );
    }

    /// A fake liveness oracle: `true` answers `Alive` for every pid.
    struct FixedLiveness(bool);

    impl crate::platform::ProcessProbe for FixedLiveness {
        fn is_alive(&self, _pid: u32) -> bool {
            self.0
        }
    }

    fn marker_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create marker dir");
        dir
    }

    /// The real marker read: a live multiprocess body views as `Multiprocess`, a
    /// dead one as `None`.
    #[test]
    fn test_live_marker_view_reads_a_live_multiprocess_marker() {
        let root = marker_dir("sweep-marker-live");
        let body = format!(
            r#"{{"pid":{},"timestamp":"2026-11-09T00:00:00.000Z","operation":null,"start_time":null,"mode":"multiprocess"}}"#,
            std::process::id()
        );
        std::fs::write(
            root.join(crate::open_guard::WAL_MODE_MARKER_FILE),
            body.as_bytes(),
        )
        .expect("write the marker");

        assert_eq!(
            crate::open_guard::live_marker_mode(&root, &FixedLiveness(true)),
            Some(WalMode::Multiprocess)
        );
        assert_eq!(
            crate::open_guard::live_marker_mode(&root, &FixedLiveness(false)),
            None,
            "a dead marker is no live claim"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A garbled marker body reads as `None` (the same direction as absent/dead),
    /// so the sensor decides rather than the garbage.
    #[test]
    fn test_unparsable_marker_view_is_none() {
        let root = marker_dir("sweep-marker-garbled");
        std::fs::write(
            root.join(crate::open_guard::WAL_MODE_MARKER_FILE),
            b"not a marker body",
        )
        .expect("write the garbage");

        assert_eq!(
            crate::open_guard::live_marker_mode(&root, &FixedLiveness(true)),
            None,
            "an unparsable marker carries no live claim"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
