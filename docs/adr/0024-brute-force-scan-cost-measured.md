# 0024 — Brute-force scan cost, measured at 10× and 100×: the budget fails, neither lever pays

- **Status:** Proposed (the owner's acceptance is the decision).
- **Date:** 2026-10-30
- **Builds on:** `docs/adr/0018-shard-facts-source-of-truth.md` (the shard owns its committed count),
  `docs/adr/0021-schema-version-discipline.md` (the version discipline the measurement ran under).
- **Blueprint:** `bp-20261030-0525-issue43-w8-benchmark-r6` (sha256 `8b5617b46d93`,
  `SYS-APPROVED 2026-10-30T05:30:44Z`).
- **Wave:** issue 43 W8, base `26d5f00492d8c5fec8e6061a28a1966ddbbf2506`.
- **Evidence:** `docs/evidence/issue-43-w8-benchmark-20261030/` (`README.md`, `raw/baseline.tsv`,
  `raw/baseline_metrics.tsv`, `machine.txt`, 9 × `raw/<cell>.jsonl` + `.summary.json`,
  `raw/driver.log`).

## Context

Decision 4 (owner, 2026-10-25) fixed a **100 ms p95** search budget and set
`max_vectors_per_shard` to ≈12,500 from the then-current 50,000 because `2 × cap × 4 µs ≤ 100 ms` — one
config line, no schema change. The code default **remains `50_000`**
(`crates/mem-core/src/config/schema.rs:583`, `crates/mem-core/tests/config_defaults.rs:105`): the wave's
decision is D4, not CAP-CHANGE, so Step 10 skipped the edit and no source file changed. The arithmetic
was never measured at the sizes the corpus is heading for: P11 measured 3,000 rows, and the issue
itself records the live corpus as 2 341 rows over 19 shards (now stale; the run-time copy read
**3,030 rows over 22 shards**).

W8 measured the memory half of a tag-filtered scope-wide search through production
`vector::search_in_shard` (inline) and a harness-local narrow twin, over nine cells: `x1` anchor,
`{x10,x100} × {inline,narrow} × {cap 50,000, cap 12,500}`, 20 cold + 20 warm samples per cell,
nearest-rank p95, cold samples in fresh processes with `posix_fadvise(DONTNEED)` over every file and
`/proc/self/io` read-byte verification. The timed section includes shard resolution, open, model-facts
verification, scans, hydrate and merge.

## Result

**The first-match decision rule lands on D4 — `NEW-ADR`. No code ships, and the cap does not change.**

| check | measured | required | verdict |
|---|---|---|---|
| D0 `p95_cold(x100-inline-cap50k)` | 8,996,590 µs | < 50,000 µs | ✗ |
| D1 `p95_cold`/`p95_warm` at `x10-inline-cap50k` | 723,554 / 608,121 µs | both ≤ 100,000 µs | ✗ |
| D1 `p95_cold`/`p95_warm` at `x100-inline-cap50k` | 8,996,590 / 8,424,591 µs | both ≤ 100,000 µs | ✗ |
| D2 `p95_cold(x100-inline-cap12k5)` vs `cap50k` | 12,655,531 vs 8,996,590 µs (**1.407×**) | ≤ 0.80× | ✗ |
| D3 `p95_cold(x10-narrow-cap50k)` vs inline | 621,558 vs 723,554 µs | ≤ (1/1.5)× = 482,369 µs | ✗ |
| D3 narrow hydrate share at `x100-narrow-cap50k` | 31.9 % (1,613,769 / 5,065,483 µs) | ≤ 10 % | ✗ |
| D3 `p95_warm(x10-narrow-cap50k)` vs inline | 497,887 vs 608,121 µs (0.819×) | ≤ 1.05× | ✓ |
| **D4 ELSE** | — | — | **NEW-ADR** |

## Decisions

**D-24-1 — Decision 4's arithmetic is re-derived, and it is optimistic.**
The warm exact-only diagnostic (query serialization + shard resolution/open + both scans; open
is ≤0.01 % of the measured time) measures both exact scans at **11.18 µs/row** at 303,000 rows with
`cap 50,000` (5.59 µs/row/column) and **12.22 µs/row** with `cap 12,500` (6.11 µs/row/column). Decision
4's own formula with the measured rate gives `2 × 12,500 × 6.11 µs = 152.8 ms` for a full-cap scope —
1.53× the budget — and an implied cap of `floor(100,000 / 12.223679) = 8,180` rows, not 12,500. The cap
therefore cannot satisfy the budget it was chosen for; the measured 100× pair confirms the direction
(colder with the smaller cap).

**D-24-2 — The narrow companion-table layout does not pay at 100×.**
Re-measured, not inherited: at 10×/`cap 50,000` the narrow arm is 14.1 % faster cold (621,558 vs
723,554 µs); at 100×/`cap 50,000` it is **64.8 % slower cold** (14,826,436 vs 8,996,590 µs) with a 1.754×
cold read ratio, and its hydrate is 31.9 % of scan+hydrate. On disk it is 13.8 % (10×) / 14.1 % (100×)
larger than inline (5,762,125,824 vs 5,051,678,720 B at 100×). The production re-layout is rejected on
this evidence.

**D-24-3 — The budget's input is the scope's total rows (F-W8-1).**
A scope-wide query fans out over every shard of the scope; `max_vectors_per_shard` bounds one file only.
At the measured 12.22 µs/row the 100 ms budget supports `scope_rows_budget ≈ 8,180` rows per queried
scope. Today's largest real scope is 885 rows, whose two exact scans alone are ≈10.8 ms at the 100× rate
(7.9 ms at the x1 rate; the measured x1 anchor, 3,030 rows, is 59,938 µs warm / 76,941 µs cold p95); one
project accumulating ≥ 8,180 memories crosses the budget independent of the cap. The per-shard reading
**does not hold** at the measured sizes either: the 10×/cap-50,000 cell is a single 30,300-row shard at
723,554 µs cold p95, and a 12,500-row shard's two exact scans alone are 152.8 ms at the 100× rate.

**D-24-4 — Named next lever.**
The next lever is an **ANN index** (a different data structure, not a layout) or a **scope-size
rollover** that bounds the rows a single scope-wide query can reach. Both are new scope; nothing ships
in this wave. W8 deliberately does not pick between them — that is open question 4 in the report.

## Consequences

- `max_vectors_per_shard` stays `50_000`; `crates/mem-core/src/config/schema.rs` and
  `crates/mem-core/tests/config_defaults.rs` are untouched. Step 10 is skipped because the decision is
  D4, not CAP-CHANGE.
- `mem-store`'s search path, schema, `sql.rs`, `golden_sql.rs` and `StoreError` are untouched; the wave
  adds one test-only harness, the driver, this ADR and the evidence.
- The cap remains useful as a **file-size/inventory** control (one shard per ~50,000 rows); it is not a
  latency lever, and this ADR records that so a future wave does not re-derive it from the same
  arithmetic.
- If an ANN index is chosen, it must still hydrate from `memories` and honour the model-facts authority
  (ADR-0018); if a scope-size rollover is chosen, F-W8-1's 8,180-row budget is the number to size it
  against.

## Rejected alternatives

- **A `benches/`/criterion target**: would need a `[[bench]]` manifest entry and a new dev-dependency —
  both forbidden by the wave's "no manifest edit" rule.
- **Production `search_in_shard` for the narrow arm**: impossible without production SQL changes (it
  hard-codes `memories.vector`/`tags_vector`), which is why the harness-local twin exists; parity is
  pinned by `test_narrow_scan_matches_inline_top_k`.
- **`sha2` for determinism digests**: not a direct dependency; FNV-1a 64 distinguishes a seed change.
- **Cloning the live root by duplicating rows**: real user text, no scale-shape control, and the scope
  distribution would be a copy — rejected; the measured length multiset is used instead.
- **A synthetic-only corpus**: that is P11, the measurement the issue says is missing at scale.

## References

- `docs/evidence/issue-43-w8-benchmark-20261030/README.md` — method, baselines, the 9-cell table,
  lever analysis, open questions, inverted world.
- `docs/adr/0018-shard-facts-source-of-truth.md`, `docs/adr/0021-schema-version-discipline.md`.
- `crates/mem-store/tests/search_cost_bench.rs`, `scripts/w8-search-cost-bench.sh`.
