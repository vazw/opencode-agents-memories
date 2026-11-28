//! Issue 41 — THE capture-latency instrument: does a due cleanup sweep slow the
//! capture path?
//!
//! Two arms over ONE private storage root, each a real child engine driven
//! through stdio:
//!
//! * **arm A (not due)** — the `cleanup-sweep.json` stamp says the sweep just
//!   ran, so the tick is a no-op. The floor.
//! * **arm B (due)** — the same stamp aged past the 24 h interval, so the first
//!   capture's wake runs a 3600-row sweep while 39 more captures are still
//!   being answered. The effect.
//!
//! # The mutant G2 kills
//!
//! A capture that waits on the sweep itself — the sweep run inside the capture's
//! task, or the sweep monopolising the executor because its yields are gone —
//! delays exactly one response by the whole sweep, so `max(due)` exceeds
//! `max(not_due)` by far more than half a tick. That is G2, the ONLY
//! discriminating gate here.
//!
//! # Why the count-above-a-percentile diagnostic is not a gate
//!
//! The shipped form of this instrument also counted how many due samples
//! exceeded arm A's p95, and asserted that count `<= 1`. It is unsound in both
//! directions, measured 6/6 on correct code and 0/40 on the yield-removed
//! mutant:
//!
//! * arm A's `p95` is the idle-jitter ceiling, 0.3–0.9 ms wide, while the
//!   design's intended exposure is one shard slice — a measured 1.6–2.8 ms
//!   median shift, so `P(due_i > p95(not_due)) ≈ 0.6` under CORRECT code;
//! * `percentile(0.95, n = 40)` is the third-largest sample, so the single
//!   640 ms delay the mutant produces cannot count against it — and the
//!   mutant's 39 post-sweep burst-drain samples are *faster* than arm A.
//!
//! It is printed as `diag_g1_outliers` after the gates and never asserted. A
//! same-run percentile form is not the fix either: `p95(due)` is dominated by
//! the slice interleaving the gate would be certifying (`19.97–159.73 ms`
//! measured against `p95(not_due) + max(2.0, 0.25 * p95(not_due)) = 9.4–12.9`).
//! The claim "did one capture wait on the sweep?" is a max claim; G2 is its
//! shape.
//!
//! # Why `--release`
//!
//! The debug profile is ~9× slower on this store shape (2628 ms vs 280.7 ms
//! for the same sweep), which would make the absolute cap G3 meaningless. Run:
//!
//! ```sh
//! cargo test --release -p opencode-agents-memory --test cleanup_tick_latency -- --nocapture
//! ```

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::Platform;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert};

/// The fixture: 36 shards x 100 idle rows = the sweep G2 divides by.
const SHARDS: u32 = 36;
const ROWS_PER_SHARD: usize = 100;
const CAPTURES: usize = 40;
const RETENTION_DAYS: i64 = 30;
const DAY_MS: i64 = 86_400_000;

/// The anti-vacuity floor on the swept duration. Dated provenance: 0.094× the
/// 533.1 ms live-shape release sweep and 0.293× the 170.5 ms synthetic release
/// sweep, both measured 2026-11-17T19:08Z. A host that sweeps 3600 rows in
/// under 50 ms scales the fixture up, never this floor down.
const MIN_TICK_MS: u64 = 50;
const G3_MAX_DUE_MS: u64 = 150;

/// G3 is an absolute cap, so it is only meaningful on an otherwise idle host:
/// seven idle runs measured `max(not_due)` at 8.16–10.29 ms, while two loaded
/// runs measured 51.62 and 218.93 ms. 25 ms separates the two populations by
/// 2.4× / 8.8×, and a breach is host load, not cleanup.
const IDLE_HOST_MAX_NOT_DUE_MS: f64 = 25.0;

const ACK_BOUND: Duration = Duration::from_secs(120);
const TICK_BOUND: Duration = Duration::from_secs(10);
const TICK_POLL: Duration = Duration::from_millis(10);
const EXIT_BOUND: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

const PROJECT_HASH: &str = "fedcba9876543210";

fn scratch() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join("scratch")
        .join("cleanup-tick-latency")
        .join(format!("run-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("home")).expect("create the private home");
    std::fs::create_dir_all(dir.join("storage")).expect("create the storage root");
    dir
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local addr")
        .port()
}

fn runtime() -> mem_store::tokio::runtime::Runtime {
    mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the helper runtime")
}

fn write_config(home: &Path) {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("create the config dir");
    std::fs::write(
        dir.join("opencode-mem.jsonc"),
        r#"{ "autoCleanupEnabled": true, "autoCleanupInterval": "24h" }"#,
    )
    .expect("write the child config");
}

fn store_config(home: &Path, storage: &Path) -> StoreConfig {
    let env: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::from([("HOME".to_string(), home.display().to_string())]);
    let config =
        mem_core::config::Config::load(&mem_core::config::ConfigPaths::standard(home), &env)
            .expect("resolve the sandbox config");
    let mut store = mem_store::config::from_core(&config);
    store.storage_path = storage.to_path_buf();
    store
}

/// 36 project shards, 100 rows each, every row a day past the retention window.
fn seed(config: &StoreConfig) -> usize {
    runtime().block_on(async {
        let connections = std::sync::Arc::new(ConnectionManager::new(
            config.storage_path.clone(),
            config.clone(),
            Platform::current(),
            std::sync::Arc::new(TursoConnectFactory),
        ));
        let manager = ShardManager::new(
            config.storage_path.clone(),
            config.clone(),
            connections,
            std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
            None,
        );
        let updated_at = mem_profile::ids::now_millis() - (RETENTION_DAYS + 1) * DAY_MS;
        let mut total = 0;
        for index in 0..SHARDS {
            if let Some(active) = manager
                .get_active_shard(Scope::Project, PROJECT_HASH)
                .await
                .expect("read the active shard")
            {
                manager
                    .mark_shard_read_only(active.id)
                    .await
                    .expect("retire the predecessor shard");
            }
            let shard = manager
                .create_shard(Scope::Project, PROJECT_HASH, index)
                .await
                .expect("create the fixture shard");
            let db = manager
                .open_shard(&shard.db_path)
                .await
                .expect("open the fixture shard");
            for row in 0..ROWS_PER_SHARD {
                vector::insert_vector(
                    db.as_ref(),
                    &MemoryInsert {
                        id: format!("idle-{index}-{row}"),
                        content: format!("row {index}-{row}"),
                        vector: vec![0.0; 768],
                        tags_vector: None,
                        container_tag: format!("opencode_project_{PROJECT_HASH}"),
                        tags: None,
                        memory_type: None,
                        created_at: updated_at,
                        updated_at,
                        metadata: None,
                        session_id: None,
                        display_name: None,
                        user_name: None,
                        user_email: None,
                        project_path: None,
                        project_name: None,
                        git_repo_url: None,
                        source_prompt_created_at: None,
                    },
                )
                .await
                .expect("insert the fixture row");
                total += 1;
            }
        }
        total
    })
}

fn dist_dir(root: &Path) -> PathBuf {
    let dir = root.join("dist");
    std::fs::create_dir_all(&dir).expect("create the dist dir");
    std::fs::write(dir.join("index.html"), "<!doctype html>").expect("write index.html");
    dir
}

/// The sidecar the tick reads at construction.
fn write_stamp(storage: &Path, last_cleanup_sweep: i64) {
    std::fs::write(
        storage.join("cleanup-sweep.json"),
        format!(r#"{{"version":1,"last_cleanup_sweep":{last_cleanup_sweep}}}"#),
    )
    .expect("write the cleanup sweep stamp");
}

struct Engine {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    stderr_path: PathBuf,
}

impl Engine {
    fn spawn(home: &Path, storage: &Path, dist: &Path, port: u16) -> Self {
        let stderr_path = storage.join("engine.err");
        let stderr = std::fs::File::create(&stderr_path).expect("create the stderr capture file");
        let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
        command
            .arg("--mode=stdio")
            .arg(format!("--storage-root={}", storage.display()))
            .arg(format!("--web-port={port}"))
            .arg(format!("--web-dist={}", dist.display()))
            .env_clear()
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("OPENCODE_MEM_WEB_HOST", "127.0.0.1")
            .env("OPENCODE_MEM_LOG_FILE", storage.join("opencode-mem.log"))
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        let mut child = command.spawn().expect("the engine binary spawns");
        let stdin = child.stdin.take().expect("engine stdin");
        let stdout = child.stdout.take().expect("engine stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child: Some(child),
            stdin: Some(stdin),
            lines,
            stderr_path,
        }
    }

    fn request(&mut self, line: &str) -> String {
        {
            let stdin = self.stdin.as_mut().expect("engine stdin open");
            stdin.write_all(line.as_bytes()).expect("write request");
            stdin.write_all(b"\n").expect("write NDJSON terminator");
            stdin.flush().expect("flush request");
        }
        self.lines
            .recv_timeout(ACK_BOUND)
            .expect("the engine answers the request")
    }

    fn hello(&mut self) {
        let hello = self.request(
            r#"{"id":"hello-1","jsonrpc":"2.0","method":"hello","params":{"client":"opencode-mem-plugin","min_protocol":"1.0.0","protocol":"1.0.0"}}"#,
        );
        assert!(
            hello.contains(r#""protocol":"1.0.0""#),
            "the handshake answered the protocol line: {hello}"
        );
    }

    /// One timed capture: the wall clock around "write the request, read the
    /// ACK" — exactly what the wrapper waits for.
    fn timed_capture(&mut self, id: usize) -> f64 {
        let line = format!(
            r#"{{"id":"capture-{id}","jsonrpc":"2.0","method":"capture","params":{{"session_id":"ses-latency","capture_id":"019a0000-0000-7f00-8000-0000000000{id:02}","project_id":"/probe/project","agent_id":"agent-1","transcript":"fix the login bug","metadata":{{"promptId":"prompt-{id}"}}}}}}"#
        );
        let started = Instant::now();
        let response = self.request(&line);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert!(
            response.contains(r#""accepted":true"#),
            "capture {id} must be accepted, got {response}"
        );
        elapsed
    }

    fn stderr_text(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn count_stderr(&self, needle: &str) -> usize {
        self.stderr_text()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }

    fn tick_line(&self) -> Option<String> {
        self.stderr_text()
            .lines()
            .find(|line| line.contains(r#""event":"cleanup.tick""#))
            .map(str::to_string)
    }

    /// M3: the tick is a detached task and the runtime drops at EOF, so arm B
    /// reads until its line exists and only THEN closes stdin — its sweep runs
    /// against a ~336 ms capture loop, and a premature EOF would cancel it.
    fn wait_for_tick(&self, bound: Duration) -> String {
        let started = Instant::now();
        loop {
            if let Some(line) = self.tick_line() {
                return line;
            }
            assert!(
                started.elapsed() < bound,
                "the engine never wrote cleanup.tick within {bound:?}:\n{}",
                self.stderr_text()
            );
            std::thread::sleep(TICK_POLL);
        }
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let child = self.child.as_mut().expect("engine alive");
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                started.elapsed() < EXIT_BOUND,
                "the engine never exited within {EXIT_BOUND:?}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Arm A: 40 timed captures, then a clean EOF. No tick may appear.
fn run_not_due(home: &Path, storage: &Path, dist: &Path, port: u16) -> Vec<f64> {
    let mut engine = Engine::spawn(home, storage, dist, port);
    engine.hello();
    let samples: Vec<f64> = (0..CAPTURES).map(|id| engine.timed_capture(id)).collect();
    engine.close_stdin();
    engine.wait_exit();
    assert_eq!(
        engine.count_stderr(r#""event":"cleanup.tick""#),
        0,
        "arm A's stamp says 'just ran', so no tick may appear:\n{}",
        engine.stderr_text()
    );
    samples
}

/// Arm B: 40 timed captures, then read stderr to the `cleanup.tick` line BEFORE
/// the EOF that would cancel the detached tick (M3). Returns the samples and
/// the parsed tick line.
fn run_due(home: &Path, storage: &Path, dist: &Path, port: u16) -> (Vec<f64>, String) {
    let mut engine = Engine::spawn(home, storage, dist, port);
    engine.hello();
    let samples: Vec<f64> = (0..CAPTURES).map(|id| engine.timed_capture(id)).collect();
    let tick = engine.wait_for_tick(TICK_BOUND);
    engine.close_stdin();
    engine.wait_exit();
    (samples, tick)
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    assert!(!sorted.is_empty(), "a percentile needs samples");
    let rank = ((fraction * sorted.len() as f64).ceil() as usize).max(1);
    sorted[rank - 1]
}

fn summarize(arm: &str, samples: &[f64]) -> (f64, f64, f64) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN samples"));
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    let max = *sorted.last().expect("samples");
    println!("arm, n, p50, p95, max");
    println!("{arm}, {}, {p50:.2}, {p95:.2}, {max:.2}", samples.len());
    (p50, p95, max)
}

#[cfg_attr(
    debug_assertions,
    ignore = "absolute G3 cap is a release-profile claim: the debug sweep is ~10x slower (measured 4956 ms vs 493 ms); run with --release"
)]
#[test]
fn cleanup_tick_latency() {
    let root = scratch();
    let home = root.join("home");
    let storage = root.join("storage");
    write_config(&home);
    let config = store_config(&home, &storage);
    assert_eq!(
        seed(&config),
        SHARDS as usize * ROWS_PER_SHARD,
        "the fixture seeds exactly {} rows",
        SHARDS as usize * ROWS_PER_SHARD
    );
    let dist = dist_dir(&root);

    // Arm A: the stamp says "just ran", so the tick is a silent no-op.
    let now = mem_profile::ids::now_millis();
    write_stamp(&storage, now);
    let not_due = run_not_due(&home, &storage, &dist, free_port());

    // Arm B: the same root, aged one millisecond past 24 h. Arm A deleted
    // nothing, so the 3600 rows are still there for the due sweep.
    write_stamp(&storage, now - DAY_MS - 1);
    let (due, arm_b_tick) = run_due(&home, &storage, &dist, free_port());
    let tick: mem_store::serde_json::Value =
        mem_store::serde_json::from_str(&arm_b_tick).expect("the cleanup.tick line is JSON");
    let tick_duration_ms = tick["duration_ms"]
        .as_u64()
        .expect("duration_ms is a number");
    let deleted_count = tick["deleted_count"]
        .as_u64()
        .expect("deleted_count is a number");

    let (_, p95_not_due, max_not_due) = summarize("not_due", &not_due);
    let (_, _p95_due, max_due) = summarize("due", &due);

    // Printed BEFORE the gates: a red gate must still carry its numbers.
    println!(
        "tick_duration_ms={tick_duration_ms} deleted_count={deleted_count} \
         max_delay_excess_ms={:.2} g2_bound_ms={:.2}",
        max_due - max_not_due,
        0.5 * tick_duration_ms as f64
    );

    // Anti-vacuity preconditions: without these the gates below could pass on
    // a store the sweep never touched.
    assert!(
        tick_duration_ms >= MIN_TICK_MS,
        "the tick must be worth measuring: duration_ms {tick_duration_ms} < {MIN_TICK_MS}"
    );
    assert!(
        max_not_due >= 1.0,
        "arm A must have measurable latency: max {max_not_due:.2} ms"
    );
    assert_eq!(
        deleted_count,
        (SHARDS as usize * ROWS_PER_SHARD) as u64,
        "the due sweep must delete the whole idle population: {tick}"
    );

    // G2 (THE discriminating gate): a capture that waited on the sweep delays
    // one response by the whole sweep, which is >> half a tick. Scale-free.
    let excess = max_due - max_not_due;
    assert!(
        excess <= 0.5 * tick_duration_ms as f64,
        "G2: max(due) - max(not_due) = {excess:.2} ms exceeds half the {tick_duration_ms} ms tick — \
         a capture waited on the sweep"
    );

    // G3's absolute cap needs a host that is not busy: a loaded host reds it on
    // correct code (measured 207–219 ms of arm A latency under loadavg ~9).
    assert!(
        max_not_due <= IDLE_HOST_MAX_NOT_DUE_MS,
        "G3 is an absolute {G3_MAX_DUE_MS} ms cap and needs an otherwise idle host: arm A's max \
         was {max_not_due:.2} ms (idle runs measure 8.2–10.3 ms). Host load, not cleanup, is the \
         cause; re-run on an idle host. loadavg: {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );

    // G3 (absolute cap): a sweep that monopolizes the executor surfaces here
    // whenever it overlaps the capture burst.
    assert!(
        max_due <= G3_MAX_DUE_MS as f64,
        "G3: max(due) = {max_due:.2} ms exceeds the {G3_MAX_DUE_MS} ms cap"
    );

    // Informational only — see the module doc's "Why the count-above-a-percentile
    // diagnostic is not a gate". Printed after the gates so a red is always the
    // gate that means something.
    let outliers = due.iter().filter(|sample| **sample > p95_not_due).count();
    println!(
        "diag_g1_outliers={outliers}/{} p95_not_due_ms={p95_not_due:.2}",
        due.len()
    );
}
