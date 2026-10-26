//! Step 1 of issue 54b — the near-duplicate scan's cost ladder.
//!
//! `#[ignore]`d on purpose: it prints a measurement table and asserts nothing
//! about timing, so `cargo test --workspace` never pays for it. Run it with
//!
//! ```text
//! cargo test -p mem-web --test dedup_scan_measure -- --ignored --nocapture
//! ```
//!
//! What it measures, over the **real** `engine::near_groups` kernel and real
//! `DedupMemory` rows (no mock, no fake clock):
//!
//! 1. the vector width, re-derived from config resolution rather than assumed;
//! 2. the cost ladder `n ∈ {100, 1_000, 5_000, 10_000, 25_000}` (plus
//!    `n = 50_000` only if the 25 000 rung finished inside five minutes),
//!    with comparisons `n(n−1)/2`, wall clock, `ns` per comparison and the
//!    fitted log-log exponent against the previous rung;
//! 3. the two skip paths the budget must cover, at `n = 5_000` — a tag-reject
//!    fixture (no cosine runs) and a claimed-set fixture (`processed` grows);
//! 4. `engine::exact_plan` at every rung, to price the linear leg beside the
//!    quadratic one;
//! 5. the recommended `n` for the centrepiece test (AC2), which has two jobs:
//!    `n(n−1)/2 < MAX_NEAR_SCAN_PAIRS` and at least ~0.1 s of scan **in this
//!    profile**;
//! 6. the machine (`lscpu` model line) and the compiler (`rustc -V`), so the
//!    numbers are attributable.
//!
//! ## The profile matters, and it is debug
//!
//! `cargo test` without `--release` is a debug build, which is the profile the
//! default suite (and therefore AC2's 0.1 s sizing) actually runs in. The
//! `MAX_NEAR_SCAN_PAIRS` value derived from this ladder is therefore
//! **conservative against a release build**: release is faster, so the same
//! comparison budget buys less wall time there. The record must say so; the
//! constant is not "derived at production speed".
//!
//! ## Why the ladder's vectors are one-hot
//!
//! The point of the ladder is the **cosine** cost, so the fixture must make
//! the kernel compute a cosine for essentially every ordered pair. One-hot
//! vectors (lane `index mod D` set to `1.0`) do that and keep it that way as
//! `n` grows:
//!
//! - two distinct lanes ⇒ cosine exactly `0.0`, below the `0.9` threshold;
//! - a repeated lane (only once `n > D`) ⇒ cosine exactly `1.0`, excluded by
//!   the kernel's `similarity < 1.0` rule.
//!
//! Either way **no candidate is ever claimed**, so
//! `engine::near_groups`'s `processed` set never grows, no later survivor is
//! skipped at its outer-loop check, and the scan is exactly
//! `n(n−1)/2` cosine computations. A fixture whose pairs land **inside**
//! `[0.9, 1.0)` would do the opposite: the first representative claims every
//! later row, every later representative is then skipped, and the ladder
//! would measure `n−1` cosines instead of `n(n−1)/2` — which is exactly the
//! R1 validity-gate trap (a measurement "too fast to be measuring cosines").
//! Content is distinct throughout, so the exact pass deletes nothing.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use mem_core::config::{DEFAULTS, embedding_dimensions_for_model};
use mem_web::api::deduplication::DedupMemory;
use mem_web::api::deduplication::engine::{NearScanLimits, exact_plan, near_groups};

/// The ladder the measurement takes. `50_000` is appended at run time only if
/// the `25_000` rung finished inside [`FIFTY_K_GATE`].
const RUNGS: [usize; 5] = [100, 1_000, 5_000, 10_000, 25_000];

/// The largest rung the ladder always takes; its wall clock gates `50_000`.
const LARGEST_MANDATORY_RUNG: usize = 25_000;

/// `50_000` is timed only when the `25_000` rung finished inside this.
const FIFTY_K_GATE: f64 = 300.0;

/// The single `container_tag` of the ladder fixtures (tag rejections must not
/// silently remove cosine work).
const TAG: &str = "tag_measure";

/// `T_SCAN_MAX` from the blueprint: the wall time one pass's near scan may
/// spend, and therefore the numerator of the cap derivation.
const T_SCAN_MAX_SECONDS: f64 = 10.0;

/// The per-shard pair grid at `max_vectors_per_shard = 50_000`.
const ONE_SHARD_GRID: f64 = 1_249_975_000.0;

/// The cap's ceiling: one shard's grid rounded up.
const CAP_CEILING: f64 = 1_250_000_000.0;

/// The cap's floor: below this a bound is not worth calling a bound.
const CAP_FLOOR: f64 = 250_000.0;

/// The R2/R3 boundary: the request timeout the scan can starve.
const REQUEST_TIMEOUT_SECONDS: f64 = 330.0;

/// One rung's observation.
struct Rung {
    n: usize,
    comparisons: u64,
    seconds: f64,
}

/// A deterministic f32 in `[-1.0, 1.0)` from a 64-bit LCG (no dependency, no
/// `rand`).
fn lcg_unit(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let high = (*state >> 33) as u32;
    (high as f32 / (u32::MAX as f32 + 2.0)) * 2.0 - 1.0
}

/// `n` rows with distinct ids and distinct contents, one container tag, and
/// one-hot vectors — see the module doc for why one-hot is the *valid*
/// fixture here.
fn one_hot_rows(n: usize, dimensions: usize) -> Vec<DedupMemory> {
    (0..n)
        .map(|index| {
            let mut vector = vec![0.0_f32; dimensions];
            vector[index % dimensions] = 1.0;
            DedupMemory {
                id: format!("mem_measure_{index}"),
                content: format!("measurement content {index}"),
                container_tag: TAG.to_string(),
                created_at: index as i64,
                vector: Some(vector),
            }
        })
        .collect()
}

/// `n` rows whose vectors all sit at roughly `cos = 0.999` to each other
/// (jittered around a shared unit direction), so the first representative
/// claims every later row and the `processed` set grows — the path whose
/// `Vec<&str>` membership scan is unbounded per pair today.
fn claimed_set_rows(n: usize, dimensions: usize) -> Vec<DedupMemory> {
    let mut state = 0x5eed_1234_u64;
    let base = 1.0_f32 / (dimensions as f32).sqrt();
    (0..n)
        .map(|index| {
            let vector = (0..dimensions)
                .map(|_| base + lcg_unit(&mut state) * 5.0e-4)
                .collect::<Vec<f32>>();
            DedupMemory {
                id: format!("mem_claimed_{index}"),
                content: format!("claimed content {index}"),
                container_tag: TAG.to_string(),
                created_at: index as i64,
                vector: Some(vector),
            }
        })
        .collect()
}

/// `n` rows whose container tags are **all distinct**, so every comparison is a
/// tag rejection and no cosine runs. Vectors are present and distinct-tagged
/// rows share nothing, so the absence and tag skips are what is measured.
fn tag_reject_rows(n: usize, dimensions: usize) -> Vec<DedupMemory> {
    let mut state = 0xfeed_9876_u64;
    (0..n)
        .map(|index| {
            let vector = (0..dimensions)
                .map(|_| lcg_unit(&mut state))
                .collect::<Vec<f32>>();
            DedupMemory {
                id: format!("mem_tag_{index}"),
                content: format!("tag content {index}"),
                container_tag: format!("tag_{index}"),
                created_at: index as i64,
                vector: Some(vector),
            }
        })
        .collect()
}

/// Times one `near_groups` run (plus its `exact_plan`, which the real caller
/// computes first) and returns the scan's wall clock and the plan's.
///
/// `claimed_groups` is the fixture-validity check: the ladder and tag-reject
/// fixtures must claim nothing (so the scan is exactly `n(n−1)/2` cosine
/// computations), while the claimed-set fixture must claim every later row
/// (one group with `n−1` duplicates). A wrong count means the fixture, not the
/// kernel, is being timed — print-only measurements still need a valid fixture.
fn time_scan(rows: &[DedupMemory], threshold: f64, claimed_groups: ClaimedGroups) -> (f64, f64) {
    let plan_start = Instant::now();
    let plan = exact_plan(rows);
    let exact_plan_seconds = plan_start.elapsed().as_secs_f64();

    let scan_start = Instant::now();
    // The ladder times the *unbounded* kernel: `u64::MAX` makes the budget
    // unreachable and the cancel flag is never set, so the measured cost is
    // the pre-bound cost the cap is derived from.
    let scan = near_groups(
        rows,
        &plan.survivors,
        threshold,
        &NearScanLimits {
            max_pairs: u64::MAX,
            cancel: Arc::new(AtomicBool::new(false)),
        },
    );
    let seconds = scan_start.elapsed().as_secs_f64();
    let groups = scan.groups;

    match claimed_groups {
        ClaimedGroups::None => assert!(
            groups.is_empty(),
            "the measurement fixture must not claim any candidate; got {} groups at n = {}",
            groups.len(),
            rows.len()
        ),
        ClaimedGroups::All => {
            assert_eq!(
                groups.len(),
                1,
                "the claimed-set fixture must report exactly one group at n = {}",
                rows.len()
            );
            assert_eq!(
                groups[0].duplicates.len(),
                rows.len() - 1,
                "the first representative must claim every later row at n = {}",
                rows.len()
            );
        }
    }
    (seconds, exact_plan_seconds)
}

/// What the measured fixture must report, as a fixture-validity check.
#[derive(Clone, Copy)]
enum ClaimedGroups {
    None,
    All,
}

/// `pairs = n(n−1)/2` as an exact `u64` for the printed table.
fn comparisons(n: usize) -> u64 {
    (n as u64).saturating_mul(n.saturating_sub(1) as u64) / 2
}

/// Floor to two significant figures — the blueprint's derivation step, so the
/// printed recommendation matches the constant's rule.
fn floor_to_two_significant_figures(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return CAP_FLOOR;
    }
    let magnitude = value.log10().floor();
    let scale = 10_f64.powf(magnitude - 1.0);
    (value / scale).floor() * scale
}

/// The blend `MAX_NEAR_SCAN_PAIRS = min(max(250_000, floor2(10 s / c_iter)), 1_250_000_000)`.
fn derive_cap(c_iter_seconds: f64) -> f64 {
    if c_iter_seconds <= 0.0 || !c_iter_seconds.is_finite() {
        // A broken measurement cannot raise a bound: the floor is all it buys.
        return CAP_FLOOR;
    }
    floor_to_two_significant_figures(T_SCAN_MAX_SECONDS / c_iter_seconds)
        .clamp(CAP_FLOOR, CAP_CEILING)
}

/// One line of a `Command`'s stdout (or its failure, so the table is never
/// silently unattributed).
fn command_line(program: &str, args: &[&str]) -> String {
    match Command::new(program).args(args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let text = if stdout.trim().is_empty() {
                stderr
            } else {
                stdout
            };
            text.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("<no output>")
                .to_string()
        }
        Err(error) => format!("<{program} failed: {error}>"),
    }
}

/// The CPU model, not `lscpu`'s first line (which is only `Architecture:`):
/// prefer the `Model name:` row so the numbers are attributable to a machine.
fn cpu_model_line() -> String {
    match Command::new("lscpu").output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .map(str::trim)
                .find(|line| line.starts_with("Model name:"))
                .map(str::to_string)
                .unwrap_or_else(|| command_line("lscpu", &[]))
        }
        Err(error) => format!("<lscpu failed: {error}>"),
    }
}

#[test]
#[ignore = "measurement harness — prints the ladder, asserts no timing"]
fn measure_near_scan_ladder() {
    // ---- 1. re-derive the vector width from config resolution -------------
    let default_model = DEFAULTS.embedding_model;
    let resolved_dimensions = embedding_dimensions_for_model(default_model) as usize;
    println!("== environment ==");
    println!("cargo profile            : debug (cargo test without --release)");
    println!("lscpu model              : {}", cpu_model_line());
    println!(
        "rustc                    : {}",
        command_line("rustc", &["-V"])
    );
    println!("DEFAULTS.embedding_model : {default_model}");
    println!(
        "DEFAULTS.embedding_dimensions (table) : {}",
        DEFAULTS.embedding_dimensions
    );
    println!(
        "embedding_dimensions_for_model(default) : {resolved_dimensions}  (used for the fixtures)"
    );
    println!(
        "threshold                : {}",
        DEFAULTS.deduplication_similarity_threshold
    );
    println!();

    let dimensions = resolved_dimensions;
    let threshold = DEFAULTS.deduplication_similarity_threshold;

    // ---- 2. the ladder ----------------------------------------------------
    // `DEDUP_MEASURE_RUNGS` is a harness-only pilot knob (comma-separated `n`
    // values). The recorded run uses the blueprint's full ladder (unset).
    let rungs: Vec<usize> = match std::env::var("DEDUP_MEASURE_RUNGS") {
        Ok(override_list) => override_list
            .split(',')
            .filter_map(|part| part.trim().parse::<usize>().ok())
            .collect(),
        Err(_) => RUNGS.to_vec(),
    };
    let derive_only = rungs.is_empty();
    if derive_only {
        println!(
            "== ladder ==\nskipped: DEDUP_MEASURE_RUNGS is empty (derive-only reprint from a recorded c_iter)"
        );
    } else {
        println!("== ladder (one rung per line, printed as it completes) ==");
        println!(
            "{:>7} {:>16} {:>12} {:>14} {:>10} {:>14}",
            "n", "comparisons", "wall_s", "ns/comparison", "exponent", "exact_plan_ms"
        );
    }

    let mut measured: Vec<Rung> = Vec::new();
    let mut ran_fifty_k = false;
    let mut skipped_fifty_k_reason: Option<String> = None;

    let mut plan: Vec<usize> = rungs.clone();
    if plan.contains(&LARGEST_MANDATORY_RUNG) && !plan.contains(&50_000) {
        plan.push(50_000);
    }
    plan.sort_unstable();
    plan.dedup();

    for n in plan {
        // The 50 000 rung stays conditional on the 25 000 rung's wall clock —
        // unless the pilot knob named it explicitly.
        if n == 50_000 {
            let largest = measured
                .iter()
                .rev()
                .find(|rung| rung.n == LARGEST_MANDATORY_RUNG);
            let allow = std::env::var("DEDUP_MEASURE_RUNGS").is_ok()
                || largest.is_some_and(|rung| rung.seconds < FIFTY_K_GATE);
            if !allow {
                skipped_fifty_k_reason = Some(format!(
                    "the 25 000 rung took {:.1} s (> the {:.0} s gate)",
                    largest.map(|rung| rung.seconds).unwrap_or(f64::INFINITY),
                    FIFTY_K_GATE
                ));
                continue;
            }
        }

        let rows = one_hot_rows(n, dimensions);
        let (seconds, exact_plan_seconds) = time_scan(&rows, threshold, ClaimedGroups::None);
        let pairs = comparisons(n);
        let ns_per_comparison = seconds * 1e9 / pairs as f64;
        let exponent = measured.last().map(|previous| {
            let previous_pairs = previous.comparisons as f64;
            let current_pairs = pairs as f64;
            (seconds / previous.seconds).ln() / (current_pairs / previous_pairs).ln()
        });
        if n == 50_000 {
            ran_fifty_k = true;
        }
        println!(
            "{:>7} {:>16} {:>12.3} {:>14.1} {:>10} {:>14.3}",
            n,
            pairs,
            seconds,
            ns_per_comparison,
            exponent
                .map(|value| format!("{value:.3}"))
                .unwrap_or_else(|| "-".to_string()),
            exact_plan_seconds * 1e3,
        );
        measured.push(Rung {
            n,
            comparisons: pairs,
            seconds,
        });
    }
    println!();

    // ---- 3. the two skip paths, at n = 5_000 ------------------------------
    const SKIP_N: usize = 5_000;
    let skip_pairs = comparisons(SKIP_N) as f64;
    if !derive_only {
        println!("== skip paths the budget must also cover (n = {SKIP_N}) ==");

        let tag_rows = tag_reject_rows(SKIP_N, dimensions);
        let (tag_seconds, _) = time_scan(&tag_rows, threshold, ClaimedGroups::None);
        println!(
            "tag rejection (no cosine)  : {:>10.3} s  {:>10.1} ns/comparison  ({skip_pairs:.0} comparisons)",
            tag_seconds,
            tag_seconds * 1e9 / skip_pairs
        );

        let claimed_rows = claimed_set_rows(SKIP_N, dimensions);
        let (claimed_seconds, _) = time_scan(&claimed_rows, threshold, ClaimedGroups::All);
        println!(
            "claimed-set growth         : {:>10.3} s  {:>10.1} ns/comparison  (the Vec<&str> membership scan)",
            claimed_seconds,
            claimed_seconds * 1e9 / skip_pairs
        );
        println!(
            "ratio cosine/tag-reject    : {:.1}x — the budget must count comparisons, not cosines",
            measured
                .iter()
                .find(|rung| rung.n == SKIP_N)
                .map(|rung| rung.seconds / tag_seconds.max(f64::MIN_POSITIVE))
                .unwrap_or(f64::NAN)
        );
        println!();
    }

    // ---- 4. the decision-rule inputs --------------------------------------
    // `c_iter` is taken at the largest MEASURED rung. In derive-only mode
    // (`DEDUP_MEASURE_RUNGS=` empty, plus `DEDUP_MEASURE_C_ITER_NS=<recorded
    // ns>`) the same arithmetic is reprinted from a number an earlier run
    // recorded, which is how the cap and the AC2 recommendation are reproduced
    // without repeating a one-hour ladder. The input is named in the output, so
    // a reprinted block can never be mistaken for a fresh measurement.
    let recorded_c_iter_ns = if derive_only {
        Some(
            std::env::var("DEDUP_MEASURE_C_ITER_NS")
                .ok()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .unwrap_or_else(|| {
                    panic!(
                        "derive-only mode needs DEDUP_MEASURE_C_ITER_NS (the recorded c_iter in ns)"
                    )
                }),
        )
    } else {
        None
    };
    let largest = measured.iter().max_by_key(|rung| rung.n);
    let c_iter_seconds = match recorded_c_iter_ns {
        Some(ns) => ns / 1e9,
        None => {
            let largest = largest.expect("the ladder measured at least one rung");
            largest.seconds / largest.comparisons as f64
        }
    };
    let c_iter_ns = c_iter_seconds * 1e9;
    let t50_measured_or_fitted = c_iter_seconds * ONE_SHARD_GRID;
    let branch = if t50_measured_or_fitted <= 1.0 {
        "R1 (validity gate: this harness is NOT measuring cosines — fix and re-run)"
    } else if t50_measured_or_fitted <= REQUEST_TIMEOUT_SECONDS {
        "R2 (offload + cap both required)"
    } else {
        "R3 (offload + cap both required; the pre-fix endpoint could never answer)"
    };

    println!("== decision rule ==");
    match (recorded_c_iter_ns, largest) {
        (Some(ns), _) => println!(
            "largest fully measured rung : derive-only reprint from a recorded c_iter = {ns:.1} ns"
        ),
        (None, Some(largest)) => println!(
            "largest fully measured rung : n = {} ({} comparisons, {:.3} s)",
            largest.n, largest.comparisons, largest.seconds
        ),
        (None, None) => unreachable!("a non-derive-only run measures at least one rung"),
    }
    if recorded_c_iter_ns.is_some() {
        println!("n = 50_000                  : NOT measured; t50 below is INFERRED from c_iter");
    } else if !ran_fifty_k {
        println!(
            "n = 50_000                  : NOT run ({}); t50 below is INFERRED from c_iter",
            skipped_fifty_k_reason
                .clone()
                .unwrap_or_else(|| "not requested".to_string())
        );
    } else {
        println!("n = 50_000                  : measured");
    }
    println!("c_iter                      : {c_iter_ns:.1} ns per candidate comparison");
    println!(
        "t50 = c_iter x 1,249,975,000: {t50_measured_or_fitted:.1} s ({})",
        if ran_fifty_k { "measured" } else { "INFERRED" }
    );
    println!("branch                      : {branch}");

    let cap = derive_cap(c_iter_seconds);
    println!(
        "floor2(10 s / c_iter)       : {}",
        floor_to_two_significant_figures(1e10 / c_iter_ns)
    );
    println!("MAX_NEAR_SCAN_PAIRS         : {cap:.0}");
    println!(
        "  -> a single 50 000-row shard (1,249,975,000 pairs) {}",
        if ONE_SHARD_GRID > cap {
            "IS truncated by the cap (the cap binds per shard)"
        } else {
            "is NOT truncated by the cap (the cap is non-binding per shard by construction)"
        }
    );
    println!(
        "  -> a whole pass covers about {:.0} survivors (sqrt(2 x {:.0} pairs))",
        (2.0 * cap).sqrt(),
        cap
    );

    // ---- 5. the centrepiece's recommended n (AC2) -------------------------
    // Two jobs: n(n-1)/2 < cap, and >= ~0.1 s of scan in THIS (debug) profile.
    let pairs_for_tenth_of_a_second = (1e8 / c_iter_ns).ceil();
    let recommended_n = {
        // Smallest n with n(n-1)/2 >= pairs_needed, then rounded up to a
        // multiple of 10 so the literal is stable against a re-measure.
        let double_pairs = 2.0 * pairs_for_tenth_of_a_second;
        let root = (1.0 + (1.0 + 4.0 * double_pairs).sqrt()) / 2.0;
        let mut n = root.ceil() as usize;
        n = n.div_ceil(10) * 10;
        while comparisons(n) as f64 >= cap {
            n = n.saturating_add(10);
        }
        n
    };
    println!();
    println!("== centrepiece recommendation (AC2) ==");
    println!(
        "recommended n               : {recommended_n}  (grid {} comparisons, {:.2} s of scan in this debug profile)",
        comparisons(recommended_n),
        c_iter_ns * comparisons(recommended_n) as f64 / 1e9
    );
    println!(
        "  jobs: {} < MAX_NEAR_SCAN_PAIRS = {}  AND  >= 0.1 s in debug",
        comparisons(recommended_n),
        cap
    );
    println!(
        "this ladder's debug c_iter  : {c_iter_ns:.1} ns/pair -> the derived cap is CONSERVATIVE against release"
    );
    println!();
    println!("== end of ladder ==");
}
