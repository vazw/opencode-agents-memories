//! Native Linux-only battery for the open-fd arm of the maintenance fence
//! (`maintenance_reservation::acquire` → `live_process_holds_fd_under`).
//!
//! Native test with no upstream counterpart, so this file takes no disposition
//! row in the parity matrix (`docs/agents/testing.md`).
//!
//! Fixtures are real processes: this same test binary re-executed through
//! `current_exe()` (the `open_guard.rs:127-337` precedent), with the child's
//! `/proc/<pid>/fd` made unreadable by a real `prctl(PR_SET_DUMPABLE, 0)`.
//! `comm` is set by exec'ing a symlink named after the wanted `comm` value:
//! `comm` is the `execve`-path basename and `argv[0]` does not set it.

#![cfg(target_os = "linux")]

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mem_store::maintenance_reservation::{AcquiredReservation, ReservationError, acquire};

/// Serialises the whole file: `/proc` is global, so one live nondumpable child
/// makes every other expect-`Ok` fixture refuse.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

/// How long any single handshake step may take.
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(60);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fixture_lock() -> std::sync::MutexGuard<'static, ()> {
    FIXTURE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// A fresh fixture directory under `<project-root>/.tmp/scratch/` (never the
/// production storage root), with a 0755 `data/` root so `check_root_permissions`
/// passes independently of the process umask.
fn fixture_base(prefix: &str) -> PathBuf {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".tmp")
        .join("scratch");
    std::fs::create_dir_all(&scratch).expect("create .tmp/scratch");
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let base = scratch.join(format!(
        "fd-holders-{prefix}-{}-{counter}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(base.join("data")).expect("create the fixture storage root");
    set_mode(&base, 0o755);
    set_mode(&base.join("data"), 0o755);
    // Canonical, so the lexical path handed to the child and to `acquire` is
    // the kernel-visible one even when `.tmp/scratch` is reached via a symlink.
    std::fs::canonicalize(&base).expect("canonical fixture base")
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("set fixture permissions");
}

/// A child process whose `/proc/<pid>/fd` posture the parent controls. The
/// handshake is the house two-line pipe protocol (`open_guard.rs` precedent):
/// the parent waits for `HOLDING <pid> fd=<n|none>` on stdout and releases the
/// child over stdin. No sleeps, no polling, no ready/release files.
///
/// Two hard rules, both from `open_guard.rs`: libtest writes its
/// `test <name> ... ` prefix WITHOUT a trailing newline under `--nocapture`, so
/// the marker is matched inside the line, never at its start; and every read is
/// bounded by [`FIXTURE_TIMEOUT`], with `Drop` killing + reaping the child on
/// every path (the re-exec'd child inherits the parent's `.tmp/build.lock`
/// descriptor, so a leaked fixture would hold the cross-wave cargo serialiser).
struct FdFixture {
    base: PathBuf,
    pid: u32,
    /// The descriptor number the child announced (`None` for `fd=none`).
    holding_fd: Option<i32>,
    child: Option<std::process::Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl FdFixture {
    /// Spawns the fixture child in `mode`. `comm_name` is `None` (exec
    /// `current_exe()` directly) or `Some(name)` (exec a symlink named `name`
    /// pointing at `current_exe()` — the only way to set `comm`).
    fn spawn(base: &Path, root: &Path, mode: &str, comm_name: Option<&str>) -> Self {
        let executable = std::env::current_exe().expect("the running test binary's path");
        let program = match comm_name {
            Some(name) => {
                let exec_dir = base.join("exec");
                std::fs::create_dir_all(&exec_dir).expect("create the fixture exec dir");
                let link = exec_dir.join(name);
                std::os::unix::fs::symlink(&executable, &link)
                    .expect("symlink the fixture child's comm name");
                link
            }
            None => executable,
        };
        let mut child = std::process::Command::new(program)
            .arg("--exact")
            .arg("fixture_child_entry")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("MEM_FD_FIXTURE_CHILD", mode)
            .env("MEM_FD_FIXTURE_ROOT", root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the fd-holder fixture child");
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped child stdout");
        // A reader thread turns the pipe into a bounded channel, so no read can
        // block the test thread indefinitely.
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut fixture = Self {
            base: base.to_path_buf(),
            pid,
            holding_fd: None,
            child: Some(child),
            stdin,
            lines,
        };
        // Every fixture child prints the readiness handshake only AFTER its
        // optional `File::open` and its `PR_SET_DUMPABLE(0)`, so once this
        // returns the child's `/proc` posture is settled and the caller's
        // preconditions observe the arm under test.
        let holding_fd = fixture.expect_holding();
        fixture.holding_fd = holding_fd;
        fixture
    }

    /// The descriptor number the child announced (`None` for `fd=none`).
    fn holding_fd(&self) -> Option<i32> {
        self.holding_fd
    }

    /// Blocks (bounded) until a line containing `marker` arrives, then returns
    /// the text after it. Panics — which kills the child via `Drop` — on
    /// timeout or EOF, naming everything the child did print.
    fn expect_line(&self, marker: &str) -> String {
        let mut seen: Vec<String> = Vec::new();
        let deadline = Instant::now() + FIXTURE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some((_, rest)) = line.split_once(marker) {
                        return rest.trim().to_string();
                    }
                    seen.push(line);
                }
                Err(error) => panic!(
                    "the fd-holder child did not print `{marker}` within {FIXTURE_TIMEOUT:?} \
                     ({error}); output so far: {seen:?}"
                ),
            }
        }
    }

    /// Consumes the `HOLDING <pid> fd=<n|none>` line and returns the descriptor
    /// number (`None` when the child holds nothing).
    fn expect_holding(&self) -> Option<i32> {
        let rest = self.expect_line("HOLDING ");
        let mut fields = rest.split_whitespace();
        let announced: u32 = fields
            .next()
            .and_then(|field| field.parse().ok())
            .unwrap_or_else(|| panic!("`HOLDING` must be followed by a pid, got {rest:?}"));
        assert_eq!(
            announced, self.pid,
            "the announced pid must be this child's, so the line is provably its own: {rest:?}"
        );
        let fd_field = fields
            .next()
            .unwrap_or_else(|| panic!("`HOLDING` must carry `fd=<n|none>`, got {rest:?}"));
        if fd_field == "fd=none" {
            return None;
        }
        Some(
            fd_field
                .strip_prefix("fd=")
                .and_then(|number| number.parse().ok())
                .unwrap_or_else(|| panic!("`HOLDING` must carry `fd=<n|none>`, got {rest:?}")),
        )
    }

    /// Consumes the `TARGET <readlink>` line a hold fixture prints (the parent
    /// cannot read `fd/` under `dumpable=0`).
    fn expect_target(&self) -> PathBuf {
        PathBuf::from(self.expect_line("TARGET "))
    }

    /// Tells the child to drop its handles and exit. Bounded; `Drop` kills and
    /// reaps on the way out either way.
    fn release(mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"release\n");
            let _ = stdin.flush();
        }
        self.expect_line("RELEASED");
    }
}

impl Drop for FdFixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// How the fixture's `/proc/<pid>/comm` must relate to the string under test.
enum ExpectedComm {
    Exact(&'static str),
    Not(&'static str),
}

/// Three preconditions before any `acquire` call, so no fixture can pass for
/// the wrong reason: the child runs as this euid, its `comm` matches the
/// expectation, and its `/proc/<pid>/fd` is readable or not exactly as the arm
/// under test requires. A host whose kernel semantics differ fails here,
/// loudly, naming what it saw.
fn assert_fixture_preconditions(pid: u32, expected_comm: ExpectedComm, fd_readable: bool) {
    let uid = effective_uid();
    assert_eq!(
        status_effective_uid(pid),
        uid,
        "fixture pid {pid} must run as this euid ({uid})"
    );
    let comm = read_comm(pid);
    match expected_comm {
        ExpectedComm::Exact(expected) => assert_eq!(
            comm, expected,
            "fixture pid {pid} must carry comm={expected:?}"
        ),
        ExpectedComm::Not(unexpected) => assert_ne!(
            comm, unexpected,
            "fixture pid {pid} must not carry comm={unexpected:?}"
        ),
    }
    let observed = std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok();
    assert_eq!(
        observed, fd_readable,
        "fixture pid {pid}: /proc/<pid>/fd readability must be {fd_readable} for the arm under \
         test (observed {observed}, comm={comm:?})"
    );
}

fn status_effective_uid(pid: u32) -> u32 {
    let path = format!("/proc/{pid}/status");
    let status =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .unwrap_or_else(|| panic!("{path} has no Uid: line"));
    line.split_whitespace()
        .nth(2)
        .and_then(|field| field.parse().ok())
        .unwrap_or_else(|| panic!("{path} has no parseable effective uid: {line:?}"))
}

fn read_comm(pid: u32) -> String {
    let path = format!("/proc/{pid}/comm");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {path}: {error}"))
        .trim_end_matches('\n')
        .to_string()
}

/// Polls `/proc/<pid>/stat` (5 ms cadence, 5 s deadline) until the process
/// state char is `want`; panics naming the last observed state on timeout.
fn wait_for_state(pid: u32, want: char) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while Instant::now() < deadline {
        if let Some(state) = process_state(pid) {
            if state == want {
                return;
            }
            last = Some(state);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("pid {pid} never reached state {want} within 5 s; last observed {last:?}");
}

/// The kernel task-state char from `/proc/<pid>/stat`, parsed after the LAST
/// `)` so a `comm` containing `)` cannot shift the fields. `None` when the
/// process is gone or the stat line is malformed.
fn process_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().next()?.chars().next()
}

/// Every same-uid process whose `/proc/<pid>/fd` is unreadable, as
/// `(pid, comm, state)`. Time-varying by nature: a property, never a count.
fn unreadable_same_uid_census() -> Vec<(u32, String, char)> {
    let uid_text = effective_uid().to_string();
    let mut census = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return census;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid_text) = name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            continue;
        };
        let same_uid = status.lines().any(|line| {
            let Some((key, value)) = line.split_once(':') else {
                return false;
            };
            key == "Uid"
                && value
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|field| field == uid_text)
        });
        if !same_uid {
            continue;
        }
        if std::fs::read_dir(format!("/proc/{pid}/fd")).is_err() {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap_or_default()
                .trim_end_matches('\n')
                .to_string();
            let state = process_state(pid).unwrap_or('?');
            census.push((pid, comm, state));
        }
    }
    census.sort();
    census
}

/// The precondition both expect-`Ok` tests share: every same-uid process with
/// an unreadable `fd/` is exempt — `comm=sshd-session` (the measured name
/// heuristic) or state `Z` (the provably holder-free class the arm skips: a
/// census zombie no longer confounds an expect-`Ok` verdict) — so the
/// exemptions cover the whole host and an `Ok` is attributable to the code
/// under test. On violation the full census is printed — a host artifact is
/// never mistaken for a regression.
fn assert_census_is_exempt_only() {
    let census = unreadable_same_uid_census();
    let foreign: Vec<&(u32, String, char)> = census
        .iter()
        .filter(|(_, comm, state)| comm != "sshd-session" && *state != 'Z')
        .collect();
    assert!(
        foreign.is_empty(),
        "every same-uid process with an unreadable /proc/<pid>/fd must be exempt \
         (comm=sshd-session or state Z); census (pid, comm, state) = {census:?}, \
         foreign = {foreign:?}"
    );
    println!("unreadable same-uid census (pid, comm, state) = {census:?}");
}

/// `Err(ActiveWriter)` exactly — variant equality, not `is_err()`.
fn assert_active_writer(outcome: Result<AcquiredReservation, ReservationError>, context: &str) {
    match outcome {
        Err(ReservationError::ActiveWriter) => {}
        Err(other) => {
            panic!("{context}: expected ReservationError::ActiveWriter, got Err({other:?})")
        }
        Ok(reservation) => panic!(
            "{context}: expected ReservationError::ActiveWriter, got Ok({})",
            reservation.path().display()
        ),
    }
}

// ---------------------------------------------------------------------------
// T3 — RED-first: the exemption itself
// ---------------------------------------------------------------------------

/// An unreadable `fd/` on a process whose `comm` is exactly `sshd-session` is
/// skipped, and the acquire mints the reservation at the parent of the root.
#[test]
fn test_unreadable_fd_dir_of_sshd_session_process_is_exempt() {
    let _serial = fixture_lock();
    let base = fixture_base("t3");
    let root = base.join("data");
    let fixture = FdFixture::spawn(&base, &root, "hold-nondumpable", Some("sshd-session"));
    assert_fixture_preconditions(fixture.pid, ExpectedComm::Exact("sshd-session"), false);
    assert_eq!(
        fixture.holding_fd(),
        None,
        "the sshd-session fixture holds no descriptor"
    );
    assert_census_is_exempt_only();

    let reservation = match acquire(&root, &root) {
        Ok(reservation) => reservation,
        Err(error) => {
            panic!("an unreadable fd/ under comm=sshd-session must be exempt, got Err({error:?})")
        }
    };
    let path = reservation.path().to_path_buf();
    println!("T3 minted reservation at {}", path.display());
    assert_eq!(path, base.join(".opencode-mem-maintenance.lock"));
    assert!(
        path.exists(),
        "the reservation file must exist at {}",
        path.display()
    );

    fixture.release();
}

// ---------------------------------------------------------------------------
// T1/T2 — the posture, pinned (characterization: green on the shipped code)
// ---------------------------------------------------------------------------

/// An unreadable `fd/` on a non-`sshd-session` process still refuses, even
/// though it holds nothing.
#[test]
fn test_unreadable_fd_dir_of_non_sshd_process_refuses() {
    let _serial = fixture_lock();
    let base = fixture_base("t1");
    let root = base.join("data");
    let fixture = FdFixture::spawn(&base, &root, "hold-nondumpable", None);
    assert_fixture_preconditions(fixture.pid, ExpectedComm::Not("sshd-session"), false);
    assert_eq!(fixture.holding_fd(), None);
    // Bound-direction discriminator: while the live nondumpable child is up,
    // the census must list it as a live, non-exempt entry — proving the
    // exemption stays bounded to state `Z` and does not swallow this class.
    let census = unreadable_same_uid_census();
    assert!(
        census
            .iter()
            .any(|(pid, _, state)| *pid == fixture.pid && *state != 'Z'),
        "T1's live nondumpable child (pid {}) must be listed as a live, non-exempt \
         census entry; census (pid, comm, state) = {census:?}",
        fixture.pid
    );

    assert_active_writer(
        acquire(&root, &root),
        "an unreadable fd/ on a non-sshd-session process must refuse",
    );
    fixture.release();
}

/// An unreadable `fd/` on a non-`sshd-session` process that DOES hold a
/// descriptor under the root still refuses. The parent cannot read `fd/` under
/// `dumpable=0`, so the child self-reads `readlink("/proc/self/fd/<n>")` and
/// prints it; the parent asserts it resolves under the fixture root.
#[test]
fn test_unreadable_fd_dir_of_non_sshd_process_holding_root_fd_refuses() {
    let _serial = fixture_lock();
    let base = fixture_base("t2");
    let root = base.join("data");
    std::fs::write(root.join("hold"), b"fixture hold").expect("create the held file");
    let fixture = FdFixture::spawn(&base, &root, "hold-nondumpable", None);
    assert_fixture_preconditions(fixture.pid, ExpectedComm::Not("sshd-session"), false);
    let fd = fixture
        .holding_fd()
        .expect("the hold fixture must report a descriptor");
    println!("T2 child pid={} fd={fd}", fixture.pid);
    let target = fixture.expect_target();
    println!("T2 TARGET {}", target.display());
    assert!(
        target.starts_with(&root),
        "the held descriptor must resolve under the fixture root {}: {}",
        root.display(),
        target.display()
    );

    assert_active_writer(
        acquire(&root, &root),
        "an unreadable fd/ holding the root must refuse",
    );
    fixture.release();
}

// ---------------------------------------------------------------------------
// T4 — the near-miss name
// ---------------------------------------------------------------------------

/// `comm=sshd` (the root-owned listener's name) is NOT the exempt class: only
/// the exact `sshd-session` match may skip the refusal.
#[test]
fn test_unreadable_fd_dir_of_sshd_listener_process_refuses() {
    let _serial = fixture_lock();
    let base = fixture_base("t4");
    let root = base.join("data");
    let fixture = FdFixture::spawn(&base, &root, "hold-nondumpable", Some("sshd"));
    assert_fixture_preconditions(fixture.pid, ExpectedComm::Exact("sshd"), false);
    assert_eq!(fixture.holding_fd(), None);

    assert_active_writer(acquire(&root, &root), "comm=sshd is not the exempt class");
    fixture.release();
}

// ---------------------------------------------------------------------------
// T5/T6 — the readable branch, pinned
// ---------------------------------------------------------------------------

/// A readable `fd/` on a process holding a descriptor under the root refuses;
/// the parent resolves `/proc/<pid>/fd/<n>` itself and asserts it starts with
/// the canonical root.
#[test]
fn test_readable_fd_dir_of_process_holding_root_fd_refuses() {
    let _serial = fixture_lock();
    let base = fixture_base("t5");
    let root = base.join("data");
    std::fs::write(root.join("hold"), b"fixture hold").expect("create the held file");
    let fixture = FdFixture::spawn(&base, &root, "hold-dumpable", None);
    assert_fixture_preconditions(fixture.pid, ExpectedComm::Not("sshd-session"), true);
    let fd = fixture
        .holding_fd()
        .expect("the hold fixture must report a descriptor");
    let link = format!("/proc/{}/fd/{fd}", fixture.pid);
    let resolved = std::fs::read_link(&link).unwrap_or_else(|error| panic!("read {link}: {error}"));
    println!(
        "T5 child pid={} fd={fd} resolved {}",
        fixture.pid,
        resolved.display()
    );
    assert!(
        resolved.starts_with(&root),
        "the descriptor must resolve under {}: {}",
        root.display(),
        resolved.display()
    );

    assert_active_writer(
        acquire(&root, &root),
        "a readable fd/ holding the root must refuse",
    );
    fixture.release();
}

/// Positive control for the readable branch: a dumpable process that holds
/// nothing is not a refusal. `hold-dumpable` with no `<root>/hold` file yields
/// exactly that child (`fd=none`).
#[test]
fn test_readable_fd_dir_of_non_holding_process_is_not_a_refusal() {
    let _serial = fixture_lock();
    let base = fixture_base("t6");
    let root = base.join("data");
    let fixture = FdFixture::spawn(&base, &root, "hold-dumpable", None);
    assert_fixture_preconditions(fixture.pid, ExpectedComm::Not("sshd-session"), true);
    assert_eq!(
        fixture.holding_fd(),
        None,
        "the idle fixture holds no descriptor"
    );
    assert_census_is_exempt_only();

    let reservation = match acquire(&root, &root) {
        Ok(reservation) => reservation,
        Err(error) => {
            panic!("a readable fd/ that holds nothing must not refuse, got Err({error:?})")
        }
    };
    assert!(
        reservation.path().exists(),
        "the reservation file must exist at {}",
        reservation.path().display()
    );

    fixture.release();
}

// ---------------------------------------------------------------------------
// T7 — RED-first: the zombie skip
// ---------------------------------------------------------------------------

/// An unreaped zombie (state `Z`) whose `fd/` answers EACCES holds no
/// descriptors — its descriptor table was destroyed at exit — so it must not
/// fail the arm closed. The child is never reaped until this fixture's `Drop`
/// (kill + `wait()`), so `acquire` scans it as a live, same-uid zombie.
#[test]
fn test_zombie_child_does_not_refuse_acquire() {
    let _serial = fixture_lock();
    let base = fixture_base("zombie");
    let root = base.join("data");
    let mut fixture = FdFixture::spawn(&base, &root, "hold-dumpable", None);
    // Preconditions BEFORE any acquire: same euid, not the sshd-session name.
    assert_eq!(
        status_effective_uid(fixture.pid),
        effective_uid(),
        "fixture pid {} must run as this euid",
        fixture.pid
    );
    assert_ne!(
        read_comm(fixture.pid),
        "sshd-session",
        "fixture pid {} must not carry comm=sshd-session",
        fixture.pid
    );
    // EOF on stdin: the child exits WITHOUT a release write and stays
    // unreaped (`Child` does not reap on `Drop`), i.e. a real zombie.
    drop(fixture.stdin.take());
    wait_for_state(fixture.pid, 'Z');
    // The measured zombie signature: `fd/` answers EACCES exactly. Any other
    // errno (e.g. ENOENT) is a fixture bug, not the arm under test.
    let error = std::fs::read_dir(format!("/proc/{}/fd", fixture.pid))
        .expect_err("a zombie's /proc/<pid>/fd must be unreadable");
    assert_eq!(
        error.raw_os_error(),
        Some(13),
        "the zombie's fd/ must fail with EACCES (the measured signature), got {error:?}"
    );
    assert_eq!(
        status_effective_uid(fixture.pid),
        effective_uid(),
        "the zombie must still run as this euid"
    );
    // Exempt-direction discriminator: with the zombie unreaped, the census
    // must list no non-exempt entry.
    assert_census_is_exempt_only();

    let reservation = match acquire(&root, &root) {
        Ok(reservation) => reservation,
        Err(error) => panic!("an unreaped zombie must not fail the arm closed, got Err({error:?})"),
    };
    let path = reservation.path().to_path_buf();
    assert_eq!(
        path,
        base.join(".opencode-mem-maintenance.lock"),
        "the reservation must be minted at the parent of the root"
    );

    // Reap the zombie BEFORE `FIXTURE_LOCK` release: `Drop` kills + `wait()`s,
    // and `wait()` on a zombie reaps it.
    drop(fixture);
}

// ---------------------------------------------------------------------------
// The re-exec'd fixture child
// ---------------------------------------------------------------------------

/// The fixture child dispatcher. One test binary serves every fixture mode; the
/// parent selects the mode with `MEM_FD_FIXTURE_CHILD`.
#[test]
#[ignore = "re-exec'd fd-holder fixture child"]
fn fixture_child_entry() {
    let root = std::env::var_os("MEM_FD_FIXTURE_ROOT")
        .map(PathBuf::from)
        .expect("MEM_FD_FIXTURE_ROOT");
    match std::env::var("MEM_FD_FIXTURE_CHILD")
        .unwrap_or_default()
        .as_str()
    {
        "hold-nondumpable" => fd_holder_child(&root, true, true),
        "idle-nondumpable" => fd_holder_child(&root, false, true),
        "hold-dumpable" => fd_holder_child(&root, true, false),
        _ => {}
    }
}

/// Opens the optional `<root>/hold`, optionally drops to `PR_SET_DUMPABLE=0`,
/// prints the handshake, and blocks on stdin until released.
fn fd_holder_child(root: &Path, try_hold: bool, nondumpable: bool) {
    use std::os::fd::AsRawFd;

    let held = if try_hold {
        std::fs::File::open(root.join("hold")).ok()
    } else {
        None
    };
    if nondumpable {
        let result = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
        assert_eq!(result, 0, "PR_SET_DUMPABLE(0) must succeed");
    }
    match held.as_ref() {
        Some(file) => {
            let fd = file.as_raw_fd();
            let link = format!("/proc/self/fd/{fd}");
            let target =
                std::fs::read_link(&link).unwrap_or_else(|error| panic!("read {link}: {error}"));
            let expected = std::fs::canonicalize(root.join("hold")).expect("canonical hold path");
            assert_eq!(
                target, expected,
                "the held descriptor must resolve to the canonical hold path"
            );
            println!("HOLDING {} fd={fd}", std::process::id());
            println!("TARGET {}", target.display());
        }
        None => println!("HOLDING {} fd=none", std::process::id()),
    }
    let _ = std::io::stdout().flush();
    let mut release = String::new();
    let _ = std::io::stdin().lock().read_line(&mut release);
    drop(held);
    println!("RELEASED");
    let _ = std::io::stdout().flush();
}
