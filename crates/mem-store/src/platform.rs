//! Platform process/sleep seams (blueprint W1.2).
//!
//! `ProcessProbe` answers "is this PID alive" for stale-operation-lock
//! detection; `SleepFn` carries the file-lock retry delay so tests inject a
//! recording sleeper instead of wall-clock waits (`docs/agents/testing.md`).

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

/// The two platforms upstream branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Platform {
    /// `process.platform` equivalent (`win32` detection only — every other
    /// host is "Unix" for the feature-set decisions this wave makes).
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Upstream `isProcessAlive` (operation-lock.ts L14-21, via `process.kill(0)`).
pub trait ProcessProbe: Send + Sync {
    fn is_alive(&self, pid: u32) -> bool;
}

/// The production probe.
///
/// Linux reads `/proc/{pid}` existence. On other platforms the probe is
/// deliberately conservative — answers `true` so a possibly-live foreign
/// lock can NEVER be stolen (the failure mode of a wrong "stale" verdict is
/// deleting another running session's migration lock); documented
/// divergence from upstream `process.kill(pid, 0)`, which is the same shape
/// of conservative check through the signal API.
pub struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn is_alive(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        std::path::Path::new("/proc").join(pid.to_string()).exists()
    }
}

/// Injected retry delay (upstream `delay(ms)` — `setTimeout` inside the
/// Windows lock retry). Production uses [`system_sleep`]; tests record the
/// requested delays instead of sleeping.
pub type SleepFn = Arc<dyn Fn(Duration) + Send + Sync>;

/// Wall-clock production sleeper.
pub fn system_sleep(delay: Duration) {
    std::thread::sleep(delay);
}
