//! issue 154 — the one-time dead-PID sweep inside `common::temp_dir`.
//!
//! One test in its own binary: `common` has no `#[test]` and no
//! static-initialiser `temp_dir()` call, so the sweep's `Once` is unconsumed
//! when the trigger call below fires it. A second test here could consume the
//! `Once` first; none may be added.
//!
//! Fixtures are captured as booleans, the `Drop` cleans every directory and
//! the held `sleep` child, and only then are the assertions made — a RED run
//! therefore leaks neither a sentinel nor a process, and the first assertion
//! to fire names the guard that broke.

#![cfg(target_os = "linux")]

mod common;

use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, SystemTime};

/// Above the sweep's 900 s rail.
const OVER_RAIL: Duration = Duration::from_secs(1200);
/// Inside the rail's (300 s, 1200 s] bracket: a rail at or below 300 s removes it.
const UNDER_RAIL: Duration = Duration::from_secs(300);

struct Fixtures {
    dirs: Vec<PathBuf>,
    child: Child,
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        for dir in &self.dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture(base: &Path, fixtures: &mut Fixtures, name: String) -> PathBuf {
    let dir = base.join(name);
    std::fs::create_dir(&dir).expect("create hygiene fixture");
    fixtures.dirs.push(dir.clone());
    dir
}

fn backdate(dir: &Path, age: Duration) {
    let file = std::fs::File::open(dir).expect("open fixture for set_times");
    let when = SystemTime::now() - age;
    file.set_times(FileTimes::new().set_modified(when))
        .expect("backdate fixture");
}

fn nanos() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

#[test]
fn test_temp_dir_sweep_removes_old_dead_pid_and_keeps_every_guarded_case() {
    if !matches!(
        mem_store::platform::ProcObservability::current(),
        mem_store::platform::ProcObservability::ProcFs
    ) {
        return; // the sweep is a deliberate no-op without /proc; nothing here is observable
    }

    let own = std::process::id();
    let pid_max: u32 = std::fs::read_to_string("/proc/sys/kernel/pid_max")
        .expect("read /proc/sys/kernel/pid_max")
        .trim()
        .parse()
        .expect("pid_max is a number");
    let dead = pid_max + 1; // one past the ceiling: the kernel never hands it out

    // Spawned before any fixture exists, so a spawn failure cannot leak one.
    let child = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn sleep");
    let live = child.id(); // a real pid of a real foreign process, never `own`
    let base = std::env::temp_dir();
    let mut fixtures = Fixtures {
        dirs: Vec::new(),
        child,
    };

    let old = fixture(
        &base,
        &mut fixtures,
        format!("mem-profile-w31-hygiene-old-{dead}-{own}-{}", nanos()),
    );
    backdate(&old, OVER_RAIL);

    let mid = fixture(
        &base,
        &mut fixtures,
        format!("mem-profile-w31-hygiene-mid-{dead}-{own}-{}", nanos()),
    );
    backdate(&mid, UNDER_RAIL);

    let fresh = fixture(
        &base,
        &mut fixtures,
        format!("mem-profile-w31-hygiene-fresh-{dead}-{own}-{}", nanos()),
    );
    // `fresh` keeps its creation mtime: the bottom edge of the age rail.

    let live_dir = fixture(
        &base,
        &mut fixtures,
        format!("mem-profile-w31-hygiene-live-{live}-{own}-{}", nanos()),
    );
    backdate(&live_dir, OVER_RAIL);

    let own_dir = fixture(
        &base,
        &mut fixtures,
        format!("mem-profile-w31-hygiene-own-{own}-{own}-{}", nanos()),
    );
    backdate(&own_dir, OVER_RAIL);

    let shape = fixture(
        &base,
        &mut fixtures,
        format!("mem-profile-w31-hygiene-shape-{dead}-{own}"),
    );
    backdate(&shape, OVER_RAIL);

    let foreign = fixture(
        &base,
        &mut fixtures,
        format!("membridge-w53-hygiene-{dead}-{own}-0"),
    );
    backdate(&foreign, OVER_RAIL);

    // The first `temp_dir` call in this binary fires the sweep.
    let trigger = common::temp_dir("hygiene-trigger");
    fixtures.dirs.push(trigger.clone());

    let old_gone = !old.exists();
    let mid_kept = mid.exists();
    let fresh_kept = fresh.exists();
    let own_kept = own_dir.exists();
    let live_kept = live_dir.exists();
    let shape_kept = shape.exists();
    let foreign_kept = foreign.exists();
    let trigger_kept = trigger.exists();

    drop(fixtures);

    assert!(old_gone, "dead pid + over-age mtime not swept");
    assert!(mid_kept, "dead pid + 300 s removed despite the rail");
    assert!(fresh_kept, "fresh dir removed");
    assert!(own_kept, "own pid's dir removed");
    assert!(live_kept, "foreign live pid's dir removed");
    assert!(shape_kept, "non-canonical name removed");
    assert!(foreign_kept, "foreign namespace removed");
    assert!(trigger_kept, "trigger dir missing");
}
