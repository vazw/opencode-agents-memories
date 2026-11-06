# Issue 129 — `/api/stats` timing breakdown (Step 2)

Status: **MEASURED**, measurement only. No fix implemented in this wave (Step 2 gate:
"if fix selected, STOP for scoped design … no fix authorized without new plan").

- Tree: worktree `.tmp/worktrees/issue-129`, base `eda2efd` (branch `issue-129/wave-residuals`), unmodified.
- Measurement build: throwaway worktree `<worktree>/.tmp/scratch/stats-measure` (detached at `eda2efd`,
  gitignored), timing instrumentation only. **No instrumentation landed in the shard tree.**
- Harness: `#[ignore]` test `measure_stats_breakdown` in that scratch copy, driven through the real
  HTTP seam against a real child engine (`spawn_engine` from `tests/production_api_wiring.rs`), token
  from `.auth-token`, 1 warm-up request then 7 measured requests, plus 7 `?probe=1` requests.
- Raw logs: `.tmp/progress/r9-shard-A/stats-run.log` (client + handler spans),
  `stats-run2.log` (adapter `open`/`query`/`map` split), `stats-run3.log` (query-variant A/B).
  Handler spans are written by the existing log facade (`mem_core::logging::log`) into the child's
  `opencode-mem.log`, i.e. **spans emit elapsed times**, not just creation.

## Instrumentation (scratch only)

| segment | instrumented at | how |
| --- | --- | --- |
| discovery (`state.api.memory.shards(None,None)`) | `mem-web/src/api/stats.rs::stats` | `Instant` around the await |
| aggregation (`collect_shard_stats`) | same | `Instant` around the call |
| adapter open / SQL / row mapping | `mem-web/src/api/store_adapter.rs::all_memories` | three `Instant`s, per shard |
| daily-log scan | `stats.rs` around `collect_daily_stats` | `Instant` |
| profile work | `stats.rs` around `collect_profile_stats` | `Instant` |
| serialization | `stats.rs` around `StatsDto` build + `envelope::ok` | `Instant` |
| unaccounted remainder | derived: `wall_us − (discovery+aggregation+daily+profile+serialize)` | arithmetic |

## Fixtures (disposable roots, exclusive ownership)

- **populated**: fresh root, **2000 distinct seeded memories in ONE project shard**
  (`seed_rows`, distinct content/vector per row, `vector_with(dims,[(i%64,1.0),(64+i%7,0.5)])`),
  registry count adopted. Returned `total` asserted `2000` on every measured request — the seed is
  real, not declared.
- **empty**: fresh root, no shards. `total` asserted `0`.
- Both: real child engine, real `dist` shell, real token gate. Load normalized by one warm-up request.

## Populated root — 2000 rows, one shard (µs)

Client wall / server handler `wall_us`, 7 requests after warm-up:

```
client_wall_us: 53460, 54468, 55044, 53867, 54406, 53565  (+ warm-up 54652)
```

| segment | min | median | max |
| --- | --- | --- | --- |
| handler wall | 52 509 | 53 357 | 53 987 |
| discovery | 1 582 | 1 642 | 1 956 |
| aggregation (total) | 49 299 | 50 161 | 50 318 |
| — adapter open (cached after 1st) | 21 | 22 | 25 |
| — SQL `GET_ALL_MEMORIES` (`SELECT * … ORDER BY created_at DESC`) | 40 729 | 41 426 | 41 680 |
| — row mapping (2000 rows) | 5 728 | 5 983 | 6 085 |
| daily-log scan (14-day, small log) | 254 | 302 | 377 |
| profile work | 1 087 | 1 120 | 1 242 |
| serialization | 156 | 159 | 164 |
| unaccounted remainder | ~1 900 | ~2 600 | ~3 400 |

Cold first request: wall 145 662 µs (discovery 5 638, open 135 991 — first shard open + migration).

## Empty root — churn floor (µs)

| segment | min | median | max |
| --- | --- | --- | --- |
| handler wall | 2 277 | 2 339 | 2 388 (cold first: 31 581) |
| discovery | 1 306 | 1 352 | 1 448 |
| aggregation | 1 | 2 | 5 |
| daily-log scan | 181 | 212 | 282 |
| profile work | 600 | 651 | 667 |
| serialization | 102 | 106 | 119 |

Probe (`?probe=1`, answers before any walk): 507–753 µs, populated and empty alike.

Noise: populated wall spread 52.5–54.0 ms (±1.4 %); empty 2.28–2.39 ms (±2.4 %). Same values with the
box otherwise idle; one earlier run under load showed the known `terminal_deadletter` flake, not stats.

## Candidate A vs B

The walk needs EVERY row (scope counts, type tallies, oldest/newest, tag-duplicate grouping), so
`COUNT(*)` cannot answer it: measured `count_only` = 0.8–1.2 ms but carries no per-row data.

**A — targeted fix: narrow projection, no ORDER BY, for the stats walk only.** Stats uses only
`container_tag`, `type`, `created_at`, `tags`; the current `SELECT *` ships `content`, the 768-float
`vector` blob and every metadata column, and sorts by `created_at DESC` for a caller that ignores order.

**B — retain loading, document the cost.** No code change; the endpoint stays a full-shard walk.

Shared discriminating measurement (same fixture, same db handle, same process, back-to-back):

| query variant | min | median | max | vs current |
| --- | --- | --- | --- | --- |
| `SELECT * … ORDER BY created_at DESC` (current) | 40 896 | 41 448 | 56 053 | 1.00× |
| `SELECT container_tag,type,created_at,tags … ORDER BY created_at DESC` | 26 593 | 27 174 | 27 651 | 1.53× faster |
| `SELECT container_tag,type,created_at,tags` (no ORDER BY) | 18 795 | 20 133 | 20 766 | **2.06× faster** |
| `SELECT COUNT(*)` | 800 | 1 113 | 1 211 | (cannot answer stats) |

Refutation of the prediction "the projection is the dominant defect": if `narrow_noorder` were within
noise of `wide_order`, the cost would be row materialization and nothing else.

**Result: A wins on the measurement** — projection + dropped sort remove ~21 ms of ~53 ms handler wall
(about −40 % end-to-end on a 2000-row single shard), and the remainder (~20 ms SQL for 2000 rows plus
~6 ms mapping) is turso per-row materialization, i.e. inherent to a full walk.

Pros A: measurable, local, no behavior change (`MemoryRow` fields the stats path already ignores stay
absent only on the stats path); no new crate edge. Cons A: needs a stats-specific port seam — a new
`MemoryPort` method, or a narrow query compressor behind the existing one; every `MemoryPort` fake
must keep compiling, so a defaulted trait method is the low-risk shape. Cons B: leaves ~40 % of the
endpoint's wall on the floor; PRD permits "measurement, not deliverable".

**Not implemented here.** Step 2's gate is explicit: a selected fix STOPS for scoped design and a
discriminating RED before implementation, and no fix is authorized by this plan. Handing the above to
the integrator as the measurement result + candidate-A scope request.
