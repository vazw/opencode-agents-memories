# Release-profile dedup scan ladder — issue 56

## Why this directory exists

`MAX_NEAR_SCAN_PAIRS = 740_000` is a production bound whose whole derivation input is a **debug**
measurement: `c_iter = 13,487.9 ns/pair`, recorded by issue 54b. Issue 56 is the follow-up it named:
detect the measurement profile instead of asserting it, then measure the same ladder once in release so the
constant's provenance is a measured release input rather than a projection.

The 54b artifact does not exist any more. **MEASURED 2026-10-26:** `.tmp/measure/` exists again (the linker
A/B re-created it) but the file it held, `.tmp/measure/20261026-issue-54b-scan-ladder.txt`, is absent —
`ls` on that path fails. The raw 54b numbers survive only because 54b transcribed them into
`.scratch/rust-port/issues/54b-dedup-scan-bounding.md`. This directory is committed for exactly that reason:
the raw runs, the scripts that produced them and the aggregation sit in git beside the record, not in
`.tmp/`.

The directory name is the **UTC** date of the runs (`date -u +%F` inside both run scripts, resolved into
`resolved-dir.txt`). At the time of writing the local clock reads `2026-10-27 02:15 +07:00` while UTC reads
`2026-10-26T19:15Z`, so the directory can differ from the local calendar date — that is the property the
`sccache-2026-10-26/` precedent actually has.

## The instrument defect, and what changed

`crates/mem-web/tests/dedup_scan_measure.rs` printed its profile as a hardcoded string:

```rust
println!("cargo profile            : debug (cargo test without --release)");
```

Every `--release` run through that harness was therefore mislabelled at its source. The harness now binds
`cfg!(debug_assertions)` once and derives every profile word from it:

- `profile_entry(bool)` builds the one printed profile line, hedged to exactly what the flag proves
  ("dev profile, **or** a custom profile with debug-assertions on" / "release, **or** a custom profile with
  debug-assertions off"), with `opt-level not probed` stated because Rust exposes no stable compile-time
  probe for it;
- `profile_short(bool)` supplies the bare word for the three prose sites (no `--release` assertion, no
  `opt-level` claim);
- `environment_lines(bool)` is the whole `== environment ==` block, profile entry first;
- `expectation_mismatch(&str, bool)` is the pre-flight guard: `DEDUP_MEASURE_EXPECT` is recognised as
  exactly three states — unset/empty, `debug`, `release`. A mismatch **and an unrecognised value** (a typo
  such as `relase`, a case variant, a number) both panic, because mapping an unknown expectation to "no
  expectation" would reproduce the defect the guard exists to prevent.

The run script sets `DEDUP_MEASURE_EXPECT=release`, so a dropped `--release` flag fails in well under a
second (measured: 0.03 s) with the reason in the artifact. The guard is evaluated **after** the environment block, so even a refused
run records which profile it found before refusing.

The harness writes no file itself: the artifact is the redirected stdout, written into this directory by the
committed run scripts. `stdout` is line-buffered, so a killed run keeps every completed rung and lacks the
`== end of ladder ==` marker, which is the truncation detector. The profile entry precedes every rung row
because the environment block is printed before the first fixture is built.

The print block beside the derivation was also relabelled, so a release artifact cannot be read as the
constant:

```text
floor2(10 s / c_iter)       : <floored candidate>
cap candidate (this run)    : <candidate>   <- derived from THIS run's c_iter, not the source constant
MAX_NEAR_SCAN_PAIRS (source): <MAX_NEAR_SCAN_PAIRS>
cap candidate / source      : <candidate / source>x
```

with the two `->` lines (per-shard truncation, whole-pass survivor coverage) kept **below** those four, and
`MAX_NEAR_SCAN_PAIRS` imported from `mem_web::api::deduplication` instead of retyped. The value is untouched.

## Run environment

Every line below is copied from `release-run-1.txt`; the `== environment ==` block is byte-identical across
the three runs (the UTC stamps live in each file's `# run N started` header and `# run N exit=... finished`
footer, not in this block), and `argv` names the release test binary.

```text
cargo profile            : detected debug_assertions=false -> optimised (debug-assertions off: release, or a custom profile with debug-assertions off); opt-level not probed
lscpu model              : Model name:                              AMD Ryzen 5 6600U with Radeon Graphics
rustc                    : rustc 1.98.0 (88d9e12ae 2026-08-18)
DEFAULTS.embedding_model : Xenova/nomic-embed-text-v1
DEFAULTS.embedding_dimensions (table) : 768
embedding_dimensions_for_model(default) : 768  (used for the fixtures)
threshold                : 0.9
argv                     : /home/vaz/Code/Personal/Rust/opencode-agents-memory/target/release/deps/dedup_scan_measure-6050abf30c149fcb --ignored --nocapture --test-threads=1
```

- **Profile:** `debug_assertions=false`, i.e. the optimised/release build — the same test
  (`test_environment_report_names_the_profile_this_binary_actually_has`) passes under `cargo test
  -p mem-web --release --test dedup_scan_measure`, and the debug witness beside it prints
  `debug_assertions=true`. The pair is the end-to-end proof that the detection fix works.
- **Fixture width (READ from the artifacts): `768` lanes** — `embedding_dimensions_for_model(default) = 768`.
  54b's `c_iter` was also taken at 768 lanes, so the debug/release ratio `k` is not confounded by a width
  change between the two measurements. Stated beside `c_iter_release` on purpose.
- **Release build wall time (MEASURED): 289.9 s** — `cargo test -p mem-web --release --test
  dedup_scan_measure --no-run`, 2026-10-26T18:56:14Z → 19:01:04Z, cargo reported `Finished release profile
  [optimized] target(s) in 4m 49s`. An earlier measurement in this tree built the same target in 357 s; this
  run re-measured 289.9 s, which is not that estimate — it is this build. Runs 2 and 3 did not re-link
  (`Finished ... in 0.26s` / `0.24s`).
- **cargo config preflight (recorded, not edited).** `~/.cargo/config.toml` carries
  `rustc-wrapper = "sccache"`, `SCCACHE_CACHE_SIZE = "10G"`, `SCCACHE_BASEDIRS = "/home/vaz/Code/Personal/Rust"`
  (the last two via its `[env]` table) and `jobs = 2`; it declares no `[build]` rustflags, and the shell
  environment carried no `RUSTFLAGS` or `SCCACHE_*` entry either. Its only `rustflags` are the two
  `-Clink-arg` entries in the difference below. Every cargo call in this record also ran through
  `.tmp/scripts/cargo-serialized.sh`, which exports `CARGO_INCREMENTAL=0` — the release default, so it does
  not move `c_iter`; the two rustflags are link-time only and likewise do not. Difference from the
  fragment recorded at `docs/evidence/sccache-2026-10-26/README.md:60-69`: the global config now **also**
  carries `[target.x86_64-unknown-linux-gnu] linker = "clang"` with
  `-Clink-arg=--ld-path=/home/vaz/.cargo/bin/wild` and `-Clink-arg=-Wl,--no-madvise-huge-pages`, and the
  repo-local `.cargo/config.toml` carries the same two entries. They do mean this build's link phase ran
  under the wild linker, which is part of the recorded wall time.
- **Per-run UTC stamps:** run 1 `19:01:44Z` → `19:02:25Z`, run 2 `19:02:25Z` → `19:03:07Z`, run 3
  `19:03:07Z` → `19:03:49Z`. Each is one separate harness invocation (`n = 3` observations per rung), not
  three passes inside one process.
- **Debug vs release is not a debug-info confound.** `[profile.dev]` sets `debug = 1` and
  `split-debuginfo = "unpacked"` (`Cargo.toml:72-82`); those are debug-info settings and do not change
  generated code. What `c_iter` measures is `opt-level` (0 vs the release default 3) plus
  `debug-assertions`/`overflow-checks` on vs off. No `[profile.release]` exists in this repo, and this wave
  set no `RUSTFLAGS`, LTO or codegen-units override.

## Debug reference (issue 54b, unchanged)

Verbatim from `.scratch/rust-port/issues/54b-dedup-scan-bounding.md:45-53` (the artifact itself is gone, see
above). **Profile: debug** (`cargo test` without `--release`), 768 lanes.

```text
      n      comparisons       wall_s  ns/comparison   exponent  exact_plan_ms
    100             4950        0.073        14801.0          -          3.668
   1000           499500        6.507        13026.3      0.972          4.083
   5000         12497500      146.553        11726.6      0.967         18.295
  10000         49995000      674.326        13487.9      1.101         31.329
```

| quantity | value | label |
|---|---|---|
| `c_iter` at `n = 10_000` | 13,487.9 ns/pair (674.326 s / 49,995,000 comparisons) | **debug**, measured |
| `t50` (one 50,000-row shard, 1,249,975,000 pairs) | 16,859 s ≈ 4.68 h | **debug**, **INFERRED** — the 50k rung never ran |
| `MAX_NEAR_SCAN_PAIRS` = floor2(10 s / 13,487.9 ns) | 740,000 | debug-derived |
| coverage at the cap | √(2 × 740,000) ≈ 1,217 survivors ≈ 2.4 % of a 50k shard | debug-derived |

Nothing in this column was edited. The debug figure is kept as the conservative input.

## Release rung table

One row per run × rung, copied from `release-run-{1,2,3}.txt` (the artifacts are never edited after a run).

| run | n | comparisons | wall_s | ns/comparison | exponent | exact_plan_ms |
|---|---|---|---|---|---|---|
| 1 | 1,000 | 499,500 | 0.308 | 616.1 | – | 0.601 |
| 1 | 5,000 | 12,497,500 | 7.880 | 630.6 | 1.007 | 2.366 |
| 1 | 10,000 | 49,995,000 | 32.168 | 643.4 | 1.015 | 3.674 |
| 2 | 1,000 | 499,500 | 0.321 | 643.1 | – | 0.867 |
| 2 | 5,000 | 12,497,500 | 8.270 | 661.8 | 1.009 | 3.364 |
| 2 | 10,000 | 49,995,000 | 33.851 | 677.1 | 1.017 | 4.268 |
| 3 | 1,000 | 499,500 | 0.318 | 637.3 | – | 0.676 |
| 3 | 5,000 | 12,497,500 | 8.273 | 662.0 | 1.012 | 2.417 |
| 3 | 10,000 | 49,995,000 | 32.941 | 658.9 | 0.997 | 3.900 |

Rung set: exactly `{1_000, 5_000, 10_000}` (`DEDUP_MEASURE_RUNGS=1000,5000,10000`). `10_000` is the
comparability rung (the same `n` 54b recorded, so `k` needs no pair-count correction); `1_000` is the gate
rung; `5_000` is the shape point and keeps the `ratio cosine/tag-reject` line numeric (run 1: `155.6x`).
No `25_000`, no `50_000` — the conditional 50k injection needs `25_000` in the plan, so it did not fire.
Every rung row has `debug_assertions=false` above it and all three runs printed `== end of ladder ==`.

The **17.9 s early-stop gate** fired on none of the runs: run 1's `n = 1_000` row is 0.308 s. The gate is
derived from the time-box, not chosen — `49,995,000 / 499,500 = 100.0901×`, and `1800 / 100.0901 = 17.98 s`,
so a `n = 1_000` row above 17.98 s guarantees the `n = 10_000` rung is killed by the 1800 s box; the gate is
set at **17.9 s, i.e. 0.08 s below that crossing, as margin**. `timeout -k 30 1800` wrapped each run and no
kill occurred (`# run N exit=0` in all three files). No `# ABORT` line was written, none was hand-written,
and no stray `dedup_scan_measure-` process survived (`# no stray measurement process`).

## Aggregation

`derive.sh` (median of the per-run `ns/comparison` at each rung; `n = 3` is odd, so the median is the middle
value and no interpolation is claimed). `derived.txt` verbatim:

```text
# derived 2026-10-26T19:04:07Z
n=1000   runs=3 min=616.1 median=637.3 max=643.1 spread=4.4%
n=5000   runs=3 min=630.6 median=661.8 max=662.0 spread=5.0%
n=10000  runs=3 min=643.4 median=658.9 max=677.1 spread=5.2%
# detected profiles:
cargo profile            : detected debug_assertions=false -> optimised (debug-assertions off: release, or a custom profile with debug-assertions off); opt-level not probed
cargo profile            : detected debug_assertions=false -> optimised (debug-assertions off: release, or a custom profile with debug-assertions off); opt-level not probed
cargo profile            : detected debug_assertions=false -> optimised (debug-assertions off: release, or a custom profile with debug-assertions off); opt-level not probed
cargo profile            : detected debug_assertions=true -> debug (debug-assertions on: dev profile, or a custom profile with debug-assertions on); opt-level not probed
# end-of-ladder markers:
/home/vaz/Code/Personal/Rust/opencode-agents-memory/docs/evidence/dedup-release-profile-2026-10-26/release-run-1.txt:1
/home/vaz/Code/Personal/Rust/opencode-agents-memory/docs/evidence/dedup-release-profile-2026-10-26/release-run-2.txt:1
/home/vaz/Code/Personal/Rust/opencode-agents-memory/docs/evidence/dedup-release-profile-2026-10-26/release-run-3.txt:1
```

The aggregation is the **median of the per-run `ns/comparison`** at each rung, not `median(wall_s)` divided
by the pair count; the two agree because the denominator is fixed per rung, and this record states which
one it computed. The fourth `cargo profile` line is the debug witness (`debug-probe-1k.txt`), which is why
both values appear in one listing.

## `c_iter_release` and `k`

- **`c_iter_release = 658.9 ns/pair`** — the `n = 10_000` median, at **768 lanes**. (min 643.4, max 677.1,
  spread 5.2 %, `n = 3` runs.)
- **`k = 13,487.9 / 658.9 = 20.470`** — the measured debug/release factor, same `n`, same width.
- `t50_release = 658.9 ns × 1,249,975,000 = 823.6 s ≈ 13.7 min` — **INFERRED** from the measured
  `c_iter_release`; the 50k rung was not run. R3 survives: 823.6 s > the 330 s request timeout, so the
  branch verdict is unchanged.
- The `cap candidate / source` ratios in the artifacts are **not** disagreements with the constant: run 1
  prints `20.27x`, run 2 `18.92x`, run 3 `20.27x`, all derived from that run's own `c_iter` at `n = 10_000`,
  whereas `MAX_NEAR_SCAN_PAIRS` remains 740,000 (this run's candidate is never written back). The
  derive-only reprint (recorded debug `c_iter`, no ladder) prints `1.00x` at 13,487.9 ns, and the debug
  probe at `n = 100` prints `1.32x` — a 100-row rung is noisy and its `c_iter` is 10,202.7 ns, so neither
  ratio is a measurement of the release record. The probe's `ratio cosine/tag-reject : NaNx` is expected:
  that invocation's ladder has no `n = 5_000` rung to look up, and it is a debug witness, not part of this
  table.

## Decision table

`cap-table.sh` re-implements the pre-registered rule exactly — `floor2`, floor 250,000, ceiling
1,250,000,000 — and prints both candidates at the measured release `c_iter`. `cap-table.txt` verbatim:

```text
candidate                       cap   t_at_cap_s   coverage pct_of_50k
current (debug-derived)       740000         0.49       1217      2.4%
release-derived            15000000         9.88       5477     11.0%
# k = 20.470  (13487.9 / 658.9)
```

Self-check run before the table was used: `cap-table.sh 13487.9` prints `740000` for both candidates, i.e.
the script agrees with the shipped constant when fed the recorded debug figure.

The **pre-registered rule**, quoted (owner decision; this wave does not touch the constant):
`T_SCAN_MAX = 10 s` was fixed *before* the debug measurement (`54b:154-156`) precisely so it could not be
tuned afterwards, and re-deriving the cap is legitimate **only if the owner re-affirms that rule for the
release profile**. The budget is `10 s` over 740,000 pairs, so break-even is
`10 s / 740_000 = 13,513.51 ns`, **not** `13,487.9 ns`: at the debug figure the current cap spends
`740,000 × 13,487.9 ns = 9.981 s`, which does not exceed 10 s.

**Branch that fired: row 1.** `c_iter_release = 658.9 ns` is at or below `13,513.51 ns`, and it is far
below `13,487.9 ns` — outside the recorded spread in the fast direction — so row 1 applies, not row 3
(row 3 is only for a figure above 13,487.9 ns but *inside* the spread; rows 1 and 3 overlap only for
`13,487.9 < c ≤ 13,513.51` when the value is inside the spread, and that band was not entered). Row 2's
condition (`c_iter_release > 13,513.51 ns` **and** beyond the spread above the debug figure) is false.
`cap_release = 15,000,000 ≠ MAX_NEAR_SCAN_PAIRS`, so row 1's raise phrasing applies rather than the
"unchanged, not raised" form (that form exists only for the no-op case where the floored candidate still
equals 740,000).

## Recommendation

The matching row of the pre-registered table, and only that row:

> Re-affirm `T_SCAN_MAX = 10 s` for the release profile, then raise `MAX_NEAR_SCAN_PAIRS` to `15000000`:
> `9.88 s` of a detached blocking task per pass buys `sqrt(k) = 4.52`× coverage (`1,217` -> `5,477`
> survivors, `2.4` -> `11.0` % of a 50k shard). Not changed in this wave.

The trade the owner is being asked to accept, stated plainly: the current cap is conservative on the
measured profile — it spends `740,000 × 658.9 ns = 0.49 s`, well under the pre-registered 10 s — so the
endpoint truncates earlier than the budget intends. That is a **usefulness** loss, not a safety bug, which
is why this is not urgent. Keeping 740,000 is safe. The **ceiling** is the quantity that does not bind:
it is 1,250,000,000, ~83× the proposed 15,000,000, so `cap_release = 15,000,000` never reaches it. The
**proposal** still binds per shard — 15,000,000 pairs against a 50,000-row shard's 1,249,975,000 — and that
truncation is exactly what buys the 5,477 survivors above.

## What this does NOT establish

- **`n = 50_000` was not run.** `t50_release = 823.6 s` is INFERRED from the measured `c_iter_release`, and
  the ladder stopped at `n = 10_000` by pre-registration.
- **`n = 3` on one machine, with no load or thermal control.** The spread is reported (4.4 / 5.0 / 5.2 %),
  not explained. Single-tenancy holds **during** a run — the fleet lock is taken inside the cargo call —
  but not between runs, so the gaps are the coder's discipline; a contended lock would consume the 1800 s
  box *before* cargo starts and would show up as an empty run, which did not happen here.
- **`opt-level` was not probed.** The profile line names `debug_assertions` and nothing more; a custom
  profile with `debug-assertions = false` and `opt-level = 0` would print the optimised line.
- **The release run's AC2 recommendation is not adopted.** `pairs_for_tenth_of_a_second` is
  profile-relative, and the suite's 0.1 s sizing belongs to the **debug** profile the default suite runs
  in; the release artifacts print `recommended n : 560` / `550` / `560` for runs 1–3 (0.10 s in the
  optimised profile), which stays an artifact line, not a suite change.
- **This record does not decide the cap.** It makes the decision decidable against the pre-registered rule
  and leaves `MAX_NEAR_SCAN_PAIRS = 740_000` unchanged; the ownership of that decision is why the issue is
  `ready-for-human` rather than `done`.
- **The debug/release ratio is not a kernel-effect study.** Both measurements run the same kernel;
  `k` measures the profile difference, not a code change.
- **`lscpu` did print a `Model name:` line** here (AMD Ryzen 5 6600U), unlike 54b; it is copied above.

## Reproduction

Scripts, all committed in this directory (`sh <script>` from anywhere; each resolves the repo root itself):

```sh
# 1. the release ladder: three separate runs, 1800 s time-box, 17.9 s early-stop gate
sh docs/evidence/dedup-release-profile-2026-10-26/run-release.sh

# 2. the debug detection witness (n = 100, the pair to the release files)
sh docs/evidence/dedup-release-profile-2026-10-26/run-debug-probe.sh

# 3. aggregation -> derived.txt
docs/evidence/dedup-release-profile-2026-10-26/derive.sh > docs/evidence/dedup-release-profile-2026-10-26/derived.txt

# 4. the cap/coverage table at the n = 10_000 median
docs/evidence/dedup-release-profile-2026-10-26/cap-table.sh 658.9 > docs/evidence/dedup-release-profile-2026-10-26/cap-table.txt
```

Every cargo call goes through `.tmp/scripts/cargo-serialized.sh` (one compile at a time; the lock is taken
inside the cargo call). The release build is paid once, before run 1; runs 2 and 3 re-link nothing. The
harness itself:

```sh
sh .tmp/scripts/cargo-serialized.sh cargo test -p mem-web --test dedup_scan_measure            # 6 unit tests, 1 ignored
sh .tmp/scripts/cargo-serialized.sh cargo test -p mem-web --release --test dedup_scan_measure  # same 6, under --release
```

## Files

| file | what it is |
|---|---|
| `README.md` | this record |
| `release-run-1.txt` … `release-run-3.txt` | raw stdout of each release run, verbatim |
| `debug-probe-1k.txt` | raw stdout of the debug detection witness (`DEDUP_MEASURE_RUNGS=100`) |
| `resolved-dir.txt` | the UTC-dated directory the run scripts resolved (copied, never retyped) |
| `run-release.sh` | the three-run loop: time-box, UTC stamps, early stop, error/ABORT capture |
| `run-debug-probe.sh` | the debug witness |
| `derive.sh`, `derived.txt` | the aggregation and its verbatim output |
| `cap-table.sh`, `cap-table.txt` | the two-candidate cap/coverage table and its verbatim output |
