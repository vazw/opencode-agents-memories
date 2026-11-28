# Cleanup-tick latency: the profile is the measurement

Status: **measurement, not a fix.** No product behaviour changed in this wave.
`tests/cleanup_tick_latency.rs::cleanup_tick_latency` is a wall-clock instrument:
it measures how long a capture waits behind one cleanup sweep. Its absolute G3
cap (`G3_MAX_DUE_MS = 150`) is a **release-profile** claim, and the debug profile
reds it by construction because the sweep itself is ~10× slower unoptimised. The
test now says so in its own source instead of hiding the red:

```rust
#[cfg_attr(
    debug_assertions,
    ignore = "absolute G3 cap is a release-profile claim: the debug sweep is ~10x slower (measured 4956 ms vs 493 ms); run with --release"
)]
```

The cap is not scaled by profile. An absolute cap that scales is not a cap.

## Run 1 — debug profile (red by construction)

Command:

```sh
cargo test -p opencode-agents-memory --test cleanup_tick_latency -- --ignored --nocapture
```

`--ignored` is used only to produce the numbers below: the default debug run now
prints `test cleanup_tick_latency ... ignored, absolute G3 cap is a
release-profile claim: ... run with --release` and exits 0.

Printed lines (host: this repo's workspace, 2026-11-28T08:35:02Z):

```
arm, n, p50, p95, max
not_due, 40, 22.27, 23.91, 25.27
arm, n, p50, p95, max
due, 40, 109.25, 221.99, 955.76
tick_duration_ms=4904 deleted_count=3600 max_delay_excess_ms=930.49 g2_bound_ms=2452.00
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Arm A (`not_due`) max 25.27 ms, arm B (`due`) max 955.76 ms, so
`max(due) − max(not_due) = 930.49 ms ≤ 2452.00 ms` — **G2 passes**. G3 fails:
`max(due) = 955.76 ms` exceeds the 150 ms cap, exactly because the unoptimised
sweep took 4904 ms of wall clock inside a 250 ms tick budget.

## Run 2 — release profile (green, the gate that ships)

Command:

```sh
cargo test --release -p opencode-agents-memory --test cleanup_tick_latency -- --nocapture
```

Printed lines (2026-11-28T08:35:21Z):

```
arm, n, p50, p95, max
not_due, 40, 7.87, 8.23, 8.85
arm, n, p50, p95, max
due, 40, 8.95, 22.85, 33.28
tick_duration_ms=375 deleted_count=3600 max_delay_excess_ms=24.42 g2_bound_ms=187.50
test result: ok. 1 passed; 0 failed; 0 ignored
```

Arm A max 8.85 ms, arm B max 33.28 ms: `max(due) − max(not_due) = 24.42 ms ≤
187.50 ms` (**G2 pass**) and `max(due) = 33.28 ms ≤ 150 ms` (**G3 pass**).

### Prior runs this decision was based on (MEASURED 2026-11-28T08:05Z, same fixture)

- debug: `tick_duration_ms=4956 max_due=957.46 excess=933.05 g2_bound=2478.00` — G2 pass, G3 fail.
- release: `tick_duration_ms=493 max_due=38.80 not_due max=9.11 excess=29.69 g2_bound=246.50` — both pass.

The runs above reproduce both verdicts on the same fixture; absolute milliseconds
differ with host load (release tick 375 ms here vs 493 ms then), the verdict does not.

## Verdict

- **G2**, the instrument's own mutant gate (`excess ≤ 0.5 × tick_duration_ms`),
  never fires in either profile. It is scale-free: `max(due)` tracks the sweep's own
  duration (955.76 / 4904 = 0.195 in debug, 33.28 / 375 = 0.089 in release; the
  earlier release run 38.80 / 493 = 0.079) rather than a fixed excess, so a loaded
  host moves both sides together.
- **G3**, the absolute 150 ms cap, is satisfied in release with a wide margin
  (33.28 ms, 22 % of the cap) and cannot be satisfied in debug because the sweep is
  10–13× slower there. The test is `#[cfg_attr(debug_assertions, ignore = ...)]`,
  so the release gate stays in the suite and the skip is visible in every debug run.
- What would refute this: **a release run on an idle host (`max_not_due <= 25`)
  that reds G3, or any release run that reds G2.** Either names a real ticker
  defect instead of host load.

## Reproduce

```sh
cargo test -p opencode-agents-memory --test cleanup_tick_latency            # prints "ignored", exit 0
cargo test --release -p opencode-agents-memory --test cleanup_tick_latency  # exit 0
```

Both commands were run for this file; the outputs above are theirs, not re-typed.

## Final-tree re-run (after `cargo fmt --all`)

The release gate was re-run on the exact tree handed off, exit 0:

```
arm, n, p50, p95, max
not_due, 40, 7.79, 8.11, 8.63
arm, n, p50, p95, max
due, 40, 8.91, 24.38, 32.31
tick_duration_ms=382 deleted_count=3600 max_delay_excess_ms=23.68 g2_bound_ms=191.00
test result: ok. 1 passed; 0 failed; 0 ignored
```

Three release runs on this host now read `max(due)` 33.28 / 32.31 ms against a
150 ms cap and `excess` 24.42 / 23.68 ms against 187.50 / 191.00 ms — the verdict
is stable, the milliseconds are not.
