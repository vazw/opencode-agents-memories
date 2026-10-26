# 54b — the near-duplicate scan is bounded and offloaded

Status: done — 2026-10-26. Landed as **`423f537`** (`fix(mem-web): bound and offload the
deduplication near-duplicate scan (issue 54b)`) on branch `issue-54b/dedup-bounding`, base `efd654b`
(= main's `df5d29e` + issue 54's approved-but-unmerged wiring). This record and issue 54's
reconciliation follow in the record-only commit that names `423f537`; issue 54's `## Comments` carries
the same landing claim.

Issue 54 made `POST /api/deduplicate` reachable. This wave makes it safe to be reachable: the
near-duplicate scan is measured, bounded, moved off the runtime thread, cancellable before engine
shutdown, and the report says honestly what the bound did.

## The defect

`DedupService::run()` deleted each shard's exact duplicates (awaited), then called
`engine::near_groups(..)` **synchronously on the engine's single `current_thread` tokio runtime** —
no await, no yield, no `spawn_blocking`. One `cosine_similarity` per ordered survivor pair, both norms
recomputed per pair, no work bound above `max_vectors_per_shard = 50_000` per shard and none at all on
shard count. The 330 s `TimeoutLayer` is a task on the same blocked thread, so it cannot fire: the
endpoint could not answer a full-shard pass, and shutdown could not interrupt one.

## Measurement (Step 1)

Artifacts: `.tmp/measure/20261026-issue-54b-scan-ladder.txt` (this worktree; the named artifact) and
`.tmp/measure/raw-ladder.txt` (the harness's raw stdout). Harness:
`crates/mem-web/tests/dedup_scan_measure.rs` — one `#[ignore]`d `#[test]` over the **real** kernel and
real `DedupMemory` rows, timed with `std::time::Instant`, asserting no timing.

Harness metadata, verbatim:

```
cargo profile            : debug (cargo test without --release)
lscpu model              : Architecture:                            x86_64
rustc                    : rustc 1.98.0 (88d9e12ae 2026-08-18)
DEFAULTS.embedding_model : Xenova/nomic-embed-text-v1
DEFAULTS.embedding_dimensions (table) : 768
embedding_dimensions_for_model(default) : 768  (used for the fixtures)
threshold                : 0.9
```

The vector width was re-derived from config resolution rather than assumed. The `lscpu` line is the
first line of `lscpu`'s stdout (`Architecture: x86_64`) — the harness prints the first non-empty line;
the machine model itself is not captured, which is a limitation of this record.

The ladder, verbatim (each row printed as its rung completed):

```
      n      comparisons       wall_s  ns/comparison   exponent  exact_plan_ms
    100             4950        0.073        14801.0          -          3.668
   1000           499500        6.507        13026.3      0.972          4.083
   5000         12497500      146.553        11726.6      0.967         18.295
  10000         49995000      674.326        13487.9      1.101         31.329
```

**Rungs not measured.** The harness died during the `n = 25_000` rung (a background process, not a
test failure); `n = 25_000` and `n = 50_000` were never completed, and the `50_000` rung is conditional
on the `25_000` rung finishing inside five minutes in any case. Their costs are **INFERRED** from
`c_iter` and the pair counts, and are labelled so: `t25 ≈ 312,487,500 × 13.4879 µs = 4,215 s ≈ 70 min`,
`t50 ≈ 1,249,975,000 × 13.4879 µs = 16,859 s ≈ 4.68 h`. The pre-registered rule takes `c_iter` at the
**largest measured rung**, which is `n = 10_000`.

### Decision rule — R3

```
c_iter            = 674.326 s / 49,995,000 comparisons   = 13,487.9 ns (13.49 µs)
t50 (one 50k shard) = 1,249,975,000 × 13.4879 µs           = 16,859 s ≈ 4.68 h   → R3 (INFERRED t50)
budget (10 s / c_iter)                                     = 741,405 pairs
floor to two significant figures                           = 740,000 pairs
MAX_NEAR_SCAN_PAIRS = min(max(250,000, 740,000), 1,250,000,000) = 740,000
```

**Branch: R3** — `t50 > 330 s`, by a factor of ≈51. Robust to the debug/release difference: even a
40× faster release build leaves `t50 ≈ 421 s`, still R3. The magnitude the finding lacked: in the
measured debug profile a full 50,000-row shard's scan takes ≈4.68 h, so the pre-fix endpoint could
never have answered a full-shard pass — the timeout that was supposed to bound it was itself starved
on the same thread. Release was **not** measured.

**Validity gate: not entered.** `c_iter` is 13.49 µs, four orders of magnitude above the ≈1 ns that
would mean the harness was not measuring cosines; the `R1` branch (a broken fixture) did not fire, so
no step was skipped and the constant is the R3-derived value, not the ceiling.

### Kernel the ladder measured

The ladder's binary was built and launched before any edit to `engine.rs` (the `HashSet` kernel and the
`NearScanLimits` argument landed later), so the recorded ladder measured the **pre-change kernel** (the
`Vec<&str>` membership version). Independently of that ordering, the ladder fixture never claims a
candidate — the harness asserts no group is reported at every rung, and it observed exactly that — so
`processed` stays empty and the `Vec`→`HashSet` change cannot move `c_iter` at all; the conservative
direction holds trivially rather than by luck. The claimed-set skip path, which is the one number that
*does* measure `Vec` membership, is priced separately below.

### Kernel identity — the base-kernel cross-check (measured)

The ladder's binary was built and launched before any edit to `engine.rs`, so it ran the pre-change
kernel. Rather than rest that on ordering alone, the `n = 10,000` rung was **re-measured against the
base kernel** (`git stash` of the tree's tracked changes, i.e. `efd654b`'s `engine.rs` with the
`Vec<&str>` membership and the 3-arg signature), verbatim:

```
      n      comparisons       wall_s  ns/comparison   exponent  exact_plan_ms
  10000         49995000      570.107        11403.3          -         50.440

tag rejection (no cosine)  :      0.474 s        38.0 ns/comparison  (12497500 comparisons)
claimed-set growth         :      0.289 s        23.1 ns/comparison  (the Vec<&str> membership scan)

c_iter                      : 11403.3 ns per candidate comparison
t50 = c_iter x 1,249,975,000: 14253.8 s (INFERRED)
branch                      : R3
floor2(10 s / c_iter)       : 870000
MAX_NEAR_SCAN_PAIRS         : 870000
```

**What it settles, and what it does not.** The base kernel is *faster* on the same rung (11,403.3 ns vs
13,487.9 ns, −15.5 %), so the ladder's number is the more conservative input and the constant is derived
from it. The spread is run-to-run variation, and **its cause is documented**: the ladder was executed by
invoking the compiled test binary *directly, outside the cargo serialiser*, precisely so the shared build
lock was not held for an hour while the sibling 45b wave built — which means the machine was concurrently
loaded. The record's own table corroborates it: `exact_plan_ms` differs **31.3 ms vs 50.4 ms (61 %)**
between the two runs of the *same code*. Limits, stated rather than implied: n = 2, no load or thermal
control, and no machine model recorded (`lscpu` printed only `Architecture: x86_64`). **Consequence:** the
spread moves the cap's *second* significant figure (740,000 from Run A vs 870,000 from Run B), so on this
machine the constant is reproducible to about one significant figure even though the pre-registered rule
quotes two. Run A's value is used because it is both the pre-registered ladder and the more conservative
bound.

The cross-check does **not** establish kernel identity by itself: both runs are the *same* kernel under
the ordering premise, so what it measures is run-to-run spread, not a kernel effect. The `Vec`/`HashSet`
question is settled independently — `processed` is only inserted on a claim, and the ladder fixture
claims nothing (the harness asserts no group at every rung), so the set stays empty and the change cannot
move `c_iter`. Kernel identity therefore rests on the build-order argument: the ladder's binary was built
and launched at 08:45–08:59Z, before `engine.rs`'s first edit (~09:16Z). That is corroborated (the raw
artifact's mtime is 08:59:08Z; the edits are later) but **not machine-proven**, and the record does not
claim more.


### Skip paths (n = 5,000)

```
tag rejection (no cosine)  :      0.486 s        38.9 ns/comparison  (12,497,500 comparisons)
claimed-set growth         :      0.294 s        23.5 ns/comparison  (the Vec<&str> membership scan)
```

A tag rejection costs ≈39 ns where a cosine costs ≈13.5 µs — 350× more — which is why the budget counts
**candidate comparisons**, not cosines: a fixture that skips every cosine still costs real time, and the
claimed-set path grows with the data.

### The measured consequence, stated plainly

740,000 candidate comparisons buys coverage of about `sqrt(2 × 740,000) ≈ 1,217` survivors **per pass,
across all shards in walk order**. On a 50,000-row shard that is ≈2.4 % of the rows; every larger pass
reports `truncated: true` and reports no near groups for the shards past the cut. That is the measured
consequence of the pre-registered `T_SCAN_MAX = 10 s`, and it is the intended trade (see the A/B
record): the exact-delete half still completes on every shard, and the report is labelled rather than
absent. `T_SCAN_MAX` was **not** changed to flatter the number — it was pre-registered before the
measurement precisely so it could not be tuned afterwards; a later change is a one-line edit with its
own recorded rationale.

### Profile caveat

The ladder ran in the **debug** profile (`cargo test` without `--release`; the artifact prints
`cargo profile: debug`), which is the profile the default suite — and therefore AC2's 0.1 s sizing —
actually runs in. The constant is therefore derived in debug and is **conservative against release**:
a release build is faster, so the same 740,000-comparison budget buys less than 10 s there. It was not
derived at production speed, and the ≈4.68 h figure is a debug-profile figure.

## The cap's value, and where it lives

`MAX_NEAR_SCAN_PAIRS = 740_000` (crate-visible constant,
`crates/mem-web/src/api/deduplication/mod.rs`), derived as
`min(max(250_000, floor_to_two_significant_figures(10 s / c_iter)), 1_250_000_000)`. The floor only
guards a broken measurement; the ceiling is one shard's whole pair grid rounded up from 1,249,975,000.
With the R3 value the derived number is far below the ceiling and **the cap binds**: a single shard at
capacity (1,249,975,000 pairs) is truncated, and a multi-shard pass reaches the budget even sooner. The
ceiling's own truth, for the record: it is a *global* per-pass budget and shard count is unbounded (no
`max_shards` key exists), so a pass accumulating more than one shard's grid can still reach it — the
ceiling is not a per-shard exemption. The record never says "cap skipped" or "truncation impossible".

**One mid-flight figure was rejected.** The orchestrator's correction of 10:01Z said 741,000; that is a
*three*-significant-figure truncation of 741,405, while the pre-registered rule specifies **two**
significant figures (740,000). The implementation and this record's derivation were right and the
mid-flight figure was wrong: the contradiction came from that message, not from the code, and the
harness itself prints `740000`. 741,000 is kept here as the rejected value so no later reader
re-derives it as a live alternative.

### Blueprint corrections (the approved body is immutable)

The approved blueprint's parenthetical at `:174-175` asserts that the R1 validity-gate defect was
*"corrected here, before any gate saw it"*. That is **false** — the previous revision *was* gated (it was
rejected, with ten modifications). The blueprint body cannot be edited after approval (its hash is the
STAMP), so the correction is recorded here instead: R1 is a **validity gate, not a design branch**, it is
reachable only through a broken measurement, and no step is ever skipped on the strength of it.

### Why a constant and not a config key

1. This is a safety bound, not a feature knob — an operator free to raise it re-arms DEDUP-001.
2. A key in the resolved config is a *wire* surface this port mirrors from upstream's schema (issue 49
   owns the resolved-key set and the TS port carries the same schema), so "one more key" is a
   compatibility commitment for a number derived from one machine's measurement.
3. `max_memories` is parsed with no consumer and is a recall limit — repurposing it would be a silent
   semantic lie — and `max_vectors_per_shard` is a per-shard *write* cap, not a global work budget,
   since shard count is unbounded.
4. A constant whose derivation sits in its doc comment is a one-line change after a re-measure, and the
   test seam is the explicit `NearScanLimits` argument, never environment or config.

## CAP SEMANTICS A/B — the loser is kept

The bound is shared; what the pass *does* when its projected scan work exceeds `MAX_NEAR_SCAN_PAIRS`
was a design choice. The ordering fact that decides whether option A is coherent: in
`DedupService::run()` the exact deletions are awaited and committed **before** the near scan runs, and
`plan.survivors` is computed before the delete loop.

### Option A — refuse above the cap (rejected)

Placement (the coherent variant): before the delete loop of the **first** shard, from a pass-level
projection of `sum over shards of pairs(plan.survivors.len())`, compared against
`MAX_NEAR_SCAN_PAIRS`. Above the cap the request answers `Err(PortError::Invalid(msg))`, which the
handler maps to the existing typed business envelope — HTTP **200**, `{"success":false,"error":<msg>}` —
the same shape the `Deduplication is disabled in config` and `Deduplication already running` arms use.
Nothing is mutated and no report is produced.

Rejected variant, recorded so it is not re-proposed: refusing **after** the deletes. The mutation has
then already landed and no scan runs at all, so the error envelope would have to carry a *deletion
count* ("deleted 412 exact duplicates, then declined the scan") — an error reporting a committed write.
Neither a refusal nor a report; not adopted.

- **Pros:** no new report key and no REST change; no partial report a consumer could misread; the
  refusal (coherent variant) is mutation-free; reuses a documented envelope.
- **Cons:** (1) **terminal** — a store above the cap gets no near-duplicate reporting at all, and in the
  coherent variant no exact deduplication either, on precisely the stores that accumulate the most
  duplicates; (2) the projection is expensive: `DedupStore` declares only `shards`, `memories`,
  `delete`, and `ShardDedupStore.shards` is private, so a **two-pass walk** implements it *inside* the
  trait shape — at the cost of holding every shard's rows at once (≈150 MB for one 50k-row 768-lane
  shard) or a second full `memories()` pass per request; (3) a second place that knows the cost model
  must stay in step with the kernel's counting; (4) no in-product recourse (no parameter to page or
  scope a pass, and no config knob by decision above).
- **Predicted outcome:** under R3 a full shard's grid alone is 1,249,975,000 comparisons, far above
  740,000, so **every** store at capacity is refused — the endpoint would be dead exactly where it is
  most needed.
- **Observation that would refute it:** real deployments stay small enough that no pass reaches the
  budget. That is a *deployment-size* observation, not a timing one, and it is narrower than it looks:
  at R3 the cap binds on any pass with more than ≈1,217 survivors, i.e. on essentially every real
  store, so A would refuse them.

### Option B — bounded prefix with reported truncation (adopted)

One global budget consumed in walk order, the exact-delete half complete on every shard, and
`nearDuplicateScan { pairsScanned, pairBudget, truncated }` always present.

- **Pros:** the mutating half is complete on every shard, so the storage contract is preserved at any
  store size; the endpoint stays usable and its report stays truthful via one additive key; the
  truncation point is deterministic (walk order plus comparison count), so equal rows give byte-identical
  reports; no pre-count, no store-seam change, no up-front all-shard memory.
- **Cons:** the near-duplicate report is **partial evidence** a consumer must read `truncated` to
  interpret — a client that ignores the key cannot tell "no near duplicates here" from "did not look";
  the cut point is deterministic but arbitrary from the operator's point of view (earlier shards get
  full coverage, later ones none); it adds one key to the REST surface (safe: the contract suites pin no
  200 dedup shape).
- **Predicted outcome:** at R3 the budget covers ≈1,217 survivors per pass, so a large store gets full
  coverage of its first shards, none of its last, and `truncated: true`.
- **Observation that would refute it:** a consumer that acts on the report as if it were whole-store
  evidence (e.g. an automatic merge that deletes on it). That consumer does not exist in this repo and
  cannot: the near pass deletes, merges and re-embeds nothing, so a partial report is inert unless a
  human reads it.

### The shared discriminating measurement — B's arm, measured

Two shards, a fake store, an injected budget via `with_near_pair_budget`: shard 1 holds one
byte-identical pair plus distinct survivors; shard 2 holds enough survivors that the remaining budget
cannot cover it. Observed (test
`test_dedup_run_reports_truncation_and_still_deletes_the_first_shards_exact_duplicates`,
`crates/mem-web/tests/dedup_service.rs`, a default-suite test): `Ok` with a report; shard 1's older
duplicate **gone**; `exactDuplicatesDeleted == 1`; `nearDuplicateScan.truncated == true`;
`pairsScanned == 10 == pairBudget`.

Only **B's arm is executable** in this tree: A was never implemented, so its column above is a reasoned
prediction read off the code paths, never a measured outcome, and this record does not claim otherwise.
B's outcome preserves the storage contract *and* keeps a usable, honestly-labelled report at a size
where A produces nothing. If B's run had left the duplicate present, B would have failed its own AC6 and
A would be the honest semantics — so the measurement is also the guard on the implementation.

**Verdict: B.** A is kept as the recorded loser, with both placements, the cost argument and the note
that it was never measured, so the next wave does not re-derive it.

## Offload, lease, cancellation

- **Offload.** The near scan moves to `tokio::task::spawn_blocking` over `Arc<Vec<DedupMemory>>` plus the
  owned survivor indices; the store legs stay awaited on the runtime. `JoinError` (cancelled or
  panicked) maps to `PortError::Store`, never a panic, and never a silent success.
- **Run-guard lease.** `DedupService::running` is an `AtomicUsize` count; `RunGuard::acquire` admits only
  `0 -> 1` with the same upstream message. `ScanLease::acquire` takes a second count **before**
  `spawn_blocking` and moves it into the closure. The reason inside-construction is forbidden is the
  **admission window**: a request future dropped after `spawn_blocking` returns but before the closure's
  first instruction would release the async guard and leave the count at zero while a scan is queued,
  admitting an overlapping pass. (A dropped blocking task drops its captures, and a lease built inside
  the closure would increment nothing — neither is the reason.) A pass is running while the count is
  non-zero, and the last holder frees admission.
- **Cancellation.** `production_deduplication_port_with_cancel` shares one
  `Arc<AtomicBool>` with the scan; `src/main.rs` sets it as the **first** statement of the stop closure,
  before `handle.stop().await`. The scan re-reads it at every outer iteration and every
  `NEAR_SCAN_CHECK_STRIDE = 1024` candidate comparisons, returns `cancelled`, and the pass answers
  `Err(PortError::Store("deduplication scan cancelled"))` rather than a partial report (D-9 keeps the
  detail out of the response). The guard and lease still unwind.
- **Kernel cost.** `processed` is a `HashSet<&str>` — membership only, never iterated, so output order
  still comes from the input order — which removes the super-quadratic membership scan from the pair
  loop. The budget counts candidate comparisons, so the bound is a true work bound.

## Accepted residual

- The "runtime drop waits for blocking tasks" leg of the shutdown story is READ from the tokio
  documentation, not re-measured here. The flag mechanism is proven at the mem-web seams
  (`dedup_offload.rs`, `dedup_service.rs`), not by an end-to-end SIGTERM timing. If a reviewer wants it
  measured, the cheapest honest probe is a one-off SIGTERM timing against the built binary with a
  budget-sized shard, recorded here — not a new test binary.
- With the runtime already gone, a scan would run to its budget (at most `T_SCAN_MAX`) before exiting;
  the flag is the mechanism that keeps the exit inside the shutdown budget, and it is set before the
  server stops by construction.
- The `JoinError -> PortError::Store` arm is mutation-only: reaching it needs a panicking or cancelled
  blocking task, and ADR-0013 forbids `panic!` in production code. It is not covered by a test, by
  design.
- **The linear leg still runs on the runtime thread.** `engine::exact_plan` is cheap but not free — the
  ladder measured 31–50 ms at `n = 10,000` in debug, which extrapolates to ~150–250 ms for a 50,000-row
  shard (the extrapolation is INFERRED; the rung was not run). It is a `HashMap` grouping pass, not a
  pair loop, so it is not the wedge this wave removes; it stays a named residual rather than a silently
  accepted cost.

## Files

`crates/mem-web/src/api/deduplication/{engine,mod}.rs`, `crates/mem-web/src/api/{ports,dto}.rs`,
`crates/mem-web/tests/{dedup_scan_bound,dedup_offload,dedup_scan_measure,dedup_grouping,dedup_service,dedup_endpoint}.rs`,
`crates/mem-web/CONTEXT.md`, `src/main.rs`, `tests/production_api_wiring.rs`,
`docs/web-skeleton-smoke.md`, `docs/agents/testing.md`, this file, and issue 54's file.
No `Cargo.toml`/`Cargo.lock` change, no new dependency, no config key.
