# 56 — Dedup · the cap, the cost curve and DEDUP-001's magnitude are all debug-profile numbers

Status: ready-for-agent — 2026-10-28 (owner ruled: re-affirm `T_SCAN_MAX = 10 s` for the release profile and
raise `MAX_NEAR_SCAN_PAIRS` to **15,000,000** — 9.88 s of the pre-registered budget, coverage 2.4 % → 11.0 %.
A one-step wave: the constant, its derivation test, and the ruling record; no re-measurement.)

## PREMISE — the numbers, and the profile they came from

Source: `.tmp/measure/20261026-issue-54b-scan-ladder.txt`, reproduced in
`.scratch/rust-port/issues/54b-dedup-scan-bounding.md`.

| quantity | value | profile |
|---|---|---|
| `c_iter` at `n = 10_000` | 13,487.9 ns/pair | **debug** (`cargo test` without `--release`) |
| `t50` (one 50 000-row shard, 1,249,975,000 pairs) | 16,859 s ≈ 4.68 h | debug, and `INFERRED` — the 50k rung never ran |
| derived cap `floor2(10 s / c_iter)` | 740,000 pairs | debug-derived |
| coverage at the cap | √(2 × 740,000) ≈ 1,217 survivors ≈ 2.4 % of a 50k shard | debug-derived |

The branch is **R3** (`t50 > 330 s`) and stays R3 under any plausible release factor: at 40× faster,
`t50 ≈ 421 s`, still above the 330 s `HTTP_IDLE_TIMEOUT`. So **the decision does not need re-measuring**. The
constant does: a release build is faster, so 740,000 comparisons buy *less* than 10 s there, and production
truncates earlier than the pre-registered budget intends.

## A second defect, in the instrument itself

`crates/mem-web/tests/dedup_scan_measure.rs:310` prints the profile as a **hardcoded string**:

```rust
println!("cargo profile            : debug (cargo test without --release)");
```

It asserts the profile rather than observing it. Run the harness with `--release` and it still prints `debug` —
so any future release measurement through this harness is mislabelled at its source, and the artifact cannot be
trusted to say which profile produced it. Fix: detect it (`cfg!(debug_assertions)`) and print the detected
value, refusing to print a claim it did not check. Same class as issue 54b's F3 — a claim of a measurement that
was not run — one layer down, in the instrument.

## Why it matters at all

The cap is not a performance knob; it is what bounds a detached blocking task, after the debugger's constraint
("a bare `spawn_blocking` converts a freeze into detached work that outlives cancellation"). A debug-derived cap
is *safe* — it under-grants — so this is a **usefulness** defect, not a safety one. That is why it is
`needs-triage` and not urgent. But ~2.4 % coverage on a full shard is near the point where the endpoint's
near-duplicate report stops being worth reading on exactly the stores that need it.

## What a follow-up should produce

1. Fix the profile print (detect, do not assert) — one line, no re-measurement.
2. Re-run the ladder **in release**, recording `c_iter_release` and the profile it actually detected.
3. Re-derive the cap from the release figure **only if** the pre-registered rule is re-affirmed for that
   profile — `T_SCAN_MAX = 10 s` was fixed before the debug measurement precisely so it could not be tuned
   afterwards, and switching profiles is the moment to restate the rule rather than quietly import a
   better-looking number.
4. Keep the debug figure in the record as the conservative input, so the two are never confused.

## Gate

`c_iter`, `t50` and the cap are `READ` from the recorded artifact; the profile label is `READ` from the source
at `5c06275`. The release `c_iter` is **unmeasured** — that is the whole issue. Nothing here reopens DEDUP-001,
which `opsec` closed on the branch (`dedup/near-scan-inline-runtime-thread` → `rejected`).

## Comments

### Outcome (record-only; implementation commits e168afb, b50fe18)

- **Detected profile.** `crates/mem-web/tests/dedup_scan_measure.rs` no longer prints a hardcoded
  `debug (cargo test without --release)`. It binds `cfg!(debug_assertions)` once and derives every profile
  word from it (`profile_entry`, `profile_short`, `environment_lines`), plus a `DEDUP_MEASURE_EXPECT` guard
  that panics on a mismatch **and** on an unrecognised value (`relase`, `Release`, `1`), evaluated after the
  environment block so a refused run still records the profile it found. Six pure unit tests pin the
  helpers, the guard and the cap derivation; the ladder stays `#[ignore]`d and still never joins
  `cargo test --workspace`.
- **Measured.** Three separate release invocations (`DEDUP_MEASURE_EXPECT=release`,
  `DEDUP_MEASURE_RUNGS=1000,5000,10000`), each detecting `debug_assertions=false`, each printing
  `== end of ladder ==`; no run was killed and the 17.9 s early-stop gate did not fire. Fixture width 768
  lanes, same as 54b. `n = 10_000` ns/pair: 643.4 / 677.1 / 658.9 → **median 658.9**, spread 5.2 %;
  `k = 13,487.9 / 658.9 = 20.470`. `t50_release = 823.6 s` is **INFERRED** (the 50k rung was not run); R3
  is unchanged (823.6 s > 330 s).
- **Decision inputs, no decision.** At `c_iter_release = 658.9 ns` the pre-registered rule yields
  `cap_release = 15,000,000` (9.88 s of a detached blocking task per pass, coverage 1,217 → 5,477
  survivors, 2.4 % → 11.0 % of a 50k shard), while the shipped cap spends 0.49 s. Row 1 of the
  pre-registered table fired. **`MAX_NEAR_SCAN_PAIRS` and its value are untouched**: any diff line touching
  the declaration is a rejection of this wave, and raising it is the owner's decision against
  `T_SCAN_MAX = 10 s`.
- **Record.** Raw runs, the scripts, the aggregation (`derived.txt`), the cap table and the README are
  committed in `docs/evidence/dedup-release-profile-2026-10-26/` (UTC date), not in `.tmp/`.

- **2026-10-28 — owner ruling: raise the cap.** `T_SCAN_MAX = 10 s` is re-affirmed **for the release profile**;
  `MAX_NEAR_SCAN_PAIRS` goes from 740,000 to **15,000,000**. The derivation is the pre-registered rule against
  the measured `c_iter_release = 658.9 ns/pair`: 15,000,000 × 658.9 ns ≈ 9.88 s, coverage 1,217 → 5,477
  survivors (2.4 % → 11.0 % of a 50k shard). The debug figure stays in the record as the conservative input.
  The wave is one step: the constant, the derivation test, and this ruling's record; **no re-measurement**.
