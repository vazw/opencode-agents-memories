//! Port of upstream `tests/profile-learning-lock.test.ts` (W3.1 step 10).
//!
//! Cross-process profile-learning lock over the coordination DB. Upstream's
//! Bun worker fixture becomes the `fixture_child_entry` subprocess (one
//! `#[ignore]`d entry executed by self re-exec, `docs/agents/testing.md`), and
//! upstream's env-gated `PLL_CAS_HOOKS` becomes the `AcquireHooks` seam.
//!
//! Upstream cases (traced 1:1 by name, 15 total):
//! - "grants the lock to exactly one process and release frees it for the next"
//! - "release is idempotent"
//! - "never steals a live lock, even held past 30 minutes"
//! - "never steals a lock owned by a process we cannot signal (EPERM)"
//! - "exactly one of two competitors CAS-reclaims the same dead owner"
//! - "a stale release cannot drop a new owner's row, even with the same PID"
//! - "recovers the lock after the holder is SIGKILLed without releasing"
//! - "reclaims a live PID only when the startup identity differs (PID reuse)"
//! - "reclaims an owner from a previous boot even though its PID is alive now"
//! - "fails closed on malformed coordination state"
//! - "proc ENOENT alone never kills; only the kill probe decides"
//! - "boot_id variants: malformed fails closed; valid-null and same-as-host stay live"
//! - "stored starttime identity: strict format, cross-kind never kills, only
//!   same-kind mismatch reclaims"
//! - "an untrusted local boot_id never acts as a dead signal"
//! - "fails closed when the coordination database cannot be opened"
//!
//! Positive-invariant mandate (mem_1792345954123): every reclaim case asserts
//! the row WAS held with the old token before reclaim, the new owner's token
//! while held, and its absence after release; loser-side negatives are never
//! standalone.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use mem_profile::learning_lock::{
    BootIdSource, LearningLock, LearningLockOptions, ProcStat, ProcStatSource, SignalOutcome,
    SignalProbe, StarttimeKind, SystemBootIdSource, SystemProcStatSource, parse_stored_starttime,
};

use common::{
    PlantedOwner, current_owner_token, plant_coordination_owner, read_child_report,
    spawn_fixture_child, wait_bounded, wait_for_barrier, write_child_report,
};

/// `2^22` exceeds every default Linux `pid_max`, so it can never be alive.
const DEAD_PID: u32 = 4_194_304;

const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
const CHILD_TIMEOUT: Duration = Duration::from_secs(45);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        Self(common::temp_dir(prefix))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.0);
    }
}

fn lock_for(dir: &Path) -> LearningLock {
    LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    ))
}

fn local_boot_id() -> Option<String> {
    SystemBootIdSource.read()
}

fn local_starttime(pid: u32) -> Option<String> {
    match SystemProcStatSource.read(pid) {
        ProcStat::Ok(value) => Some(value),
        _ => None,
    }
}

fn euid_is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and no side effects.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// A VALID current-platform starttime that is simply not this process's:
/// numeric ticks on Linux (a real field-22 value no modern process has).
fn local_kind_mismatch_starttime() -> String {
    if local_starttime(std::process::id()).is_some_and(|value| {
        parse_stored_starttime(&value).map(|p| p.kind) == Some(StarttimeKind::Darwin)
    }) {
        "darwin:Sun Nov 16 10:11:12 2025".to_string()
    } else {
        "1".to_string()
    }
}

/// A VALID current-platform starttime for a dead-pid row (well-formed, and
/// guaranteed to differ from anything readable for a live process).
fn local_kind_dead_pid_starttime() -> String {
    if local_starttime(std::process::id()).is_some_and(|value| {
        parse_stored_starttime(&value).map(|p| p.kind) == Some(StarttimeKind::Darwin)
    }) {
        "darwin:Sun Nov 16 10:11:12 2025".to_string()
    } else {
        "12345678".to_string()
    }
}

fn foreign_kind_starttime() -> String {
    if local_starttime(std::process::id()).is_some_and(|value| {
        parse_stored_starttime(&value).map(|p| p.kind) == Some(StarttimeKind::Darwin)
    }) {
        "12345678".to_string()
    } else {
        "darwin:Sat Oct  3 09:00:00 2026".to_string()
    }
}

/// The identity this process records (boot id + own starttime).
fn own_identity() -> (Option<String>, Option<String>) {
    (local_boot_id(), local_starttime(std::process::id()))
}

// ---------------------------------------------------------------------------
// Child process plumbing
// ---------------------------------------------------------------------------

struct Child {
    child: std::process::Child,
    out: PathBuf,
    /// The fixture mode (e.g. `lock:cas-race:b`) — the identity a timeout
    /// message names.
    label: String,
}

impl Child {
    fn spawn(dir: &Path, mode: &str) -> Self {
        let out = dir.join(format!("child-{}.out", mode.replace(':', "-")));
        let mut env: Vec<(String, String)> = common::isolated_env(dir);
        env.push(("MEM_FIXTURE_STORAGE".to_string(), dir.display().to_string()));
        let refs: Vec<(&str, &str)> = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let child = spawn_fixture_child(mode, &out, &refs).expect("spawn fixture child");
        Self {
            child,
            out,
            label: mode.to_string(),
        }
    }

    fn report(mut self) -> serde_json::Value {
        let status = wait_bounded(&mut self.child, CHILD_TIMEOUT)
            .unwrap_or_else(|error| panic!("child {error:?}"));
        assert!(
            status.success(),
            "child exited with {status:?}; stderr: {}",
            std::fs::read_to_string(self.out.with_extension("stderr")).unwrap_or_default()
        );
        read_child_report(&self.out)
    }

    fn wait_for_barrier(&mut self, dir: &Path, name: &str) {
        self.wait_for_barrier_for(dir, name, BARRIER_TIMEOUT);
    }

    /// Bounded barrier wait with a timeout message that names the side, the
    /// child and its artifacts. The artifacts are collected **before** the
    /// kill, and every read is panic-free: a panic while panicking aborts the
    /// binary, so only `unwrap_or_else`/`unwrap_or_default` appear on this path.
    fn wait_for_barrier_for(&mut self, dir: &Path, name: &str, timeout: Duration) {
        if wait_for_barrier(dir, name, timeout) {
            return;
        }
        let status = match self.child.try_wait() {
            Ok(Some(status)) => format!("exited ({status})"),
            Ok(None) => "still running (parked)".to_string(),
            Err(error) => format!("try_wait failed ({error})"),
        };
        let report = read_bounded(&self.out.with_extension("json"));
        let stderr = read_bounded(&self.out.with_extension("stderr"));
        let _ = self.child.kill();
        let _ = self.child.wait();
        panic!(
            "barrier hs.{name} never appeared within {timeout:?}; child={label}; status={status}; \
             report={report}; stderr={stderr}",
            label = self.label,
        );
    }

    fn signal(&self, signal: i32) {
        #[cfg(unix)]
        {
            // SAFETY: the child pid is live and owned by this process.
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, signal);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = signal;
        }
    }

    fn kill_and_reap(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Child {
    /// A parked peer is killed and reaped on **every** panic path — today it
    /// would otherwise exit only when its own 30 s `go.update` wait expires.
    /// A second `kill`/`wait` on a reaped child is a no-op and is ignored.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Panic-free artifact read for the timeout path: an absent file becomes a
/// readable token instead of a second panic, and the text is truncated to 4096
/// bytes on a `char` boundary (never mid-codepoint).
fn read_bounded(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| format!("<absent: {error}>"));
    if text.len() <= 4096 {
        return text;
    }
    let cutoff = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= 4096)
        .last()
        .unwrap_or(0);
    format!("{}…<truncated>", &text[..cutoff])
}

/// Extracts a caught panic payload's text (the timeout test asserts on it).
#[cfg(unix)]
fn panic_payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// `kill(pid, 0)` is the only observable that separates a killed-and-**reaped**
/// child (`ESRCH`) from a killed-but-unreaped **zombie** (`0`), which is why
/// this pins the reap as well as the kill. A survivor is SIGKILLed before the
/// assertion fails: no fixture child may keep the inherited `.tmp/build.lock`
/// descriptor.
#[cfg(unix)]
fn assert_reaped(pid: u32, phase: &str) {
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if alive == 0 {
        // SAFETY: signalling a pid this test spawned and whose death it is
        // asserting; the signal is SIGKILL and the child is already doomed.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        panic!("{phase}: child pid {pid} was killed but not reaped (kill(pid, 0) still answers 0)");
    }
    let errno = std::io::Error::last_os_error().raw_os_error();
    assert_eq!(
        errno,
        Some(libc::ESRCH),
        "{phase}: child pid {pid} must answer ESRCH (killed and reaped), got errno {errno:?}"
    );
}

fn run_child(dir: &Path, mode: &str) -> serde_json::Value {
    Child::spawn(dir, mode).report()
}

/// Writes a handshake barrier file.
fn signal_barrier(dir: &Path, name: &str) {
    std::fs::write(common::barrier_path(dir, name), b"ready").expect("write barrier");
}

fn read_barrier(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(common::barrier_path(dir, name)).expect("read barrier")
}

// ---------------------------------------------------------------------------
// Issue 60 — the timeout names the side and the cause; Drop kills and reaps
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn test_barrier_timeout_message_names_the_side_the_exit_status_and_the_child_artifacts() {
    let dir = TempDir::new("lock-timeout-message");
    let mut child = Child::spawn(dir.path(), "lock:try");
    let status = wait_bounded(&mut child.child, CHILD_TIMEOUT).expect("the lock:try child exits");
    assert!(
        status.success(),
        "the lock:try child must exit 0, so the status text is stable: {status:?}"
    );
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        child.wait_for_barrier_for(dir.path(), "seen.never", Duration::from_millis(50));
    }))
    .expect_err("a barrier that never appears must panic");
    let message = panic_payload_text(payload.as_ref());
    for needle in [
        "hs.seen.never",
        "lock:try",
        "exit status: 0",
        "\"acquired\":true",
        "stderr",
    ] {
        assert!(
            message.contains(needle),
            "the timeout message must name {needle:?}: {message}"
        );
    }
}

#[cfg(unix)]
#[test]
fn test_drop_kills_and_reaps_a_parked_fixture_child() {
    // Phase 1 — direct `drop` on a parked `lock:hold-forever` child.
    let dir = TempDir::new("lock-drop-reap");
    let mut child = Child::spawn(dir.path(), "lock:hold-forever");
    child.wait_for_barrier(dir.path(), "acquired");
    let pid = child.child.id();
    drop(child);
    assert_reaped(pid, "phase 1 (direct drop)");

    // Phase 2 — the unwind path. `_parked` MUST own the Child: `let _ = parked`
    // does not bind and drops it immediately, and `drop(parked)` before the
    // panic drops it before the unwind. Either shape makes the "unwind → Drop"
    // claim vacuous. The `move` closure takes ownership, and `catch_unwind`
    // needs `AssertUnwindSafe` because `Child` is not `UnwindSafe`.
    let dir = TempDir::new("lock-unwind-reap");
    let mut parked = Child::spawn(dir.path(), "lock:hold-forever");
    parked.wait_for_barrier(dir.path(), "acquired");
    let pid = parked.child.id();
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _parked = parked;
        panic!("synthetic unwind with the parked peer in scope");
    }));
    assert!(payload.is_err(), "the synthetic panic must unwind");
    assert_reaped(pid, "phase 2 (unwind drop)");
}

// ---------------------------------------------------------------------------
// 1/15 — one grant, reacquire after release
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grants_lock_to_exactly_one_process_and_release_frees_next() {
    let dir = TempDir::new("lock-grant");
    let mut winner = Child::spawn(dir.path(), "lock:hold");
    winner.wait_for_barrier(dir.path(), "acquired");

    // Positive invariant: while the winner is parked INSIDE its critical
    // section the row exists and carries the winner's token.
    assert!(
        current_owner_token(dir.path()).await.is_some(),
        "the winner must hold a visible coordination row while parked"
    );

    // The loser's attempt fully completes before release.
    let loser = run_child(dir.path(), "lock:try");
    assert_eq!(loser["acquired"], serde_json::Value::Bool(false));

    signal_barrier(dir.path(), "release");
    let winner_report = winner.report();
    assert_eq!(
        winner_report,
        serde_json::json!({"acquired": true, "heldBefore": true, "heldAfter": false})
    );

    // The freed lock is acquirable again, and the probe releases it.
    let again = run_child(dir.path(), "lock:probe");
    assert_eq!(again["acquired"], serde_json::Value::Bool(true));
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// 2/15 — release idempotence (in-process)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_release_is_idempotent() {
    let dir = TempDir::new("lock-release-twice");
    let lock = lock_for(dir.path());

    let guard = lock.try_acquire(None).await.expect("first acquire");
    assert!(lock.is_held().await, "the acquired row is visible");
    assert_eq!(
        current_owner_token(dir.path()).await,
        Some(guard.owner_token().to_string())
    );

    guard.release().await;
    assert!(!lock.is_held().await, "the row is gone after release");
    guard.release().await;
    assert!(!lock.is_held().await, "a second release is a no-op");
}

// ---------------------------------------------------------------------------
// 3/15 — never steal a live lock, no matter its age
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_live_owner_never_stolen_even_after_thirty_minutes() {
    let dir = TempDir::new("lock-live-30min");
    let (boot_id, starttime) = own_identity();
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: std::process::id(),
            boot_id,
            starttime,
            acquired_at: mem_profile::ids::now_millis() - 31 * 60 * 1000,
            ..Default::default()
        },
    )
    .await;
    // Positive precondition: the row really was planted.
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    let probe = run_child(dir.path(), "lock:probe");
    assert_eq!(probe["acquired"], serde_json::Value::Bool(false));
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token"),
        "the live owner's row must be untouched"
    );
}

// ---------------------------------------------------------------------------
// 4/15 — EPERM owner is alive, never stolen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_eperm_owner_never_stolen() {
    if euid_is_root() || cfg!(windows) {
        return;
    }
    let dir = TempDir::new("lock-eperm");
    // PID 1 is alive and root-owned: kill(1, 0) yields EPERM for a non-root
    // caller. No starttime identity is recorded, so the signal probe decides.
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: 1,
            starttime: None,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    let probe = run_child(dir.path(), "lock:probe");
    assert_eq!(probe["acquired"], serde_json::Value::Bool(false));
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );
}

// ---------------------------------------------------------------------------
// 5/15 — exactly one CAS competitor reclaims
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_exactly_one_competitor_cas_reclaims_dead_owner() {
    let dir = TempDir::new("lock-cas-race");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: DEAD_PID,
            starttime: Some(local_kind_dead_pid_starttime()),
            ..Default::default()
        },
    )
    .await;

    let mut a = Child::spawn(dir.path(), "lock:cas-race:a");
    let mut b = Child::spawn(dir.path(), "lock:cas-race:b");
    a.wait_for_barrier(dir.path(), "seen.a");
    b.wait_for_barrier(dir.path(), "seen.b");

    // Both sides observed the SAME stale owner token before either CAS fires.
    let token_a = read_barrier(dir.path(), "token.a");
    let token_b = read_barrier(dir.path(), "token.b");
    assert_eq!(token_a, token_b);
    assert_eq!(token_a, "planted-owner-token");

    signal_barrier(dir.path(), "go.update");

    let report_a = a.report();
    let report_b = b.report();
    let acquired: Vec<bool> = [report_a["acquired"].clone(), report_b["acquired"].clone()]
        .into_iter()
        .filter_map(|value| value.as_bool())
        .filter(|acquired| *acquired)
        .collect();
    assert_eq!(
        acquired.len(),
        1,
        "CAS on the same token: exactly one winner"
    );

    // The winner released on exit, so the lock is free again.
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// 6/15 — a stale release cannot drop a new owner's row (same PID)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stale_release_cannot_drop_new_owner_row_same_pid() {
    let dir = TempDir::new("lock-stale-release");
    let lock = lock_for(dir.path());
    let old_guard = lock.try_acquire(None).await.expect("acquire T1");
    // Positive precondition: T1's row is really on disk.
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some(old_guard.owner_token())
    );

    // Simulate a takeover that reuses the same PID: the row now belongs to a
    // NEW owner token.
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            token: "simulated-new-owner-token".to_string(),
            pid: std::process::id(),
            boot_id: local_boot_id(),
            starttime: local_starttime(std::process::id()),
            ..Default::default()
        },
    )
    .await;

    old_guard.release().await;

    assert!(
        lock.is_held().await,
        "the new owner's row must survive the stale release"
    );
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("simulated-new-owner-token")
    );
}

// ---------------------------------------------------------------------------
// 7/15 — SIGKILL recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sigkilled_holder_lock_reclaimed_by_new_process() {
    let dir = TempDir::new("lock-sigkill");
    let mut holder = Child::spawn(dir.path(), "lock:hold-forever");
    holder.wait_for_barrier(dir.path(), "acquired");
    assert!(
        current_owner_token(dir.path()).await.is_some(),
        "the killed holder really held the row"
    );

    holder.signal(libc::SIGKILL);
    holder.kill_and_reap();

    let probe = run_child(dir.path(), "lock:probe");
    assert_eq!(probe["acquired"], serde_json::Value::Bool(true));
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// 8/15 — PID reuse reclaim (same-kind starttime mismatch)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pid_reuse_same_kind_mismatch_starttime_reclaims() {
    if local_starttime(std::process::id()).is_none() {
        return; // no starttime identity on this platform
    }
    let dir = TempDir::new("lock-pid-reuse");
    let (boot_id, _own) = own_identity();
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: std::process::id(),
            boot_id,
            starttime: Some(local_kind_mismatch_starttime()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token"),
        "positive precondition: the stale row exists"
    );

    let lock = lock_for(dir.path());
    let guard = lock
        .try_acquire(None)
        .await
        .expect("PID reuse is reclaimable");
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some(guard.owner_token()),
        "the new owner's token replaced the stale one"
    );
    guard.release().await;
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// 9/15 — previous-boot reclaim
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_previous_boot_owner_reclaimed_though_pid_alive() {
    if local_boot_id().is_none() {
        return; // no boot identity on this platform
    }
    let dir = TempDir::new("lock-previous-boot");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: std::process::id(),
            boot_id: Some("00000000-0000-0000-0000-000000000000".to_string()),
            starttime: local_starttime(std::process::id()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    let lock = lock_for(dir.path());
    let guard = lock
        .try_acquire(None)
        .await
        .expect("previous boot reclaims");
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some(guard.owner_token())
    );
    guard.release().await;
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// 10/15 — malformed coordination state fails closed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_malformed_coordination_row_fails_closed() {
    let dir = TempDir::new("lock-malformed");
    // pid 0 can never identify a process: the record is structurally invalid
    // and must be treated as untrustworthy, not as a free lock.
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: 0,
            ..Default::default()
        },
    )
    .await;

    let probe = run_child(dir.path(), "lock:probe");
    assert_eq!(probe["acquired"], serde_json::Value::Bool(false));
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token"),
        "a malformed row is never unlinked"
    );
}

// ---------------------------------------------------------------------------
// 11/15 — proc ENOENT alone never kills
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proc_enoent_alone_never_kills_only_signal_probe_decides() {
    // Owner pid 2^22 → /proc/<pid>/stat absent in every environment. A
    // current-platform-kind starttime makes the checker take the proc-stat
    // branch and hit the absent → kill-probe fallthrough for real.
    let mut outcomes: std::collections::BTreeMap<&str, bool> = std::collections::BTreeMap::new();
    for forced in ["EPERM", "OK", "ESRCH"] {
        let dir = TempDir::new(&format!("lock-proc-probe-{forced}"));
        plant_coordination_owner(
            dir.path(),
            &PlantedOwner {
                pid: DEAD_PID,
                starttime: Some(local_kind_dead_pid_starttime()),
                ..Default::default()
            },
        )
        .await;

        let report = run_child(dir.path(), &format!("lock:proc-probe:{forced}"));
        assert_eq!(report["forced"], serde_json::json!(forced));
        let acquired = report["acquired"].as_bool().unwrap_or(false);
        assert_eq!(
            acquired,
            forced == "ESRCH",
            "forced {forced} must read as acquired={} ({report})",
            forced == "ESRCH"
        );

        if acquired {
            assert_eq!(
                current_owner_token(dir.path()).await,
                None,
                "the ESRCH winner released on exit"
            );
        } else {
            assert_eq!(
                current_owner_token(dir.path()).await.as_deref(),
                Some("planted-owner-token"),
                "forced {forced} must leave the row untouched"
            );
        }
        outcomes.insert(forced, acquired);
    }
    assert_eq!(
        outcomes,
        std::collections::BTreeMap::from([("EPERM", false), ("OK", false), ("ESRCH", true)])
    );
}

// ---------------------------------------------------------------------------
// 12/15 — boot-id variants
// ---------------------------------------------------------------------------

/// Plants a row with a raw `boot_id` value (the upstream fixture's number /
/// object variants cannot be expressed as `Option<String>`).
async fn plant_boot_variant(dir: &Path, boot_id: mem_store::db::Value) {
    use mem_store::db::Value;
    common::with_coordination_db(dir, |db| async move {
        db.execute(mem_profile::learning_lock::LOCK_TABLE_DDL, Vec::new())
            .await
            .expect("create lock table");
        db.execute(
            "INSERT OR REPLACE INTO profile_learning_lock
              (name, owner_token, pid, boot_id, starttime, acquired_at)
            VALUES ('profile-learning', ?, ?, ?, ?, ?)",
            vec![
                Value::Text("planted-owner-token".to_string()),
                Value::Integer(i64::from(std::process::id())),
                boot_id,
                local_starttime(std::process::id()).into(),
                Value::Integer(mem_profile::ids::now_millis()),
            ],
        )
        .await
        .expect("plant row");
        db.query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
            .await
            .expect("checkpoint");
    })
    .await;
}

#[tokio::test]
async fn test_boot_id_variants_malformed_fail_closed_valid_null_and_same_stay_live() {
    use mem_store::db::Value;
    let Some(own_boot) = local_boot_id() else {
        return;
    };
    if local_starttime(std::process::id()).is_none() {
        return;
    }

    let variants: Vec<(&str, Value, bool)> = vec![
        ("validNull", Value::Null, false),
        ("invalidString", Value::Text("garbage".to_string()), false),
        ("number", Value::Integer(12_345), false),
        (
            "object",
            Value::Text("{\"not\":\"a boot id\"}".to_string()),
            false,
        ),
        ("sameAsHost", Value::Text(own_boot), false),
    ];

    for (name, boot_value, expected_acquired) in variants {
        let dir = TempDir::new(&format!("lock-boot-{name}"));
        plant_boot_variant(dir.path(), boot_value).await;
        assert_eq!(
            current_owner_token(dir.path()).await.as_deref(),
            Some("planted-owner-token"),
            "{name}: positive precondition"
        );

        let lock = lock_for(dir.path());
        let guard = lock.try_acquire(None).await;
        assert_eq!(
            guard.is_some(),
            expected_acquired,
            "{name}: live/invalid rows are never reclaimed"
        );
        assert_eq!(
            current_owner_token(dir.path()).await.as_deref(),
            Some("planted-owner-token"),
            "{name}: the planted row is untouched"
        );
    }
}

// ---------------------------------------------------------------------------
// 13/15 — stored starttime identity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_starttime_identity_variants_kind_guards_and_reclaims() {
    let Some(_own_boot) = local_boot_id() else {
        return;
    };
    let own_pid = std::process::id();
    let Some(_own_starttime) = local_starttime(own_pid) else {
        return;
    };

    // Strict format validation: every malformed stored identity fails closed
    // and leaves the planted row untouched.
    for (name, starttime) in [
        ("bare-darwin-prefix", "darwin:"),
        ("darwin-garbage-suffix", "darwin:garbage"),
        ("darwin-malformed-lstart", "darwin:not-a-date-at-all"),
        ("darwin-partial-lstart", "darwin:Sat Oct 3"),
        ("darwin-24h-violation", "darwin:Sat Oct  3 25:61:61 2026"),
        ("empty-string", ""),
        ("prefix-numeric-junk", "darwin:12345"),
    ] {
        let dir = TempDir::new(&format!("lock-starttime-{name}"));
        plant_coordination_owner(
            dir.path(),
            &PlantedOwner {
                pid: DEAD_PID,
                starttime: Some(starttime.to_string()),
                ..Default::default()
            },
        )
        .await;

        let lock = lock_for(dir.path());
        assert!(
            lock.try_acquire(None).await.is_none(),
            "{name}: malformed identity must fail closed"
        );
        assert_eq!(
            current_owner_token(dir.path()).await.as_deref(),
            Some("planted-owner-token"),
            "{name}: row untouched"
        );
    }

    // Gate 1 regression: a live holder whose stored starttime is a valid
    // FOREIGN-platform format must never be CAS-stolen via a bogus mismatch.
    let dir = TempDir::new("lock-starttime-foreign-live");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: own_pid,
            starttime: Some(foreign_kind_starttime()),
            ..Default::default()
        },
    )
    .await;
    let lock = lock_for(dir.path());
    assert!(
        lock.try_acquire(None).await.is_none(),
        "a foreign-kind identity on a live owner is not comparable"
    );
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    // Incomparable identity on a genuinely dead PID still never asserts death.
    let dir = TempDir::new("lock-starttime-foreign-dead");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: DEAD_PID,
            starttime: Some(foreign_kind_starttime()),
            ..Default::default()
        },
    )
    .await;
    let lock = lock_for(dir.path());
    assert!(lock.try_acquire(None).await.is_none());
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    // Control: a LOCAL-kind identity on a provably dead PID still reclaims.
    let dir = TempDir::new("lock-starttime-local-dead");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: DEAD_PID,
            starttime: Some(local_kind_dead_pid_starttime()),
            ..Default::default()
        },
    )
    .await;
    let lock = lock_for(dir.path());
    let guard = lock
        .try_acquire(None)
        .await
        .expect("local-kind dead pid reclaims");
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some(guard.owner_token())
    );
    guard.release().await;
    assert_eq!(current_owner_token(dir.path()).await, None);

    // Live owner with its exact own starttime: never reclaimed.
    let dir = TempDir::new("lock-starttime-live-exact");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: own_pid,
            starttime: local_starttime(own_pid),
            ..Default::default()
        },
    )
    .await;
    let lock = lock_for(dir.path());
    assert!(lock.try_acquire(None).await.is_none());
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    // Null starttime on a live owner: the signal probe decides (alive).
    let dir = TempDir::new("lock-starttime-live-null");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: own_pid,
            starttime: None,
            ..Default::default()
        },
    )
    .await;
    let lock = lock_for(dir.path());
    assert!(lock.try_acquire(None).await.is_none());
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );

    // Same-kind mismatch on a live PID: legitimate PID-reuse reclaim.
    let dir = TempDir::new("lock-starttime-same-kind-mismatch");
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: own_pid,
            starttime: Some(local_kind_mismatch_starttime()),
            ..Default::default()
        },
    )
    .await;
    let lock = lock_for(dir.path());
    let guard = lock
        .try_acquire(None)
        .await
        .expect("same-kind mismatch reclaims");
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some(guard.owner_token())
    );
    guard.release().await;
    assert_eq!(current_owner_token(dir.path()).await, None);
}

// ---------------------------------------------------------------------------
// 14/15 — an untrusted local boot id never acts as a dead signal
// ---------------------------------------------------------------------------

struct FixedBoot(Option<String>);

impl BootIdSource for FixedBoot {
    fn read(&self) -> Option<String> {
        self.0.clone()
    }
}

#[tokio::test]
async fn test_untrusted_local_boot_id_never_dead_signal() {
    if local_starttime(std::process::id()).is_none() {
        return;
    }
    let dir = TempDir::new("lock-untrusted-boot");
    // The local boot reader is garbage (None / unreadable), the stored row has
    // a well-formed foreign boot id and this process's own starttime.
    plant_coordination_owner(
        dir.path(),
        &PlantedOwner {
            pid: std::process::id(),
            boot_id: Some("00000000-0000-0000-0000-000000000000".to_string()),
            starttime: local_starttime(std::process::id()),
            ..Default::default()
        },
    )
    .await;

    let mut options = LearningLockOptions::new(
        dir.path().to_path_buf(),
        common::store_config(dir.path().to_path_buf()),
    );
    options.boot_id = std::sync::Arc::new(FixedBoot(None));
    let lock = LearningLock::new(options);

    assert!(
        lock.try_acquire(None).await.is_none(),
        "an untrusted local boot id must never become a dead signal"
    );
    assert_eq!(
        current_owner_token(dir.path()).await.as_deref(),
        Some("planted-owner-token")
    );
}

// ---------------------------------------------------------------------------
// 15/15 — unopenable coordination DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_coordination_db_unopenable_fails_closed() {
    let dir = TempDir::new("lock-unopenable");
    // A directory where the DB file should be makes every statement fail:
    // acquisition must degrade to None (skip this round), never throw.
    std::fs::create_dir(dir.path().join(common::PROFILE_LEARNING_COORDINATION_DB))
        .expect("create blocking directory");

    let lock = lock_for(dir.path());
    assert!(lock.try_acquire(None).await.is_none());
    assert!(!lock.is_held().await);
}

// ---------------------------------------------------------------------------
// Child entry (self re-exec)
// ---------------------------------------------------------------------------

struct ForcedSignal(SignalOutcome);

impl SignalProbe for ForcedSignal {
    fn signal0(&self, _pid: u32) -> SignalOutcome {
        self.0
    }
}

fn child_lock(dir: &Path) -> LearningLock {
    LearningLock::new(LearningLockOptions::new(
        dir.to_path_buf(),
        common::store_config(dir.to_path_buf()),
    ))
}

fn wait_barrier_bounded(dir: &Path, name: &str) -> bool {
    wait_for_barrier(dir, name, BARRIER_TIMEOUT)
}

/// Unreachable-by-default child entry; harmless no-op when the mode env is
/// unset. Executed through self re-exec
/// (`--exact fixture_child_entry --ignored`).
#[test]
#[ignore = "fixture subprocess entry — driven only via self re-exec"]
fn fixture_child_entry() {
    let Some(mode) = std::env::var_os(common::FIXTURE_CHILD_ENV) else {
        return;
    };
    let mode = mode.to_string_lossy().to_string();
    let Some(out) = std::env::var_os(common::FIXTURE_OUT_ENV) else {
        return;
    };
    let out = PathBuf::from(out);
    let Some(rest) = mode.strip_prefix("lock:") else {
        return;
    };
    let storage = std::env::var("MEM_FIXTURE_STORAGE").expect("MEM_FIXTURE_STORAGE");
    let dir = PathBuf::from(storage);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");

    match rest {
        "hold" => {
            let report = runtime.block_on(async {
                let lock = child_lock(&dir);
                let guard = lock.try_acquire(None).await.expect("winner acquires");
                let held_before = lock.is_held().await;
                std::fs::write(common::barrier_path(&dir, "acquired"), b"ready")
                    .expect("signal acquired");
                assert!(
                    wait_barrier_bounded(&dir, "release"),
                    "release barrier timed out"
                );
                guard.release().await;
                let held_after = lock.is_held().await;
                serde_json::json!({
                    "acquired": true,
                    "heldBefore": held_before,
                    "heldAfter": held_after,
                })
            });
            write_child_report(&out, &report);
        }
        "try" | "probe" => {
            let report = runtime.block_on(async {
                let lock = child_lock(&dir);
                let guard = lock.try_acquire(None).await;
                let acquired = guard.is_some();
                if let Some(guard) = guard {
                    guard.release().await;
                }
                serde_json::json!({ "acquired": acquired })
            });
            write_child_report(&out, &report);
        }
        "hold-forever" => {
            let guard = runtime.block_on(async {
                let lock = child_lock(&dir);
                lock.try_acquire(None).await.expect("holder acquires")
            });
            std::fs::write(common::barrier_path(&dir, "acquired"), b"ready")
                .expect("signal acquired");
            // Park until the parent SIGKILLs this process. The guard is kept
            // alive so the row is never released.
            std::mem::forget(guard);
            loop {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        cas if cas.starts_with("cas-race:") => {
            let label = cas.trim_start_matches("cas-race:").to_string();
            let report = runtime.block_on(async {
                let lock = child_lock(&dir);
                let token_dir = dir.clone();
                let seen_dir = dir.clone();
                let token_label = label.clone();
                let seen_label = label.clone();
                let hooks = mem_profile::learning_lock::AcquireHooks::new(
                    move |token: Option<&str>| {
                        std::fs::write(
                            common::barrier_path(&token_dir, &format!("token.{token_label}")),
                            token.unwrap_or("none").as_bytes(),
                        )
                        .expect("write token barrier");
                    },
                    move || {
                        std::fs::write(
                            common::barrier_path(&seen_dir, &format!("seen.{seen_label}")),
                            b"ready",
                        )
                        .expect("write seen barrier");
                        assert!(
                            wait_barrier_bounded(&seen_dir, "go.update"),
                            "go.update barrier timed out"
                        );
                    },
                );
                let guard = lock.try_acquire(Some(&hooks)).await;
                let acquired = guard.is_some();
                if let Some(guard) = guard {
                    guard.release().await;
                }
                serde_json::json!({ "acquired": acquired })
            });
            write_child_report(&out, &report);
        }
        proc_probe if proc_probe.starts_with("proc-probe:") => {
            let forced = proc_probe.trim_start_matches("proc-probe:");
            let outcome = match forced {
                "EPERM" => SignalOutcome::PermissionDenied,
                "OK" => SignalOutcome::Alive,
                "ESRCH" => SignalOutcome::NoSuchProcess,
                other => panic!("unknown forced signal outcome: {other}"),
            };
            let mut options =
                LearningLockOptions::new(dir.clone(), common::store_config(dir.clone()));
            options.signal = std::sync::Arc::new(ForcedSignal(outcome));
            let lock = LearningLock::new(options);
            let report = runtime.block_on(async {
                let guard = lock.try_acquire(None).await;
                let acquired = guard.is_some();
                if let Some(guard) = guard {
                    guard.release().await;
                }
                serde_json::json!({ "forced": forced, "acquired": acquired })
            });
            write_child_report(&out, &report);
        }
        other => panic!("unknown lock child mode: {other}"),
    }
}
