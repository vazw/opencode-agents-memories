# Issue 41 — capture-triggered cleanup tick

Evidence for the automatic retention sweep triggered by an accepted capture, and for
`GET /api/cleanup/status`.

Every figure below is the **verbatim output of a command run in the rework sessions of
`2026-11-17T20:00Z`–`20:30Z`, `2026-11-18T20:45Z`–`21:05Z` and `2026-11-18T21:20Z`–`22:10Z`** on
this host (16 cores, Linux, `x86_64-unknown-linux-gnu`, wild linker). Where a number is not mine
it is labelled **REPORTED**, with its source. Nothing in a verdict cell is arithmetic.

Five corrections to this file, recorded rather than quietly applied:

* its §(d) table carried G2/G3 verdicts for runs that panicked at the G1 assert and therefore
  never executed those asserts. Those cells are now either `not executed (…)` or moved into a
  block explicitly labelled arithmetic. The G1 assert itself is gone — see (d).
* its §(a)/(c) debug rows were release numbers under a debug label (the debug sweep is
  2 700–2 800 ms, not 184 ms). They are replaced by measured debug rows.
* its §(c) mutant rows were recorded without a rebuild check, and one was reproduced by review-2
  as a different mutant on a stale binary. Every mutant row is now produced by the recipe in (c)
  — line selected by `grep`, `cargo clean -p mem-store`, `Compiling mem-store` asserted present —
  and a row without that line counts as unrun.
* its §(c) **wall-clock assertion was the wrong oracle on this host**: it red correct code in 3 of
  7 runs (review-3) and in 1 of 40 more measured here, while `polls_in_sweep` never left 108/108.
  The released gate is now the **poll count**; the wall clock is printed as
  `wall_clock=evidence(not asserted)`. Measured false-red rate of the shipped form: 0 in 55 runs,
  loadavg 1.02–4.44 — a rate, not a single green row, per the recipe in (c).
* its §(c) per-shard figure read "0.046 of the bound" where 0.046 was the share of the **sweep**;
  corrected to both numbers (0.21–0.46 of the bound, 0.046 of the sweep) with the fixture and
  profile named.

Fixtures:

- synthetic: `.tmp/scratch/cleanup-tick-*` (private `HOME`, private storage root);
- live-shape copy A — `.tmp/scratch/live-shape-issue41/data`, copied `2026-11-17T19:54Z`, then
  swept once by the live arm (894 deletes) and finally **every row pinned** for the
  delete-loop-out-of-the-work measurement;
- live-shape copy B — `.tmp/scratch/live-shape-issue41b/data`, copied `2026-11-17T20:21Z`, never
  written, the source of §(b).

The live engine (pid 4719, `127.0.0.1:3742`, `/home/opencode/.opencode-mem/data`) was never
opened for write, signalled, restarted or bound. Every store reading is of a `cp -a`.

---

## (a) The sweep this wave actually costs

| fixture | profile | sweep ms | what runs alongside | run |
|---|---|---|---|---|
| synthetic 36 × 100 | `--release` | **193.3** | nothing — the gap instrument owns the runtime | 20:19Z |
| synthetic 36 × 100 | debug | **2713.8** | nothing | 20:19Z |
| synthetic 1 × 2 000 (fat) | `--release` | **105.4** | nothing | 20:20Z |
| synthetic 1 × 2 000 (fat) | debug | **2154.1** | nothing | 20:20Z |
| live copy, 894 deletes | debug | **1919.4** | nothing | 20:22Z |
| live copy, every row pinned (`deleted=0`) | debug | **542.0** | nothing | 20:23Z |
| synthetic 36 × 100 | `--release` | **429** (`tick_duration_ms`) | 40 sequential captures | 20:15Z |
| synthetic 36 × 100 | `--release` | **539** (`tick_duration_ms`) | 40 sequential captures | 20:16Z |
| synthetic 36 × 100, both `yield_now` removed | `--release` | **422** (`tick_duration_ms`) | 40 sequential captures | 20:17Z |

The isolated 193.3 ms is the release sweep's own cost; the 429/539 ms pair is the same sweep
**sharing** the `current_thread` runtime with 40 captures. The per-shard and per-chunk
`yield_now` hand the executor back between slices, so the sweep's wall clock stretches while any
single capture's wait stays bounded — that is the trade the design makes, and (d) is its gate.

The pinned live copy's 542 ms with **zero deletes** is the floor of a 36-shard sweep on a real
store: `open_shard` + `RETENTION_CANDIDATES_SELECT` per shard, nothing else.

---

## (b) The live store's shape

Derived `2026-11-17T20:21Z` from copy B, read-only. `CUT = now − 30 × 86 400 000 = 1792354870914`.

```sh
# 1. shard count and the registry's own vector total
sqlite3 metadata.db "SELECT count(*)||'|'||sum(vector_count) FROM shards;"
#   -> 36|6604

# 2. per-shard counts (top five)
sqlite3 metadata.db "SELECT group_concat(vector_count) FROM
  (SELECT vector_count FROM shards ORDER BY vector_count DESC LIMIT 5);"
#   -> 2726,1379,564,347,267

# 3. idle rows, summed over the shard files
for f in $(sqlite3 metadata.db "SELECT db_path FROM shards;"); do
  sqlite3 "$f" "SELECT count(*) FROM memories WHERE updated_at < $CUT;"
done
#   -> 894

# 4. expired prompts
sqlite3 user-prompts.db "SELECT count(*) FROM user_prompts WHERE created_at < $CUT;"
#   -> 421

# 5. the grace-exempt subset of those prompts
sqlite3 user-prompts.db \
  "SELECT count(*) FROM user_prompts WHERE created_at < $CUT AND linked_memory_id IS NOT NULL;"
#   -> 368
```

Figure 3 is a pre-grace count: the sweep additionally exempts pinned rows and rows reachable
from the 368 linked prompts. All five figures move with live capture — the same queries twenty
minutes earlier answered `36|6590`, `2717,1379,564,347,267`, `892`, `417`, `366`.

**The two invariants that do not move:** 36 shards, and `sum(vector_count) == count(*)` over the
shard files. The largest shard holds 2 726 of 6 604 vectors — **14.8× the mean** — which is what
makes the live arm's relative bound unreachable (§c).

**`immutable=1` is not a way to read this store.** `sqlite3 "file:…metadata.db?immutable=1"`
answers `16|2233` (the last checkpoint, blind to the `-wal`). Read a copy.

---

## (c) The gap instrument — `crates/mem-store/tests/retention_sweep_gap.rs`

Mechanism: run the real `run_retention_sweep` on a `current_thread` runtime beside a probe task
that yields continuously, and report two quantities from the sweep window:

* **`polls_in_sweep`** — how many probe polls landed inside `started..=ended`. This is the gate.
  It equals the number of explicit `yield_now` calls exactly: 108 (36 shard + 72 chunk), 41
  (1 + 40), 36 on a no-delete 36-shard sweep, 0 without the shard line. It is structural, so it
  does not move with host load, and the yield contract is *exactly* "the sweep parks": a ready
  task is polled once per explicit yield.
* **`max_gap_ms`** and the quiet-host `ratio_bound_ms` — printed as `wall_clock=evidence(not
  asserted)`. Recorded, never a pass/fail gate.

`samples=` is not evidence either and must not be read as it: ~100 000 of those samples are the
probe free-spinning during the 30 ms pre-sweep window (~0.3 µs a poll in release).

### Why the wall clock is not a gate (measured on this host)

| gate form | n | loadavg | reds on correct code | `polls_in_sweep` |
|---|---|---|---|---|
| `max_gap ≤ 0.10 × sweep`, debug + release (r4) | 7 | 1.1–2.6 (review-3) | **3** | 108/108 every run |
| same form, re-inserted as a test-mutant and re-measured here | 20 | 3.00–5.02 | **1** | 108/108 |
| same form | 20 | 1.12–1.22 | 0 | 108/108 |
| `max_gap ≤ max(0.10 × sweep, 100 ms)` (debug-3) | 50 | 1.40–10.66 | 0 (REPORTED) | 108/41 |
| ceiling form, one run measured here | 1 | 8.67 | **1** (`max_gap 185.97` > 100 on a 386.1 ms sweep) | 108 |
| ceiling form (review-4) | 1 | 9.16 | **1** (`max_gap 137.80` > 100 on a 217.9 ms sweep) | 41 |
| **poll count (shipped), this session** | **55** | **1.02–4.44** | **0** | **108/41 every run** |
| **poll count (shipped), review-4** | **21** | **1.55–9.16** | **0** | **108/41** |

Combined, the shipped gate is **0 false reds in 76 runs, loadavg 1.02–9.16**. No ceiling high
enough to clear the two correct-code gaps above can separate the release zero-yield mutant
(`211.75`), so the ceiling is dead in every variant and the column stays printed only.

One shard body is 18–97 ms of CPU work with `run_delay_ms = 0.00` on a tmpfs store, so a loaded
host inflates a *correct* sweep's gap without breaking any yield; the count never moves. My own
single ceiling-form run at loadavg 8.67 (185.97 ms gap on a 2× inflated sweep, `polls_in_sweep=108`)
contradicts debug-3's 0/25 at that load and is why the ceiling is printed rather than asserted:
no wall-clock form survives a host that can deschedule the process wholesale.

**What catches a slow-but-yielding sweep, then?** Not this instrument — its wall clock is
evidence only. `tests/cleanup_tick_latency.rs` G3 (`max(due) <= 150 ms`, behind
`IDLE_HOST_MAX_NOT_DUE_MS = 25`) and the production `cleanup.tick` span's `duration_ms` are the
oracles for absolute cost; a slow sweep here shows up as a larger `max_gap` in the print and a
larger `tick_duration_ms` there, not as a red gap.

Asserted, in full: `deleted == expected`, `sweep_ms >= MIN_SWEEP_MS` (50 ms, load can only
inflate it), and `polls_in_sweep >= min_polls` (36 on 36 shards, 2 on one fat shard). The
blocking control additionally asserts `polls_in_sweep == 0` and a one-sided `max_gap > 0.10 ×
sweep` (load can only help that one).

```sh
cargo test -p mem-store --test retention_sweep_gap -- --test-threads=1 --nocapture     # 6 tests
cargo test --release -p mem-store --test retention_sweep_gap -- --test-threads=1      # 6 tests
```

**Debug, exit 0, 6 passed** (`2026-11-18`, clean rebuild, loadavg 0.95):

```
gap_instrument: fixture=synthetic 1x2000 sweep_ms=2102.9 max_gap_ms=78.85 ratio_bound_ms=210.29 quiet_host_ceiling_ms=100 polls_in_sweep=41 deleted=2000 gate=polls>=2 wall_clock=evidence(not asserted)
gap_instrument: fixture=synthetic 36x100 sweep_ms=2720.0 max_gap_ms=43.57 ratio_bound_ms=272.00 quiet_host_ceiling_ms=100 polls_in_sweep=108 deleted=3600 gate=polls>=36 wall_clock=evidence(not asserted)
gap_instrument: fixture=control-slice yield sweep_ms=180.2 max_gap_ms=5.01 ratio_bound_ms=18.02 polls_in_sweep=36 gate=polls>=36
gap_instrument: fixture=control-blocking sweep_ms=173.1 max_gap_ms=173.09 ratio_bound_ms=17.31 polls_in_sweep=0 gate=polls==0
```

**Release, exit 0, 6 passed** (`2026-11-18`, clean rebuild, loadavg 0.95):

```
gap_instrument: fixture=synthetic 1x2000 sweep_ms=98.3 max_gap_ms=4.28 ratio_bound_ms=9.83 quiet_host_ceiling_ms=100 polls_in_sweep=41 deleted=2000 gate=polls>=2 wall_clock=evidence(not asserted)
gap_instrument: fixture=synthetic 36x100 sweep_ms=178.6 max_gap_ms=2.92 ratio_bound_ms=17.86 quiet_host_ceiling_ms=100 polls_in_sweep=108 deleted=3600 gate=polls>=36 wall_clock=evidence(not asserted)
gap_instrument: fixture=control-slice yield sweep_ms=180.0 max_gap_ms=5.00 ratio_bound_ms=18.00 polls_in_sweep=36 gate=polls>=36
gap_instrument: fixture=control-blocking sweep_ms=173.1 max_gap_ms=173.07 ratio_bound_ms=17.31 polls_in_sweep=0 gate=polls==0
```

### Validating this gate — the recipe, not a claim (recurrence guard)

This wave has now shipped six gate-integrity defects; the two after r3 were *unmeasured*
wall-clock gates. **A single `exit 0` row is not evidence.** Any change to this file's assertions
ships the measurement:

```sh
BIN=$(ls -t target/release/deps/retention_sweep_gap-* | grep -v '\.d$' | head -1)
for i in $(seq 1 10); do cut -d' ' -f1 /proc/loadavg; "$BIN" --test-threads=1 --nocapture; done
```

Record `n`, the loadavg of every run, min/p50/max of `max_gap_ms`, and the red count — plus the
`polls_in_sweep` of every run, because that column is the one that must not move. The same rule
sits above `GAP_CEILING_MS` in the test file, so a future edit that tightens the clock has to
delete a measured sentence to do it.

### The mutant recipe, and why it has a guard

Every mutant row below was produced with this recipe. A row without a `Compiling mem-store` line
is a **stale binary** and counts as unrun — review-2 measured three such runs itself, and the
`cp`+`sed` rewrite preserves mtimes, so cargo happily reuses the previous binary. Select the line
by `grep`, never by a hardcoded line number: this wave has already moved those numbers once.

```sh
python3 - <<'PY'
import pathlib
p = pathlib.Path("crates/mem-store/src/shard_manager.rs")
lines = p.read_text().splitlines(keepends=True)
hits = [i for i, line in enumerate(lines) if "yield_now().await" in line]
assert len(hits) == 2, hits          # shipped = 2
del lines[hits[0]]                    # M-c: hits[0] | M-b: hits[1] | M-a: both
p.write_text("".join(lines))
PY
grep -c 'yield_now().await' crates/mem-store/src/shard_manager.rs   # M-a 0, M-b/M-c 1
cargo clean -p mem-store && cargo clean -p mem-store --release
cargo test --release -p mem-store --test retention_sweep_gap -- --test-threads=1 --nocapture
grep -q 'Compiling mem-store' <log> || echo 'MUTANT NOT BUILT — row invalid'
# restore, then verify BEFORE recording any green row
sha256sum -c <<< "a55cd901bd9cc795f729c2456450442c167d1dceb2d3090e75cb0684acf13a08  crates/mem-store/src/shard_manager.rs"
grep -c 'yield_now().await' crates/mem-store/src/shard_manager.rs   # must read 2
```

### M-a — both `yield_now` removed (chunking kept)

`2026-11-18`, clean rebuild, `Compiling mem-store` present.

```
gap_instrument: fixture=synthetic 1x2000 sweep_ms=97.5 max_gap_ms=97.46 ratio_bound_ms=9.75 quiet_host_ceiling_ms=100 polls_in_sweep=0 deleted=2000 gate=polls>=2 wall_clock=evidence(not asserted)
synthetic 1x2000: the sweep parked a ready task: 0 poll(s) in the sweep window, expected at least 2
gap_instrument: fixture=synthetic 36x100 sweep_ms=173.0 max_gap_ms=173.03 ratio_bound_ms=17.30 quiet_host_ceiling_ms=100 polls_in_sweep=0 deleted=3600 gate=polls>=36 wall_clock=evidence(not asserted)
synthetic 36x100: the sweep parked a ready task: 0 poll(s) in the sweep window, expected at least 36
# exit 101 — 3 passed / 3 failed (both property fixtures + the no-delete test)
```

`polls_in_sweep=0` on every fixture: with no explicit yield the sweep is scheduled once and the
probe never runs until it is over. The recorded gap is ≈1.0 of the sweep (10.0× the ratio bound)
in both profiles — 174.93 / 173.4 release, 2693.65 / 2693.6 debug — but the RED here is the
count, which is why it reds on a loaded host too.

### M-b — the per-chunk `yield_now` removed (fat fixture): the chunking's yield

`2026-11-18`, clean rebuild, `Compiling mem-store` present.

```sh
cargo test --release -p mem-store --test retention_sweep_gap test_a_fat -- --test-threads=1 --nocapture
# exit 101
gap_instrument: fixture=synthetic 1x2000 sweep_ms=97.4 max_gap_ms=97.27 ratio_bound_ms=9.74 quiet_host_ceiling_ms=100 polls_in_sweep=1 deleted=2000 gate=polls>=2 wall_clock=evidence(not asserted)
synthetic 1x2000: the sweep parked a ready task: 1 poll(s) in the sweep window, expected at least 2
```

The 36 × 100 fixture stays **green** under this mutant (`polls_in_sweep=36`, only the shard line) —
exactly as designed: that fixture does not test the chunking. Debug gap for the same mutant:
`2118.03 / 2118.8`, bound `211.88` → ratio 1.000.

Three independent measurements of this mutant agree on the magnitude — `96.02 / 96.2`
(2026-11-17), `96.78 / 97.0` and `96.21 / 96.4` (debug-2 and rework-2, 2026-11-18) — i.e. **≈10×
the bound**, not "9.98×" as a precision claim. Only the *bound* moves with the sweep's wall clock;
the ratio cannot, because the mutant's body is the whole sweep. The count is what pins it now.

Shipped fat fixture for comparison: `4.28 / 98.3` release, `78.85 / 2102.9` debug — both
`polls_in_sweep=41`.

1 000 rows was tried first and rejected: release swept them in 48.5 ms, under the 50 ms
`MIN_SWEEP_MS` floor, so the fixture would have had to lower a gate to pass. The fixture was
scaled up instead of the floor being scaled down.

### M-c — the per-shard `yield_now` removed: pinned by the poll count

`2026-11-18`, clean rebuild, `Compiling mem-store` present. Release **5 passed / 1 failed** — the
one failure is the no-delete test; every other fixture is green, including on the old ratio gate:

```
gap_instrument: fixture=synthetic 1x2000 sweep_ms=96.6 max_gap_ms=4.30 ratio_bound_ms=9.66 quiet_host_ceiling_ms=100 polls_in_sweep=40 deleted=2000 gate=polls>=2 wall_clock=evidence(not asserted)
gap_instrument: fixture=synthetic 36x100 sweep_ms=187.9 max_gap_ms=3.28 ratio_bound_ms=18.79 quiet_host_ceiling_ms=100 polls_in_sweep=72 deleted=3600 gate=polls>=36 wall_clock=evidence(not asserted)
thread 'test_a_sweep_with_nothing_to_delete_still_yields_once_per_shard' (20545) panicked at
crates/mem-store/tests/retention_sweep_gap.rs:391:5:
the sweep parked a ready task for its whole run: 0 poll(s) in a window of 36 shards with nothing
to delete (4.0 ms); the per-shard `yield_now` in `run_retention_sweep` must give one scheduling
point per shard
```

| profile | fixture | sweep ms | max gap ms | ratio bound ms | polls_in_sweep |
|---|---|---|---|---|---|
| release | 36 × 100 | 187.9 | 3.28 | 18.79 | 72 (chunk only, was 108) |
| release | 1 × 2000 | 96.6 | 4.30 | 9.66 | 40 (was 41) |
| release | 36 × 100, nothing to delete | 4.0 | 3.95 | — | **0 (was 36) → RED** |
| debug | 36 × 100 | 2760.9 | 43.48 | 276.09 | 72 |
| debug | 1 × 2000 | 2149.1 | 81.30 | 214.91 | 40 |
| debug | 36 × 100, nothing to delete | 40.2 | 40.22 | — | **0 (was 36) → RED** |

So the line is not decoration and it is not pinned by any ratio: on a sweep that deletes, the
chunk loop already supplies a scheduling point — this run's 36 × 100 gap was **0.17 of the ratio
bound** with the shard line gone — while the older two-line measurement put the worst at **0.21–0.46
of the bound** (0.046 **of the sweep**: `8.21 / 177.4`; review-3 re-measured `3.65 / 17.38 = 0.210`
and `4.17 / 9.53 = 0.437`). Its only job is the **no-delete steady state** — every tick after the
first finds an empty idle population — where it is the only scheduling point, and there the count
is 36 versus 0.

### The live arm is evidence, not a gate

The relative bound cannot bind on an unbalanced store: the granule is one shard, and the largest
shard is 14.8× the mean, so one shard's open + select alone must exceed `0.10 × sweep`. Measured
on copy A with **every row pinned** so the delete loop does no work at all (exit 0):

```
gap_instrument: samples=7999 polls_in_sweep=36 top_gaps_ms=[210.77, 80.87, 43.88]
gap_instrument: fixture=live-copy sweep_ms=584.9 max_gap_ms=210.77 ratio_bound_ms=58.49 deleted=0 polls_in_sweep=36
```

3.60× the ratio bound **with zero deletes** — no delete-loop arrangement can pass it — and
`polls_in_sweep=36` confirms the mechanism on a real store: one scheduling point per shard, and
on this shape (nothing deletable) the per-shard line is the only one that produces them. The arm
prints that ratio and asserts only that the copy was not already swept, so a red assertion cannot
hide behind an env var.

### What is pinned, and by what (measured, 2026-11-18)

| yield | pinned by | evidence |
|---|---|---|
| per-chunk (inside `RETENTION_DELETE_CHUNK` loop) | `test_a_fat_single_shard_is_deleted_in_chunks_not_one_body` | M-b: `polls_in_sweep` 1 < 2 → RED (41 shipped); recorded gap `97.27 > 9.74` |
| per-shard (top of the shard loop) | `test_a_sweep_with_nothing_to_delete_still_yields_once_per_shard` | M-c: `polls_in_sweep` 36 → 0 → RED; every ratio fixture green under the same mutant |

The per-shard yield cannot be gated by a ratio at all — on every fixture that deletes, the chunk
yield masks it — so it is gated by the shape where it is load-bearing. `polls_in_sweep >= SHARDS`
is an inequality on purpose: an upper bound would pin turso's internals, and if turso ever became
truly async the line would be redundant anyway. That is the ceiling this test deliberately does
not encode.

---

## (d) The capture-latency instrument — `tests/cleanup_tick_latency.rs`

36 shards × 100 idle rows, one private root, two child engines. Arm A's `cleanup-sweep.json` says
"just ran", so its tick is a no-op; arm B ages the stamp past 24 h and reuses the same root, so
the first capture's wake sweeps 3 600 rows while 39 more captures are answered. Per-request
milliseconds = wall clock around "write the NDJSON line, read the ACK".

```sh
cargo test --release -p opencode-agents-memory --test cleanup_tick_latency -- --nocapture
```

### The gates, after the rework

* **G2 (the only discriminating gate)** — `max(due) − max(not_due) ≤ 0.5 × tick_duration_ms`.
  A capture that waited on the sweep waits the whole sweep, which is twice the bound.
* **G3 (absolute cap)** — `max(due) ≤ 150 ms`, guarded by an idle-host precondition
  (`max(not_due) ≤ 25 ms`; idle runs measure 8.2–10.8 ms, loaded runs 51.6 and 218.9 ms).
* **The count-above-a-percentile diagnostic is printed, never asserted.** Its shipped form
  (`count(due_i > p95(not_due)) ≤ 1`) red correct code 6/6 — arm A's p95 is a 0.3–0.9 ms idle-jitter
  ceiling against the design's own 1.6–2.8 ms median shift, and `p95` at n = 40 is the
  third-largest sample, so it also greens a single 640 ms delay. A same-run percentile form does
  not repair it: `p95(due)` measured 19.97–159.73 against `p95(not_due) + max(2.0, 0.25·p95(not_due))`
  = 9.4–12.9, red on 6/6 correct runs.

### Pre-fix runs (G1 asserted, `:468`)

These ran before the rework. Every G2/G3 cell is `not executed` because the G1 assert panicked
first — this is the correction to the first version of this file.

| # | arm | n | p50 | p95 | max | tick ms | executed assert and verdict |
|---|---|---|---|---|---|---|---|
| 1 | not_due | 40 | 8.20 | 8.58 | 10.29 | not recorded | — |
| 1 | due | 40 | 10.15 | 40.99 | 109.94 | not recorded | G1 panicked (25/40); G2, G3 not executed |
| 2 | not_due | 40 | 17.19 | 37.38 | 51.62 | not recorded | — |
| 2 | due | 40 | 21.83 | 77.04 | 195.67 | not recorded | G1 panicked (9/40); G2, G3 not executed |
| 3 | not_due | 40 | 7.38 | 7.83 | 10.29 | not recorded | — |
| 3 | due | 40 | 9.29 | 19.97 | 22.53 | not recorded | G1 panicked (25/40); G2, G3 not executed |
| 4 | not_due | 40 | 7.41 | 7.81 | 8.75 | — | — |
| 4 | due | 40 | 12.16 | 159.73 | 218.58 | 1271 | G1 panicked (25/40); G2, G3 not executed |
| 5 | not_due | 40 | 7.52 | 8.80 | 9.79 | — | — |
| 5 | due | 40 | 10.32 | 40.25 | 138.31 | 589 | G1 panicked (25/40); G2, G3 not executed |
| M | not_due | 40 | 7.56 | 7.95 | 8.16 | — | — |
| M | due | 40 | 0.32 | 2.87 | 647.88 | 640 | **G2 executed and RED** (639.72 > 320.00); G3 not executed |

<details><summary>Computed from that run's pre-gate print — arithmetic, not assertions</summary>

`max_due − max_not_due`, against `tick_duration_ms / 2`:

```
run 4: 218.58 − 8.75 = 209.83 ; 1271 / 2 = 635.50   (G2 would have held)
run 5: 138.31 − 9.79 = 128.52 ;  589 / 2 = 294.50   (G2 would have held)
run M: 647.88 − 8.16 = 639.72 ;  640 / 2 = 320.00   (G2 red — and it DID execute, see row M)
run 2: G3 alone: max(due) 195.67 > 150               (would have been red on host load)
```

Runs 1–3 have no tick value in the record, so nothing about their G2/G3 can be derived; their
cells stay empty.

</details>

### Post-fix runs (shipped code, `20:15Z`–`21:05Z`)

All four exited **0**, so every assert in the body executed and held.

| # | arm | n | p50 | p95 | max |
|---|---|---|---|---|---|
| 1 | not_due | 40 | 7.45 | 8.26 | 8.85 |
| 1 | due | 40 | 10.33 | 22.07 | 36.39 |
| 2 | not_due | 40 | 7.81 | 8.74 | 10.76 |
| 2 | due | 40 | 12.45 | 33.33 | 39.65 |
| 3 | not_due | 40 | 8.04 | 8.48 | 9.06 |
| 3 | due | 40 | 11.63 | 34.57 | 84.43 |
| 4 | not_due | 40 | 8.27 | 8.80 | 10.06 |
| 4 | due | 40 | 9.31 | 21.22 | 30.27 |

Executed verdicts, printed by the instrument itself:

```
run 1: tick_duration_ms=429 deleted_count=3600 max_delay_excess_ms=27.54 g2_bound_ms=214.50
       diag_g1_outliers=25/40 p95_not_due_ms=8.26          -> exit 0
run 2: tick_duration_ms=539 deleted_count=3600 max_delay_excess_ms=28.89 g2_bound_ms=269.50
       diag_g1_outliers=25/40 p95_not_due_ms=8.74          -> exit 0
run 3: tick_duration_ms=595 deleted_count=3600 max_delay_excess_ms=75.37 g2_bound_ms=297.50
       diag_g1_outliers=25/40 p95_not_due_ms=8.48          -> exit 0
run 4: tick_duration_ms=389 deleted_count=3600 max_delay_excess_ms=20.21 g2_bound_ms=194.50
       diag_g1_outliers=25/40 p95_not_due_ms=8.80          -> exit 0   (2026-11-18, loadavg 1.93)
```

* G2 executed and held: `27.54 ≤ 214.50`, `28.89 ≤ 269.50`, `75.37 ≤ 297.50`, `20.21 ≤ 194.50`.
* idle-host precondition executed and held: `8.85 ≤ 25`, `10.76 ≤ 25`, `9.06 ≤ 25`, `10.06 ≤ 25`.
* G3 executed and held: `36.39 ≤ 150`, `39.65 ≤ 150`, `84.43 ≤ 150`, `30.27 ≤ 150`.
* diagnostic (not a gate): `25/40` all four runs — the count that used to fail the suite now
  simply reports how many captures the design perturbed at all.

The per-shard `yield_now` is not this suite's subject: with it removed G2 still held
(`55.43 ≤ 245.50`) and G3 too (`64.44 ≤ 150`) — see (c) M-c; that line is pinned by the poll
count, not by wall clock.

### G3's host sensitivity, measured (one RED on correct code)

`20:24Z`, loadavg `3.93` (another wave's build in this tree), shipped code, **exit 101**:

```
not_due, 40, 8.19, 8.51, 8.58
due, 40, 10.24, 61.23, 210.75
tick_duration_ms=749 deleted_count=3600 max_delay_excess_ms=202.16 g2_bound_ms=374.50
thread 'cleanup_tick_latency' panicked at tests/cleanup_tick_latency.rs:512:5:
G3: max(due) = 210.75 ms exceeds the 150 ms cap
```

G2 held (`202.16 ≤ 374.50`) and the idle-host precondition passed (`8.58 ≤ 25`) because the
load arrived **during arm B**: arm A's max was quiet at 8.58 ms while arm B took a 210 ms spike.
The precondition therefore catches a busy host, not a busy moment — a residue of G3's absolute
shape, recorded here rather than papered over. `20:26Z` on the same tree at loadavg `1.63`, the
same command exited 0: `not_due 8.04/8.48/9.06`, `due 11.63/34.57/84.43`, `tick=595`,
`excess 75.37 ≤ 297.50`, `G3 84.43 ≤ 150`. Making G3 relative to the same-run floor
(`max(due) ≤ max(not_due) + 150`) is the next wave's call.

### Mutant — both `yield_now` removed (`20:17Z`)

```sh
cargo test --release -p opencode-agents-memory --test cleanup_tick_latency -- --nocapture
# exit 101
not_due, 40, 7.68, 9.97, 11.34
due, 40, 0.31, 2.92, 427.66
tick_duration_ms=422 deleted_count=3600 max_delay_excess_ms=416.31 g2_bound_ms=211.00
thread 'cleanup_tick_latency' panicked at tests/cleanup_tick_latency.rs:492:5:
G2: max(due) - max(not_due) = 416.31 ms exceeds half the 422 ms tick — a capture waited on the sweep
```

The panic frame is the **G2** assert at `:492`, and `diag_g1_outliers` did not print — the
diagnostic sits after the gates, where a red is always the gate that means something. Restored
from `.tmp/tmp/sm-shipped.rs` immediately (`grep -c MUTANT` → 0, byte-identical).

---

## Deviations from upstream

1. **`autoCleanupEnabled` defaults to `false`; upstream defaults to `true`.** An agent-memory
   engine that deletes rows on its own the first time it is used is not a safe default for a
   store holding a user's history. The manual `POST /api/cleanup` is unaffected, so upstream's
   behaviour is one key away.
2. **`autoCleanupInterval` is a config key; upstream hardcodes 24 h.** The grammar is
   `Nh`/`Nm`/`Ns` groups summed (`"2h30m"` = 9 000 s), unknown grammar is rejected under the
   key's name, and a project-level config cannot set it.
3. **`shard_manager.rs` gains chunked deletes plus the per-shard yield.** The blueprint's
   one-statement fence and its own conditional-escalation clause contradict each other; the
   escalation's trigger was met, and the chunking is now pinned by the fat-shard test in (c)
   (`polls_in_sweep` 1 < 2 with the per-chunk line removed). Accepted cost: one
   `sync_count_statement` per `delete_memories` call, i.e. 2 count syncs per 100-row fixture
   shard (72 per sweep vs 36) for ≈ +7 % synthetic sweep time; `delete_memories_chunked` is the
   upgrade path, out of scope here. The **per-shard** yield is not gated by any wall clock and
   cannot be — on every fixture that deletes, the chunk loop already supplies a scheduling point.
   It is gated by `test_a_sweep_with_nothing_to_delete_still_yields_once_per_shard`, a poll-count
   test on the no-delete steady state (36 polls shipped, 0 without the line) — see (c) M-c.
4. **`CleanupRunError` has a third variant, `Failed(PortError)`.** The blueprint's literal
   `{ Busy, Unavailable }` cannot carry a port error, and `run_sweep` would then lose the
   byte-identical answers `cleanup()` already ships.
5. **The live-shape gap arm is evidence-only.** Its bound is unreachable on an unbalanced store
   (3.60× over with zero deletes). It prints; it asserts only that the copy was not already
   swept.

No ADR: a default value, one config key, an internal chunk constant and a test-scope decision
are not architectural rulings.
