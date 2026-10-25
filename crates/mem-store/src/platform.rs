//! Platform process/sleep seams (blueprint W1.2).
//!
//! `ProcessProbe` answers "is this PID alive" for stale-operation-lock
//! detection; `SleepFn` carries the file-lock retry delay so tests inject a
//! recording sleeper instead of wall-clock waits (`docs/agents/testing.md`).
//!
//! Liveness is a **fail-closed** decision. A lock is a file whose body names a
//! pid; a wrong "dead" verdict unlinks another process's live lock
//! (`operation_lock::read_live_lock`) and the next caller owns the storage root
//! too (`engine_lock::acquire_engine_lock`) — two writers, one database. Only
//! two facts may therefore produce "dead":
//!
//! 1. the pid does not exist at all, or
//! 2. the pid exists but belongs to a **different** process than the one that
//!    wrote the record — the recorded start-time token disagrees with the
//!    observed one, i.e. the pid was recycled.
//!
//! Everything unverifiable answers "alive".
//!
//! The decision is [`decide_liveness`], a pure function of
//! `(pid, recorded token, observed process, host observability)`. The `cfg`
//! lives in the *gathering* layer ([`ProcObservability::current`],
//! [`observe_process`]) and never inside the decision, so every cell of the
//! platform × token table is unit-testable on a single host.

#![allow(dead_code)]

use std::path::Path;
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

/// The stale-lock verdict, deliberately two-valued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Treat the lock as held — never unlink it.
    Alive,
    /// The recorded holder is gone, or its pid now names another process.
    Dead,
}

impl Liveness {
    /// `true` for [`Liveness::Alive`] — the "do not unlink the lock" verdict.
    #[must_use]
    pub fn is_alive(self) -> bool {
        matches!(self, Self::Alive)
    }
}

/// What the **host** can verify about a pid.
///
/// This is the platform branch, expressed as data. It is passed into
/// [`decide_liveness`] rather than read by it, so off-Linux behaviour is
/// testable from Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcObservability {
    /// Linux: `/proc` yields both existence and a start-time token.
    ProcFs,
    /// No procfs (every non-Linux host, Windows included): neither existence
    /// nor a start-time token is verifiable.
    Unobservable,
}

impl ProcObservability {
    /// The running host's observability — the only `cfg`-shaped choice on the
    /// liveness path, and it sits *outside* the tested decision.
    ///
    /// Linux with no usable `/proc` (a stripped container, an unmounted
    /// procfs) is deliberately **not** `ProcFs`: there every pid reads absent
    /// and every foreign lock would be stolen, so such a host is reported
    /// `Unobservable` and fails closed.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(target_os = "linux") && Path::new("/proc/self/stat").exists() {
            Self::ProcFs
        } else {
            Self::Unobservable
        }
    }
}

/// What the gathering layer actually saw for a pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedProcess {
    /// No `/proc/{pid}` entry (or no procfs to look in).
    Absent,
    /// The pid exists. `start_time` is `Some` when `/proc/{pid}/stat` could be
    /// read, `None` when it exists but the stat fields were unreadable.
    Present {
        /// Field 22 of `/proc/{pid}/stat` — `starttime`, in clock ticks since
        /// boot (`CLK_TCK`, normally 100, so it is not wall time).
        start_time: Option<u64>,
    },
}

/// The whole liveness rule, as a pure function.
///
/// | observability | pid | recorded token | observed                 | verdict |
/// |---------------|-----|----------------|--------------------------|---------|
/// | any           | 0   | any            | any                      | `Dead`  |
/// | `Unobservable`| >0  | any            | any                      | `Alive` |
/// | `ProcFs`      | >0  | any            | `Absent`                 | `Dead`  |
/// | `ProcFs`      | >0  | absent         | `Present{Some(_)}`       | `Alive` |
/// | `ProcFs`      | >0  | `Some(_)`      | `Present{None}`          | `Alive` |
/// | `ProcFs`      | >0  | `Some(t)`      | `Present{Some(t)}`       | `Alive` |
/// | `ProcFs`      | >0  | `Some(t)`      | `Present{Some(other)}`   | `Dead`  |
///
/// The `Unobservable` row is checked **before** the observation is inspected:
/// an unverifiable host never returns `Dead`, whatever it was handed.
#[must_use]
pub fn decide_liveness(
    pid: u32,
    recorded_start: Option<u64>,
    observed: ObservedProcess,
    observability: ProcObservability,
) -> Liveness {
    // pid 0 is never a user process (and `read_live_lock` requires pid > 0), so
    // a null id is dead on every platform.
    if pid == 0 {
        return Liveness::Dead;
    }
    // Fail closed. Off Linux nothing can be verified, so a possibly-live
    // foreign lock must never be stolen.
    if observability == ProcObservability::Unobservable {
        return Liveness::Alive;
    }
    match observed {
        ObservedProcess::Absent => Liveness::Dead,
        ObservedProcess::Present { start_time: None } => Liveness::Alive,
        ObservedProcess::Present {
            start_time: Some(observed),
        } => match recorded_start {
            // The one dead verdict on an existing pid: the record's token names
            // a process that no longer exists, and this pid belongs to someone
            // else now (PID reuse).
            Some(recorded) if recorded != observed => Liveness::Dead,
            // Matching token, legacy record without a token, or nothing to
            // compare: alive.
            _ => Liveness::Alive,
        },
    }
}

/// Reads the start-time token of `pid` from `/proc/{pid}/stat` field 22.
///
/// `comm` (field 2) is parenthesised and may itself contain spaces and `)`, so
/// the fields are counted from the **last** `)` in the line: the next token is
/// field 3 (`state`), making field 22 token index `22 - 3 = 19`.
#[cfg(target_os = "linux")]
fn read_proc_start_time(pid: u32) -> Option<u64> {
    let stat =
        std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

/// Non-Linux hosts have no procfs to read a start-time token from.
#[cfg(not(target_os = "linux"))]
fn read_proc_start_time(_pid: u32) -> Option<u64> {
    None
}

/// The start-time token of `pid` as this host can see it.
///
/// Used when *writing* a lock record (the writer records its own token) and by
/// [`observe_process`]. `None` means "unobservable", which the decision treats
/// conservatively.
#[must_use]
pub fn process_start_time(pid: u32) -> Option<u64> {
    read_proc_start_time(pid)
}

/// Gathers what this host can see about `pid` for [`decide_liveness`].
#[cfg(target_os = "linux")]
#[must_use]
pub fn observe_process(pid: u32) -> ObservedProcess {
    if pid == 0 {
        return ObservedProcess::Absent;
    }
    match std::fs::metadata(Path::new("/proc").join(pid.to_string())) {
        // The process may exit between these two reads; that is why the token
        // is optional rather than an error (an unreadable token means "alive").
        Ok(_) => ObservedProcess::Present {
            start_time: read_proc_start_time(pid),
        },
        // The only absence accepted as proof of death.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ObservedProcess::Absent,
        // Any other error (permissions, `hidepid=` mounts, I/O errors) cannot
        // prove the process is gone, so the holder stays alive. Caveat: a
        // `hidepid=` mount reports ENOENT for other users' pids, so that case is
        // indistinguishable from absence — a pre-existing limit of a
        // `/proc`-based probe, not something the start-time token changes.
        Err(_) => ObservedProcess::Present { start_time: None },
    }
}

/// No procfs off Linux: nothing is observable.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn observe_process(_pid: u32) -> ObservedProcess {
    ObservedProcess::Absent
}

/// Upstream `isProcessAlive` (operation-lock.ts L14-21, via `process.kill(0)`).
pub trait ProcessProbe: Send + Sync {
    /// Existence-only query (the original seam, kept for the callers that have
    /// no lock record to feed it).
    fn is_alive(&self, pid: u32) -> bool;

    /// Token-aware query used by `read_live_lock`.
    ///
    /// The default delegates to [`Self::is_alive`] and therefore ignores the
    /// token: a probe that cannot observe start times (every fake, and any
    /// future non-procfs probe) keeps exactly the liveness semantics it had
    /// before the token existed. `SystemProcessProbe` overrides it.
    fn liveness(&self, pid: u32, _recorded_start: Option<u64>) -> Liveness {
        if self.is_alive(pid) {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    }
}

/// The production probe.
///
/// The verdict is [`decide_liveness`], fed by [`ProcObservability::current`]
/// and [`observe_process`]:
///
/// * **Linux** — `/proc/{pid}` existence plus the `starttime` token. A recorded
///   token that disagrees with the observed one means the pid was recycled, so
///   the recorded holder is dead.
/// * **Everywhere else** — [`ProcObservability::Unobservable`], which answers
///   `Alive` for any non-zero pid: a possibly-live foreign lock can NEVER be
///   stolen (the failure mode of a wrong "stale" verdict is deleting another
///   running session's migration lock). Accepted residual: a genuinely dead
///   holder on a non-Linux host keeps its lock file until an operator removes
///   it — the safe direction. A real probe there needs `libc`/`sysinfo`; that
///   is a dependency decision `AGENTS.md` reserves for an issue, so this module
///   implements the documented conservative answer instead of inventing one.
pub struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn is_alive(&self, pid: u32) -> bool {
        // No record to compare against: existence only, with the same
        // conservative off-Linux answer.
        decide_liveness(
            pid,
            None,
            observe_process(pid),
            ProcObservability::current(),
        )
        .is_alive()
    }

    fn liveness(&self, pid: u32, recorded_start: Option<u64>) -> Liveness {
        decide_liveness(
            pid,
            recorded_start,
            observe_process(pid),
            ProcObservability::current(),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The three host classes the probe must answer for. Only Linux can be run
    /// here, so they are named as data and every class goes through
    /// [`decide_liveness`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Host {
        Linux,
        OtherUnix,
        Windows,
    }

    impl Host {
        const ALL: [Self; 3] = [Self::Linux, Self::OtherUnix, Self::Windows];

        /// What [`ProcObservability::current`] yields on that host.
        fn observability(self) -> ProcObservability {
            match self {
                Self::Linux => ProcObservability::ProcFs,
                Self::OtherUnix | Self::Windows => ProcObservability::Unobservable,
            }
        }

        /// What [`observe_process`] yields on that host for a *live* pid.
        fn live_observation(self) -> ObservedProcess {
            match self {
                Self::Linux => ObservedProcess::Present {
                    start_time: Some(7_654_321),
                },
                Self::OtherUnix | Self::Windows => ObservedProcess::Absent,
            }
        }
    }

    /// Both directions, per platform: an alive observation reads `Alive` and a
    /// dead one reads `Dead` on Linux — while off Linux BOTH read `Alive`.
    ///
    /// Inverted world: with the shipped bug (`/proc` consulted on every host)
    /// `OtherUnix`/`Windows` + `Absent` would answer `Dead` and this test fails.
    #[test]
    fn test_decide_liveness_both_directions_per_platform() {
        for host in Host::ALL {
            let observability = host.observability();
            let token = Some(7_654_321);

            // "Alive" direction: a pid that exists with a matching token.
            assert_eq!(
                decide_liveness(42, token, host.live_observation(), observability),
                Liveness::Alive,
                "{host:?}: a live holder must read alive"
            );

            // "Dead" direction: no process behind the pid at all.
            assert_eq!(
                decide_liveness(42, token, ObservedProcess::Absent, observability),
                if host == Host::Linux {
                    Liveness::Dead
                } else {
                    // Off Linux nothing is verifiable: fail closed.
                    Liveness::Alive
                },
                "{host:?}: absence of the pid means dead only where it can be observed"
            );

            // Off Linux the gate beats the observation, even a contradictory
            // one: an existing pid with a *mismatched* token is still alive.
            if host != Host::Linux {
                assert_eq!(
                    decide_liveness(
                        42,
                        Some(1),
                        ObservedProcess::Present {
                            start_time: Some(2)
                        },
                        observability
                    ),
                    Liveness::Alive,
                    "{host:?}: unverifiable holder must never be stolen"
                );
            }
        }
    }

    /// pid 0 is a null id, not a process: dead on every host, including the
    /// unobservable ones.
    ///
    /// Inverted world: if the `Unobservable` short-circuit ran before the null
    /// check, `(0, Unobservable)` would answer `Alive` and this test fails.
    #[test]
    fn test_decide_liveness_pid_zero_is_dead_everywhere() {
        for host in Host::ALL {
            let observability = host.observability();
            for observed in [
                ObservedProcess::Absent,
                ObservedProcess::Present {
                    start_time: Some(5),
                },
                ObservedProcess::Present { start_time: None },
            ] {
                for recorded in [None, Some(5), Some(6)] {
                    assert_eq!(
                        decide_liveness(0, recorded, observed, observability),
                        Liveness::Dead,
                        "{host:?}: pid 0 is never a holder (observed {observed:?}, recorded {recorded:?})"
                    );
                }
            }
        }
    }

    /// PID reuse: the token decides. A record naming a pid that now carries a
    /// different start time is stale.
    ///
    /// Inverted world: if the token were ignored (the pre-fix behaviour), the
    /// mismatch cell would answer `Alive` and the lock would never be
    /// reclaimed.
    #[test]
    fn test_decide_liveness_pid_reuse_is_dead_on_mismatch() {
        let linux = ProcObservability::ProcFs;
        assert_eq!(
            decide_liveness(
                4242,
                Some(1_000),
                ObservedProcess::Present {
                    start_time: Some(2_000)
                },
                linux
            ),
            Liveness::Dead,
            "a recycled pid must not validate the previous owner's lock"
        );
        assert_eq!(
            decide_liveness(
                4242,
                Some(1_000),
                ObservedProcess::Present {
                    start_time: Some(1_000)
                },
                linux
            ),
            Liveness::Alive,
            "the same pid with the same start time is the same process"
        );
    }

    /// Unverifiable tokens are conservative: no recorded token (legacy record)
    /// or no observed token (unreadable stat) reads `Alive`.
    ///
    /// Inverted world: if an absent token were treated as "obviously not the
    /// holder", both cells would answer `Dead` and an old lock file would be
    /// stolen from a live process.
    #[test]
    fn test_decide_liveness_unobservable_token_is_alive() {
        let linux = ProcObservability::ProcFs;
        assert_eq!(
            decide_liveness(
                4242,
                None,
                ObservedProcess::Present {
                    start_time: Some(9)
                },
                linux
            ),
            Liveness::Alive,
            "a legacy record without a token cannot prove a reuse"
        );
        assert_eq!(
            decide_liveness(
                4242,
                Some(9),
                ObservedProcess::Present { start_time: None },
                linux
            ),
            Liveness::Alive,
            "an unreadable observed token cannot prove a reuse"
        );
        assert_eq!(
            decide_liveness(
                4242,
                None,
                ObservedProcess::Absent,
                ProcObservability::Unobservable
            ),
            Liveness::Alive,
            "a non-procfs host never declares a holder dead"
        );
    }

    /// The real probe: `/proc` is read, the token is read, and a forged token
    /// is dead. This is the only cell that exercises the `cfg` gathering side.
    ///
    /// Inverted world: if `ProcessProbe::liveness` still delegated to the
    /// existing-pid check, the forged-token assertion would answer `Alive`.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_system_probe_reads_its_own_start_time_token() {
        let probe = SystemProcessProbe;
        let me = std::process::id();

        assert!(probe.is_alive(me), "this process is alive");
        let own = process_start_time(me).expect("own /proc/{pid}/stat is readable");
        assert_eq!(
            process_start_time(me),
            Some(own),
            "the start-time token is stable for a live process"
        );

        assert_eq!(
            probe.liveness(me, Some(own)),
            Liveness::Alive,
            "the recorded owner token validates"
        );
        assert_eq!(
            probe.liveness(me, Some(own.wrapping_add(1))),
            Liveness::Dead,
            "a different token means the pid was recycled"
        );
        assert_eq!(
            probe.liveness(me, None),
            Liveness::Alive,
            "a record without a token stays conservative"
        );
    }

    /// `/proc/{pid}/stat` field 22 is what the parser returns: cross-check a
    /// child's token against the parent's and against the file itself.
    ///
    /// Inverted world: an off-by-one field index (e.g. 20 instead of 22) would
    /// return a different column and the direct comparison below fails.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_start_time_parser_selects_stat_field_22() {
        let me = std::process::id();
        let stat = std::fs::read_to_string(format!("/proc/{me}/stat")).expect("read own stat");
        let after_comm = stat.rsplit_once(')').expect("comm is parenthesised").1;
        let manual: u64 = after_comm
            .split_whitespace()
            .nth(19)
            .expect("field 22 is present")
            .parse()
            .expect("field 22 is an integer");

        assert_eq!(process_start_time(me), Some(manual));
        // A child started later starts later (monotonic clock ticks).
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let child_token = process_start_time(child.id());
        assert!(
            child_token.is_some_and(|child| child >= manual),
            "the child's token is readable and not older than ours: {child_token:?} vs {manual}"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
