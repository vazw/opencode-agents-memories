//! Issue 41 instrument: does the retention sweep hold the engine's
//! `current_thread` executor for its whole run?
//!
//! The engine builds a single-threaded runtime (`engine.rs:1265`), so a sweep
//! that never yields would delay a queued `capture` by the whole sweep. This
//! instrument runs the real `run_retention_sweep` on a `current_thread`
//! runtime with a probe task that yields continuously, and counts the probe
//! polls that land inside the sweep window.
//!
//! **The declared claim**: the sweep yields per shard and per delete chunk
//! (`RETENTION_DELETE_CHUNK = 50`, one `yield_now` after each) and the sweep's
//! awaits park nowhere else, so a ready task is polled once per explicit yield
//! inside the sweep window — `polls_in_sweep >= SHARDS` on 36 shards, `>= 2` on
//! one fat shard. That count is the binding gate; the wall clock is printed
//! evidence and is asserted nowhere (see `GAP_CEILING_MS` for the two measured
//! reasons a wall-clock gate reds correct code on this host).
//!
//! Two fixtures carry the work claim (36 × 100 and 1 × 2 000) and **record**
//! the wall clock as evidence; two controls prove the instrument can see
//! per-slice scheduling and a blocking body; and one test pins the scheduling
//! points by count on the no-delete steady state. The live arm is a sixth test
//! and is **evidence only** — it asserts nothing about the count (it would pass
//! at `polls_in_sweep=1`); see its own doc. No fixture-level wall clock is
//! asserted — see `GAP_CEILING_MS` for the two measured reasons.
//!
//! With no `yield_now` in the loop the test **fails**, and that failure is the
//! point: the sweep then holds the `current_thread` executor for its whole run,
//! so a queued capture waits the entire sweep. The failing assertion is the
//! count — `polls_in_sweep=0`, expected 36 / 2 — and the recorded gap that goes
//! with it is the whole sweep: `max_gap 2689.16` of a 2689.1 ms debug sweep,
//! `174.33` of 174.3 in release (clean rebuild, 2026-11-18).
//!
//! `samples=` is NOT that evidence and must not be read as it: ~100 000 of the
//! samples are the probe free-spinning during the 30 ms pre-sweep window
//! (~0.3 µs a poll in release). `polls_in_sweep` is the windowed count, and it
//! equals the number of explicit `yield_now` calls exactly — 108 (36 shard + 72
//! chunk), 41 (1 + 40), 36 on a no-delete 36-shard sweep, 0 without the
//! per-shard line.
//!
//! The per-shard `yield_now` is invisible to the ratio on every fixture that
//! deletes: the chunk loop supplies a scheduling point there, so removing the
//! per-shard line leaves the worst ratio at **0.21–0.46 of the bound** (0.046
//! **of the sweep**, debug-2 R-2: `8.21 / 177.4 = 0.046` of the sweep,
//! `8.21 / 17.74 = 0.463` of the bound; review-3 re-measured `3.65 / 17.38 = 0.210`
//! and `4.17 / 9.53 = 0.437`). Its only real job is the no-delete steady state,
//! where it is the *only* scheduling point —
//! `test_a_sweep_with_nothing_to_delete_still_yields_once_per_shard` pins that
//! by counting polls, not milliseconds.
//!
//! **The control is what makes this a gate.** `test_the_instrument_reds_a_sweep_that_holds_the_executor`
//! runs the same `measure_gaps` arithmetic over a deliberately blocking body
//! and asserts **zero polls** in the window (plus a one-sided gap check that
//! load can only help). Without it, a later refactor of `measure_gaps` (a
//! trailing-sample truncation, say) can silently re-vacuous the property while
//! the other tests stay green — that is exactly the defect r3 shipped.
//!
//! The live-shape arm is opt-in and **evidence-only**:
//! `OPENCODE_MEM_GAP_STORE=<abs path to a copied storage root>` (the sweep
//! deletes rows, so it must be a copy, never the live root). It prints
//! `sweep_ms / max_gap_ms / bound_ms / deleted` and asserts only that the copy
//! was not already swept; on an unbalanced store the bound cannot bind, because
//! the granule is one shard and the live copy's largest shard is 14.8× the mean.

mod common;

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::TursoConnectFactory;
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert};

const ROWS_PER_SHARD: usize = 100;
const SHARDS: usize = 36;
const RETENTION_DAYS: u32 = 30;
/// The sweep must have done real work — `deleted == expected` on the property
/// fixtures — before the recorded gap or the poll count means anything.
const MIN_SWEEP_MS: f64 = 50.0;
/// The largest poll gap the **control harness** allows, as a fraction of the
/// sweep's own wall time. Used by the two controls and by nothing else: no
/// property fixture asserts a wall clock.
const GAP_RATIO: f64 = 0.10;
/// Where the recorded wall clock sits on a quiet host: a release 36-shard sweep
/// is ~180 ms and one shard body is 3–5 ms, so `0.10 × sweep` is ~18 ms.
///
/// **The wall clock is evidence here, never an assertion.** Two measured
/// reasons: (1) the ratio red correct code in 4 of 47 runs (3 of 7 at loadavg
/// 1.10–1.11, 1 of 20 at 3.00–5.02, 0 of 20 at 1.12–1.22) while
/// `polls_in_sweep` stayed 108/108 — one shard body is
/// CPU-bound, 18–97 ms, with `run_delay_ms = 0.00`; (2) a fixed ceiling does not
/// rescue it either — a correct run measured `max_gap 185.97` on an inflated
/// 386.1 ms sweep (`polls_in_sweep=108`) at loadavg 8.67, and review-4 measured
/// `137.80` on a 217.9 ms sweep (`polls_in_sweep=41`) at loadavg 9.16; no
/// ceiling high enough to clear those can separate the release zero-yield
/// mutant (`211.75`). What the yield contract asserts is that the sweep parks
/// between shards; that is the count.
///
/// **Rule for this file: any wall-clock assertion here must be looser than the
/// 10-run spread measured at loadavg >= 1, and any tightening must ship that
/// measurement with it.** A gate whose false-red rate was not measured is the
/// defect these constants exist to correct (README §(c) records the protocol:
/// n, per-run loadavg, min/p50/max of the gap, and the red count).
const GAP_CEILING_MS: f64 = 100.0;
/// `common::store_config`'s embedding width.
const DIMENSIONS: usize = 2;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn manager(config: &StoreConfig) -> ShardManager {
    let connections = Arc::new(ConnectionManager::new(
        config.storage_path.clone(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    ShardManager::new(
        config.storage_path.clone(),
        config.clone(),
        connections,
        Arc::new(SystemProcessProbe),
        Some(mem_core::embedding::EmbeddingIdentity {
            model: config.embedding_model.clone(),
            dimensions: config.embedding_dimensions,
            pooling: mem_core::config::EmbeddingPooling::Mean,
            dtype: None,
            use_task_prefixes: false,
            query_prefix: None,
            document_prefix: None,
            max_tokens: None,
        }),
    )
}

async fn seed(manager: &ShardManager, shards: usize, rows: usize, idle: bool) {
    // `idle` = old enough for the 30-day retention cutoff (the sweep has work);
    // `!idle` = one second old (the sweep has nothing to delete).
    let stamp = if idle {
        now_millis() - 60 * 86_400_000
    } else {
        now_millis() - 1_000
    };
    for index in 0..shards {
        let hash = format!("{:016x}", index + 1);
        let info = manager
            .create_shard(Scope::Project, &hash, 0)
            .await
            .expect("the fixture shard is created");
        let db = manager
            .open_shard(&info.db_path)
            .await
            .expect("the fixture shard opens");
        for row in 0..rows {
            vector::insert_vector(
                db.as_ref(),
                &MemoryInsert {
                    id: format!("mem-{index}-{row}"),
                    content: format!("row {index}/{row}"),
                    vector: vec![0.0; DIMENSIONS],
                    tags_vector: None,
                    container_tag: format!("opencode_project_{hash}"),
                    tags: None,
                    memory_type: None,
                    created_at: stamp,
                    updated_at: stamp,
                    metadata: None,
                    session_id: None,
                    display_name: None,
                    user_name: None,
                    user_email: None,
                    project_path: Some("/project".to_string()),
                    project_name: None,
                    git_repo_url: None,
                    source_prompt_created_at: None,
                },
            )
            .await
            .expect("the fixture row inserts");
        }
    }
}

/// What one measurement produced. `polls_in_sweep` is the fact the yield
/// contract is stated in and the only one any property fixture asserts —
/// `max_gap_ms` is the wall-clock fact, printed for the record and for the two
/// controls, asserted by no property fixture.
struct GapStats {
    sweep_ms: f64,
    max_gap_ms: f64,
    polls_in_sweep: usize,
}

/// Runs `sweep` to completion on a `current_thread` runtime with a probe task
/// yielding continuously, and returns the sweep's wall time, its largest poll
/// gap, and how many probe polls landed INSIDE the sweep window.
///
/// The probe's last sample is the interval that ENDS when `stop` is set, and it
/// is kept in `max_gap_ms`: after `sweep.await` returns, `stop.store(true)` runs
/// synchronously in this task, so the probe resumes exactly once and its final
/// push spans the wait it actually suffered. Truncating that sample would
/// discard the only evidence a fully blocking sweep leaves (r3's defect: a
/// 173 ms `std::thread::sleep` passed at 0.06 ms against a 17.3 ms bound).
///
/// That last sample is deliberately EXCLUDED from `polls_in_sweep` — it is
/// pushed after `stop` is set, i.e. after the window closed, and it is the very
/// sample the poll-count property is not about. Counting it would credit a
/// blocking sweep with a poll it did not yield for.
async fn measure_gaps<F>(sweep: F) -> GapStats
where
    F: Future<Output = ()>,
{
    let stop = Arc::new(AtomicBool::new(false));
    let gaps: Arc<Mutex<Vec<(Instant, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let probe = tokio::spawn({
        let stop = Arc::clone(&stop);
        let gaps = Arc::clone(&gaps);
        async move {
            let mut last = Instant::now();
            loop {
                tokio::task::yield_now().await;
                let now = Instant::now();
                gaps.lock()
                    .expect("the gap vector")
                    .push((now, now.duration_since(last).as_micros() as u64));
                last = now;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    let started = Instant::now();
    sweep.await;
    let ended = Instant::now();
    let sweep_ms = ended.duration_since(started).as_secs_f64() * 1000.0;
    stop.store(true, Ordering::Relaxed);
    let _ = probe.await;

    let gaps = gaps.lock().expect("the gap vector").clone();
    let max_gap_micros = gaps.iter().map(|(_, gap)| *gap).max().unwrap_or(0);
    let max_gap_ms = max_gap_micros as f64 / 1000.0;
    let polls_in_sweep = gaps
        .iter()
        .filter(|(at, _)| *at >= started && *at <= ended)
        .count();
    let mut top: Vec<u64> = gaps.iter().map(|(_, gap)| *gap).collect();
    top.sort_unstable_by(|a, b| b.cmp(a));
    let top_ms: Vec<String> = top
        .iter()
        .take(3)
        .map(|micros| format!("{:.2}", *micros as f64 / 1000.0))
        .collect();
    println!(
        "gap_instrument: samples={} polls_in_sweep={polls_in_sweep} top_gaps_ms=[{}]",
        gaps.len(),
        top_ms.join(", ")
    );
    GapStats {
        sweep_ms,
        max_gap_ms,
        polls_in_sweep,
    }
}

/// Runs one real retention sweep through `measure_gaps`.
async fn measure(manager: &ShardManager) -> (GapStats, u64) {
    let now = now_millis();
    let retention_ms = i64::from(RETENTION_DAYS) * 86_400_000;
    let cutoff = now - retention_ms;
    let mut deleted = 0u64;
    let stats = measure_gaps(async {
        let outcome = manager
            .run_retention_sweep(cutoff, retention_ms, retention_ms * 2, &HashSet::new())
            .await
            .expect("the sweep runs");
        deleted = outcome.deleted_count;
    })
    .await;
    (stats, deleted)
}

fn assert_the_bound(
    fixture: &str,
    stats: &GapStats,
    deleted: u64,
    expected: u64,
    min_polls: usize,
) {
    let GapStats {
        sweep_ms,
        max_gap_ms,
        polls_in_sweep,
    } = *stats;
    let ratio_bound_ms = sweep_ms * GAP_RATIO;
    println!(
        "gap_instrument: fixture={fixture} sweep_ms={sweep_ms:.1} max_gap_ms={max_gap_ms:.2} \
         ratio_bound_ms={ratio_bound_ms:.2} quiet_host_ceiling_ms={GAP_CEILING_MS:.0} \
         polls_in_sweep={polls_in_sweep} deleted={deleted} gate=polls>={min_polls} \
         wall_clock=evidence(not asserted)"
    );
    assert_eq!(
        deleted, expected,
        "{fixture}: the sweep deleted every idle row (the work happened)"
    );
    assert!(
        sweep_ms >= MIN_SWEEP_MS,
        "{fixture}: the fixture failed to exercise the sweep ({sweep_ms:.1} ms)"
    );
    // The structural gate: one scheduling point per shard, and per delete chunk
    // where one fires. Deterministic — it does not move with host load, unlike
    // the recorded gap above.
    assert!(
        polls_in_sweep >= min_polls,
        "{fixture}: the sweep parked a ready task: {polls_in_sweep} poll(s) in the sweep window, \
         expected at least {min_polls}"
    );
}

/// The rows in the single fat shard. Chunking at 50 makes the longest single
/// body `50/FAT_ROWS` of the sweep; an unchunked delete is one body ≈ the whole
/// sweep. This is the chunking's gate — and the gate is the **poll count**:
/// shipped 41 (1 shard yield + 40 chunk yields), the per-chunk yield removed
/// gives 1, which reds `min_polls = 2` before the wall clock is consulted. On
/// 36 × 100 the delete body is small, so removing the per-chunk yield leaves
/// that fixture green (36 polls, per-shard only) — hence a fat shard.
///
/// 1 000 rows was not enough on a release build: the whole sweep measured
/// 48.5 ms, under the 50 ms `MIN_SWEEP_MS` floor, so the fixture would have had
/// to lower a gate to pass. 2 000 rows doubles the sweep instead (~97 ms
/// release, ~2.3 s debug) and keeps the ratio where it is.
const FAT_ROWS: usize = 2000;

#[tokio::test(flavor = "current_thread")]
async fn test_a_fat_single_shard_is_deleted_in_chunks_not_one_body() {
    let config = common::store_config("retention-gap-fat");
    let manager = manager(&config);
    seed(&manager, 1, FAT_ROWS, true).await;
    let (stats, deleted) = measure(&manager).await;
    let _ = common::cleanup_temp_dir(&config.storage_path);
    assert_the_bound(
        &format!("synthetic 1x{FAT_ROWS}"),
        &stats,
        deleted,
        FAT_ROWS as u64,
        2,
    );
}

/// The 36-shard deleting sweep: `polls_in_sweep >= SHARDS` — one poll per
/// shard yield plus one per delete chunk, 108 measured on the shipped code.
#[tokio::test(flavor = "current_thread")]
async fn test_a_sweep_that_deletes_polls_once_per_shard_and_chunk() {
    let config = common::store_config("retention-gap");
    let manager = manager(&config);
    seed(&manager, SHARDS, ROWS_PER_SHARD, true).await;
    let (stats, deleted) = measure(&manager).await;
    let _ = common::cleanup_temp_dir(&config.storage_path);
    assert_the_bound(
        &format!("synthetic {SHARDS}x{ROWS_PER_SHARD}"),
        &stats,
        deleted,
        (SHARDS * ROWS_PER_SHARD) as u64,
        SHARDS,
    );
}

/// A sweep that deletes nothing is the steady state — every tick after the
/// first finds an empty idle population (measured 19.5 ms release on the live
/// store, issue-41 README §(a) run 3) — and it is the ONE shape where the
/// per-chunk `yield_now` never runs, so the per-shard one is the only
/// scheduling point on it. Measured, clean rebuild, both profiles: 36 polls
/// in the sweep window (one per shard) shipped, **0** with the per-shard
/// `yield_now` removed, on a 42.0 ms debug / 4.1 ms release sweep. No ratio
/// expresses this — on every fixture that deletes, the chunk yields mask the
/// difference (**0.21–0.46 of the bound**, worst 0.46) — so this test counts
/// scheduling points instead of wall clock.
///
/// No `MIN_SWEEP_MS` floor here: a no-op sweep is cheap by nature and raising
/// the fixture to 50 ms would mean seeding ~200 000 rows.
#[tokio::test(flavor = "current_thread")]
async fn test_a_sweep_with_nothing_to_delete_still_yields_once_per_shard() {
    let config = common::store_config("retention-gap-noop");
    let manager = manager(&config);
    seed(&manager, SHARDS, ROWS_PER_SHARD, false).await;
    let (stats, deleted) = measure(&manager).await;
    let _ = common::cleanup_temp_dir(&config.storage_path);
    assert_eq!(deleted, 0, "the fixture has nothing to delete");
    assert!(
        stats.polls_in_sweep >= SHARDS,
        "the sweep parked a ready task for its whole run: {} poll(s) in a window of \
         {} shards with nothing to delete ({:.1} ms); the per-shard `yield_now` in \
         `run_retention_sweep` must give one scheduling point per shard",
        stats.polls_in_sweep,
        SHARDS,
        stats.sweep_ms
    );
}

/// The live-shape arm: **evidence, not a gate.**
///
/// On an unbalanced store the relative bound cannot bind, by construction. The
/// granule is one shard, and the live copy is 36 shards whose largest holds
/// 2 717 of 6 590 vectors — 14.8× the mean — so one shard's `open_shard` +
/// `RETENTION_CANDIDATES_SELECT` alone must exceed `0.10 × sweep`. Measured on
/// a copy with every row pinned, so the delete loop did no work at all:
/// `max_gap 210.59` of a 688.8 ms sweep (`deleted=0`, bound 68.88) — 3.06× over,
/// with zero deletes. No delete-loop change can move that, which is why this
/// arm prints and does not assert: a red assertion behind an env var would be
/// an inverse-vacuous oracle.
#[tokio::test(flavor = "current_thread")]
async fn test_a_copy_of_the_live_store_shape_is_recorded_as_evidence() {
    let Ok(root) = std::env::var("OPENCODE_MEM_GAP_STORE") else {
        eprintln!("gap_instrument: OPENCODE_MEM_GAP_STORE unset; live-shape arm skipped");
        return;
    };
    let mut config = common::store_config("retention-gap-live");
    // The copy carries the live embedding identity, so build the config the
    // way the engine does and then point it at the copy.
    if let Ok(core) = mem_core::config::Config::load_default(&mem_core::env::ProcessEnv) {
        config = mem_store::config::from_core(&core);
        config.storage_path = std::path::PathBuf::from(&root);
    } else {
        config.storage_path = std::path::PathBuf::from(&root);
    }
    let manager = manager(&config);
    let (stats, deleted) = measure(&manager).await;
    let bound_ms = stats.sweep_ms * GAP_RATIO;
    println!(
        "gap_instrument: fixture=live-copy sweep_ms={:.1} max_gap_ms={:.2} bound_ms={bound_ms:.2} deleted={deleted} polls_in_sweep={}",
        stats.sweep_ms, stats.max_gap_ms, stats.polls_in_sweep
    );
    // The one assertion that still means something here: the copy was not
    // already swept, so the numbers above describe a real run.
    assert!(
        stats.sweep_ms >= MIN_SWEEP_MS,
        "live-copy: the copy was already swept ({:.1} ms)",
        stats.sweep_ms
    );
}

/// The shape the fix must produce: per-shard slices that yield between them.
///
/// Kept as a control so the instrument is proven able to see a fine-grained
/// yielding body. The assertion is the **count** (one poll per slice), not the
/// clock: a host stall inflates the gap for a body that yields perfectly, and
/// this control must not be able to red on that. The gap it measured is printed.
#[tokio::test(flavor = "current_thread")]
async fn test_a_slice_and_yield_body_polls_once_per_slice() {
    let stats = measure_gaps(async {
        for _ in 0..SHARDS {
            let slice = Instant::now();
            while slice.elapsed() < Duration::from_millis(5) {
                std::hint::spin_loop();
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let bound_ms = stats.sweep_ms * GAP_RATIO;
    println!(
        "gap_instrument: fixture=control-slice yield sweep_ms={:.1} max_gap_ms={:.2} ratio_bound_ms={bound_ms:.2} polls_in_sweep={} gate=polls>={SHARDS}",
        stats.sweep_ms, stats.max_gap_ms, stats.polls_in_sweep
    );
    assert!(
        stats.sweep_ms >= MIN_SWEEP_MS,
        "control: the sliced body did not run ({:.1} ms)",
        stats.sweep_ms
    );
    assert!(
        stats.polls_in_sweep >= SHARDS,
        "control: a body sliced into {SHARDS} yields was polled {} time(s) — the instrument \
         cannot see per-slice scheduling, so it cannot be a gate",
        stats.polls_in_sweep
    );
}

/// The gate on the gate: a body that holds the executor must be seen as one
/// uninterruptible wait, and must show **zero** polls in the window.
///
/// Same `measure_gaps` arithmetic as the property test, so a refactor that
/// re-vacuouses the measurement (r3 truncated the one sample that spans a
/// blocking run) reds here instead of passing silently. The `max_gap` check is
/// one-sided: host load can only inflate the gap, so it cannot break this
/// assertion — only a blind instrument can.
#[tokio::test(flavor = "current_thread")]
async fn test_the_instrument_reds_a_sweep_that_holds_the_executor() {
    let stats = measure_gaps(async { std::thread::sleep(Duration::from_millis(173)) }).await;
    let bound_ms = stats.sweep_ms * GAP_RATIO;
    println!(
        "gap_instrument: fixture=control-blocking sweep_ms={:.1} max_gap_ms={:.2} ratio_bound_ms={bound_ms:.2} polls_in_sweep={} gate=polls==0",
        stats.sweep_ms, stats.max_gap_ms, stats.polls_in_sweep
    );
    assert!(
        stats.sweep_ms >= MIN_SWEEP_MS,
        "control: the blocking body did not run ({:.1} ms)",
        stats.sweep_ms
    );
    assert_eq!(
        stats.polls_in_sweep, 0,
        "control: a fully blocking body was polled inside its own window — the window maths \
         is broken, so the count gates cannot be trusted"
    );
    assert!(
        stats.max_gap_ms > bound_ms,
        "control: a fully blocking body measured only {:.2} ms of gap against a \
         {bound_ms:.2} ms bound — the instrument cannot see the mutant it claims to kill",
        stats.max_gap_ms
    );
}
