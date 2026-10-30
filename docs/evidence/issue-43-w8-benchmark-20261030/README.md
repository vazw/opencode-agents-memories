# W8 — brute-force search cost at 10× and 100× (issue 43 §43.10)

Measurement date: 2026-10-30. Blueprint: `bp-20261030-0525-issue43-w8-benchmark-r6`.
Harness: `crates/mem-store/tests/search_cost_bench.rs`; driver:
`scripts/w8-search-cost-bench.sh`; raw evidence: `raw/` (this directory).

## Question and the recorded owner decision

Does the brute-force, scope-wide memory search meet the owner's **100 ms p95** budget at 10× and 100×
today's corpus, and if not, does the recorded decision-4 lever (`max_vectors_per_shard ≈ 12,500`) or the
narrow companion-table layout close the gap?

Owner decision 4 (2026-10-25) is **recorded, not re-litigated**: a 100 ms p95 search budget,
`max_vectors_per_shard ≈ 12,500` from 50,000 because `2 × cap × 4 µs ≤ 100 ms`, one config line, no
schema change, shard count grows, and the arithmetic must be auditable in ADR-0024. W8 measures whether
that budget holds at 10×/100×; it does not reopen the budget.

## Method

This is the memory half of `GET /api/search` on the tag-filtered branch, measured through
`mem-store`'s own production search code for the inline arm:

1. `manager.get_all_shards(scope, Some(hash))`;
2. per shard: open the shard DB, `shard_facts::model_facts_cached`, `judge_model_facts` (W3 authority);
3. per shard: `vector::search_in_shard(db, qvec, tag, 40, Some(qtext))` (inline) or the harness-local
   **narrow twin** (identical SQL shape except the three statements that move the vectors to a
   companion table: content scan, tags scan, hydrate; the keyword statement and the ranking/blend code
   are production's);
4. merge: deduplicate by id, sort by `(similarity desc, created_at desc, id asc)` (the handler's
   `created_at` tie-break plus a deterministic `id`), take 20.

The narrow arm's three statements that differ, verbatim from
`crates/mem-store/tests/search_cost_bench.rs:1208-1231`:

```sql
-- content scan
SELECT id, vector_distance_cos(vector, vector32(?)) AS dist
FROM memory_vectors WHERE container_tag = ? AND vector IS NOT NULL
ORDER BY dist ASC LIMIT ?
-- tags scan
SELECT id, vector_distance_cos(tags_vector, vector32(?)) AS dist
FROM memory_vectors WHERE container_tag = ? AND tags_vector IS NOT NULL
ORDER BY dist ASC LIMIT ?
-- hydrate (ids bound, count-clamped to 1000; the only join, top-K only)
SELECT m.id, m.content, m.tags, m.created_at, m.metadata, m.container_tag, m.display_name,
       m.user_name, m.user_email, m.project_path, m.project_name, m.git_repo_url, m.is_pinned,
       m.session_id,
       vector_distance_cos(v.vector, vector32(?)) AS content_dist,
       CASE WHEN v.tags_vector IS NOT NULL THEN vector_distance_cos(v.tags_vector, vector32(?))
            ELSE NULL END AS tags_dist
FROM memories m JOIN memory_vectors v ON v.id = m.id
WHERE m.id IN (?, …) AND m.container_tag = ?
```

The keyword statement is production `sql::KEYWORD_SCORES_TAGGED` unchanged; the three statements above
are the only SQL that differs.

The timed section starts immediately before `get_all_shards` and ends after the merge, so `elapsed_us`
includes `open_us`, `facts_us`, `scan_us`, `hydrate_us`, `search_us` and the merge; they are nested
components, not additive to it.

Dataset: one xorshift64* stream, one row seed and one query seed for every cell; cell rows are the first
`R` indices of the same stream, so 10× is a strict prefix of 100× and paired cells see identical rows and
an identical query vector/text. Content lengths come from the **measured** multiset
(`crates/mem-store/tests/fixtures/w8_content_length_histogram.tsv`, 3,030 lengths, 1,268 distinct);
content bytes are synthetic ASCII from a 256-word vocabulary with the three query tokens injected at
≈1 % each; vectors are deterministic f32 in `[-1, 1]`; tags/metadata are synthetic. Row shape follows the
plan-time ratios (`index % 3,016 < 2,954` tags vectors, `< 1,402` metadata objects), labelled plan-time;
only lengths depend on the measured fixture. The cell name is a label only — nothing in the data depends
on it.

Cells: `x1-inline-cap50k` anchor plus `{x10,x100} × {inline,narrow} × {cap 50,000, cap 12,500}`; rows =
`multiplier × B` with `B` from `raw/baseline.tsv`; shard counts are asserted as
`shard_count(rows, cap)` on the registry before measuring.

Cold: one **fresh process per sample**, `sync`, then `posix_fadvise(POSIX_FADV_DONTNEED)` on every file
of the root (`metadata.db`, every shard `.db`, each existing `-wal`/`-shm`/`-tshm`), then one timed
query; cold honesty is `read_bytes` (delta from `/proc/self/io` around the timed section) ≥ 50 % of the
cell payload, recorded per sample as `cold_verified`. Warm: one process, 3 warm-ups, then 20 timed
samples. n = 20 per series; p95 is nearest-rank. The warm exact-only diagnostic (5 reps) times query
serialization, `get_all_shards`, `open_shard` and the two exact scans (`exact_only_scan` starts its timer
before resolution); the warm samples' `open_us` mean is ≤0.28 ms (≤0.01 % of the diagnostic), so its
µs/row is an upper bound on the scans alone. The no-match diagnostic (5 reps) runs the full query with
absent keyword tokens.

Machine (quoted from `machine.txt`): AMD Ryzen 5 6600U, 12 threads, 14 GiB RAM, `nvme0n1` 1.8 T
non-rotational NVMe, Linux `7.3.0_1` x86_64, rustc/cargo 1.98.0, turso 0.8.3. Disk: 781 GiB free at
Step 0. The driver waits until `/proc/loadavg < 2` before each cell start (203 × 5 s waits across the
wave, 42 of them before `x10-inline-cap12k5`; `raw/driver.log`), and the release build itself waits on
the repo cargo lock; the 9-cell wave started 2026-10-30T07:26:32Z and ended 2026-10-30T08:17:25Z.

Excluded: the embedding-model call and the prompts search (not DB cost, not available in `mem-store`);
the HTTP handler (the merge comparator mirrors its tie-break, but the handler is not exercised).

## Baseline

`raw/baseline.tsv` (6 fields: `<BASE>\t<S>\t<B>\t<dims>\t<bytes/row>\t<DST>`) and
`raw/baseline_metrics.tsv` (`key<TAB>value`) are the source of every baseline number below. Step 0 copied
`$HOME/.opencode-mem/data` into a fresh timestamped scratch directory, checkpointed every copied file,
and read only that copy. The live root was still being written at run time.

Two baselines are kept **labelled**:

- **plan-time (2026-10-30T03:20–03:44Z; source: blueprint `bp-20261030-0525-issue43-w8-benchmark-r6`
  [PROJECT OVERVIEW] plan-time column; never used for any cell)**: 21 shards / 3,016 rows; dims 768;
  tags 2,954/3,016; metadata 1,402/3,016 global and 406/884 in the largest shard (145 B average); content
  p50 607 / p95 2,128 / max 6,603 / sum 2,522,626; payload 7,029 B/row; disk 8,650 B/row; largest shard
  884 rows, 7,680,000 B.
- **Measured at run time (`raw/baseline.tsv`, `raw/baseline_metrics.tsv`)**: **3,030 rows over 22
  shards**; dims 768; tags 2,968/3,030 = 97.95 %; metadata 1,409/3,030 = 46.5 % global and 407/885 =
  46.0 % in the largest shard (145 B average); content p50 607 / p95 2,125 / max 6,603 / sum 2,530,896;
  payload 21,294,538 B = 7,027 B/row; disk 26,243,072 B over 22 files = 8,661 B/row; largest shard 885
  rows, 7,688,192 B; `metadata.db` `user_version = 1`.

**Stale figure, recorded and never used**: the issue's 2 341 rows over 19 shards (43 §43.10,
re-verification line 42) is not the current corpus; the measured copy reads 3,030 rows over 22 shards.
Row/shape drift is recorded and used, never overwritten.

## Results

Every per-cell number below appears in `raw/<cell>.summary.json`; timings are µs, nearest-rank p95.
`shard MiB` is the summed `*.db` file size; `read ratio` is mean cold `read_bytes` / cell payload;
`scan`/`hydrate` are the narrow arm's mean split from the warm samples (inline records only the bundled
`search_us`, so its split columns are 0).

| cell | rows | shards | cold p50 µs | cold p95 µs | cold max µs | warm p50 µs | warm p95 µs | warm max µs | shard MiB | read ratio | scan µs | hydrate µs |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `x1-inline-cap50k` | 3,030 | 1 | 74371 | 76941 | 79036 | 58992 | 59938 | 60778 | 24.2 | 1.201 | 0 | 0 |
| `x10-inline-cap50k` | 30,300 | 1 | 654677 | 723554 | 1053030 | 605007 | 608121 | 616413 | 241.2 | 1.194 | 0 | 0 |
| `x10-inline-cap12k5` | 30,300 | 3 | 671226 | 1023328 | 1095232 | 593990 | 601684 | 606368 | 241.2 | 1.210 | 0 | 0 |
| `x10-narrow-cap50k` | 30,300 | 1 | 590525 | 621558 | 662706 | 482605 | 497887 | 500994 | 274.4 | 1.359 | 320903 | 162567 |
| `x10-narrow-cap12k5` | 30,300 | 3 | 589887 | 611597 | 623399 | 482118 | 497882 | 500480 | 274.4 | 1.359 | 304927 | 177153 |
| `x100-inline-cap50k` | 303,000 | 7 | 7143073 | 8996590 | 10817974 | 6333952 | 8424591 | 9226200 | 2409.0 | 1.313 | 0 | 0 |
| `x100-inline-cap12k5` | 303,000 | 25 | 9445825 | 12655531 | 13023921 | 7068891 | 7187047 | 7227428 | 2408.6 | 1.251 | 0 | 0 |
| `x100-narrow-cap50k` | 303,000 | 7 | 9015325 | 14826436 | 15297778 | 4987571 | 5519195 | 5564728 | 2747.9 | 1.754 | 3451714 | 1613769 |
| `x100-narrow-cap12k5` | 303,000 | 25 | 7776996 | 12299643 | 13064340 | 5035051 | 11130862 | 13783184 | 2747.2 | 1.485 | 3924884 | 1930461 |

All nine cells passed the registry gate (`count(*) = shard_count(rows, cap)`,
`sum(vector_count) = rows`, `min(vector_count) > 0`; `raw/driver.log`): shards 1 / 1 / 3 / 1 / 3 / 7 /
25 / 7 / 25. Every one of the 180 cold samples is `cold_verified = true`. The synthetic payload is
exactly 21,183,282 B at 3,030 rows and scales 10×/100× (211,832,820 B / 2,118,328,200 B).

Layout disk sums re-measured (the issue's "narrow is larger" claim, not inherited): 10× inline
505,810,944 B vs narrow 575,483,904 B (**+13.8 %**); 100× inline 5,051,678,720 B vs narrow 5,762,125,824 B
(**+14.1 %**).

## Lever analysis

- **Per-row exact scan rate** (warm exact-only diagnostic — includes query serialization and shard
  resolution/open, ≤0.01 %; both columns per row):
  8.96 µs/row at 3,030 rows; 10.49 µs/row at 30,300 (`cap 50,000`); 11.18 µs/row at 303,000
  (`cap 50,000`); 12.22 µs/row at 303,000 (`cap 12,500`). That is ≈5.6 µs per row **per column** at
  100×/cap 50,000 and ≈6.1 µs at cap 12,500.
- **Decision-4 arithmetic re-derived**: at the measured 6.11 µs/row/scan, a full 12,500-row scope costs
  `2 × 12,500 × 6.11 µs = 152.8 ms` before keyword/hydrate/open/merge — 1.53× the 100 ms budget. The
  implied cap for a 100 ms scope-wide scan is `floor(100,000 / 12.223679) = 8,180` rows, not 12,500.
- **Cap Δ at the inline 100× pair**: `p95_cold(cap 12,500) = 12,655,531 µs` vs
  `p95_cold(cap 50,000) = 8,996,590 µs` — the cap makes cold **worse** (ratio **1.407**, more shard
  opens and files), and it changes neither the total rows scanned nor the budget verdict.
- **Layout Δ at the cap-50,000 pair**: `p95_cold(x10-narrow) = 621,558 µs` vs
  `p95_cold(x10-inline) = 723,554 µs` (**−14.1 %**), but at 100× the narrow cell is
  `14,826,436 µs` vs `8,996,590 µs` (**+64.8 %**, 14 % more bytes, and a mean cold read ratio of 1.754 vs
  1.313 inline — 1.34× the inline ratio). The narrow
  hydrate is 1,613,769 µs of `scan + hydrate = 5,065,483 µs` at `x100-narrow-cap50k`
  (**31.9 %**), not the ≤ 10 % the re-layout needs. The prior P11 claim that "narrow is faster cold" does
  not survive at 100×.

## Decision

Evaluating the rule in order, the **first match is D4** — `NEW-ADR`:

- **D0 KILL** — `p95_cold(x100-inline-cap50k) = 8,996,590 µs` is **not** `< 50,000 µs`. ✗
- **D1 BUDGET HOLDS** — at `x10-inline-cap50k`: `p95_cold = 723,554 µs` and `p95_warm = 608,121 µs`;
  at `x100-inline-cap50k`: `p95_cold = 8,996,590 µs` and `p95_warm = 8,424,591 µs`. All four exceed
  100,000 µs. ✗
- **D2 CAP HELPS** — `12,655,531 µs ≤ 0.80 × 8,996,590 µs = 7,197,272 µs` is false (ratio **1.407**). ✗
- **D3 RELAYOUT PAYS** — `p95_cold(x10-narrow-cap50k) = 621,558 µs ≤ (1/1.5) × 723,554 = 482,369 µs` is
  false; the `x100-narrow-cap50k` hydrate share is **31.9 %** (not ≤ 10 %). The warm arm
  (`497,887 ≤ 1.05 × 608,121 = 638,527`) passes, but all three conditions are required. ✗
- **D4 ELSE** — **`NEW-ADR`**: ADR-0024 records the measured baseline, the failure of both levers at the
  tested sizes, F-W8-1, and the named next lever. **No code ships; no cap change; no schema change.**

## F-W8-1 — a scope-wide query scans the scope, not the cap

The budget's input is the **scope's total rows**: the query fans out over every shard of the scope, and
the shard cap bounds one file only. At the measured 12.22 µs/row (both exact scans) the 100 ms budget
supports `scope_rows_budget ≈ 8,180` rows per queried scope. Today's largest real scope is 885 rows
(`raw/baseline_metrics.tsv`, `largest_shard_rows`); the 10× cell is one scope of 30,300 rows, 3.7× over
that budget. A corpus that grows by adding scopes keeps per-request latency flat; one project
accumulating ≥ 8,180 memories crosses the budget regardless of `max_vectors_per_shard`.

## Open questions to the owner (each with its measured input)

1. **Is the budget per scope or per shard?** A scope-wide query scans every shard of the scope; the
   measured 10× scope (30,300 rows) is 723,554 µs cold / 608,121 µs warm at cap 50,000, far over 100 ms.
   The per-shard reading **does not hold** at the measured sizes either: that cell is a **single shard**
   (`raw/x10-inline-cap50k.summary.json`: shards 1), and at cap 12,500 a full 12,500-row shard's two
   exact scans alone cost `12.223679 µs/row × 12,500 = 152.8 ms` at the 100× rate (124.5 ms at the 10×
   rate). The cap-12,500 cells hold 12,500-row shards by construction (10×: 12,500 + 12,500 + 5,300;
   100×: 24 × 12,500 + 3,000; registry `min(vector_count)` 5,300 / 3,000). Only today's scale is under
   the measured budget: the x1 anchor (3,030 rows) is 76,941 µs cold / 59,938 µs warm p95, and the
   885-row largest scope's exact scans are ≈10.8 ms at the 100× rate (its full-request time was not
   measured). **"Per shard" is not a safe reading at 10×/100×.**
2. **Which scope shape is the horizon?** Today: 22 scopes, largest 885 rows, aggregate 3,030
   (`raw/baseline_metrics.tsv`). The benchmark grows **one** scope. Is the target the corpus or the
   largest scope?
3. **Cold or warm?** The budget text does not say. This report treats the budget as met only if **both**
   cold and warm p95 are ≤ 100 ms; if steady-state was meant, the cold cells are information.
4. **If the answer is an ANN index**, is that in issue 43's scope or a new issue? The measured exact scan
   is 8.96–12.22 µs/row for both scans (4.48–6.11 µs/row/column) depending on size and shard count; an
   ANN index changes the data structure, not a layout.
5. **Is this laptop the reference machine** for a budget quoted to users (Ryzen 5 6600U, 14 GiB,
   consumer NVMe)? The numbers were measured on this machine; re-run on the release target before
   quoting them.

## Threats to validity

- One machine, one page cache, one clock: the numbers are not portable. The driver gates cell starts on
  `/proc/loadavg < 2` only (203 × 5 s waits; `raw/driver.log`) and has **no** cargo-build detector.
  Every `cargo` invocation in the driver goes through `.tmp/scripts/cargo-serialized.sh` (`flock` on
  `.tmp/build.lock`), so a build from another worktree that bypassed the wrapper would be undetected
  below loadavg 2. The wave spanned 51 minutes of an otherwise active desktop.
- Synthetic content: the length multiset is measured, the bytes are not; token inject absorbs part of
  the keyword scan, and the keyword statement's LIMIT stops at 80 matches in this corpus.
- Cold honesty is a lower bound: `read_bytes ≥ payload/2` was true for every sample (per-sample ratios
  1.1927–2.2942; per-cell means 1.194–1.754; the max is `x100-narrow-cap50k`, sample
  `read_bytes=4,859,834,368` / payload `2,118,328,200 = 2.294`), but `posix_fadvise` is advisory and
  page-cache state is only as honest as the kernel makes it.
- The narrow arm is a harness-local twin, not production code; parity is pinned by
  `test_narrow_scan_matches_inline_top_k` (2,000 rows, pre-sort per-id similarity delta 0, equal id sets,
  equal post-merge order), but a production re-layout would face hydrate/index choices the twin does not.
- n = 20, nearest-rank p95; the warm `x100-inline-cap50k` p95 (8.42 s) is noisy against its p50 (6.33 s),
  so the cap comparison also quotes the cold pair.

Run history: three attempts. (a) 07:04:52Z aborted at `x10-inline-cap12k5` seeding — the final
partial-shard check compared the shard's own `vector_count` with the scope total; fixed to `rows % cap`.
(b) 07:20Z aborted at `x10-narrow-cap50k` warm — the exact-only diagnostic used the production inline
scan against the narrow schema; made layout-aware. (c) 07:26:32–08:17:25Z completed and is the only
wave in `raw/driver.log`. The earlier attempts' artifacts were deleted (`find -delete`) and their driver
logs were not preserved, so `raw/driver.log` records the final wave plus the release-build output, not
every command of the session; the Step 7 smoke run (`raw/smoke.jsonl`, `raw/smoke.summary.json`)
predates the log reset. The two aborted attempts are reported from the wave handoff, not re-derived.

## Inverted world — what would flip this decision

- A measured exact-scan rate below ≈1.6 µs/row at 30,000 rows (and sustained to 100×) would put the
  100 ms budget back within reach → **NO-WORK** (D0/D1), and would refute the per-row rate above.
- A ≥ 20 % cold p95 gain at `cap 12,500` in the inline 100× pair would make the cap the lever →
  **CAP-CHANGE**; the measured direction is the opposite (1.407×).
- A ≥ 1.5× cold gain with a ≤ 10 % hydrate share at 100× would make the narrow re-layout pay →
  **NEW-ADR recommending the re-layout**; measured −14 % at 10×, +64.8 % at 100×, 31.9 % hydrate.
- A scope-shape finding that the owner's horizon is "add scopes, never grow one" would make per-request
  latency flat and the budget a corpus-size question, not a scope-size one.

## Sources

- Baseline: `raw/baseline.tsv`, `raw/baseline_metrics.tsv`, `machine.txt`.
- Per-cell: `raw/<cell>.jsonl` (one line per sample) and `raw/<cell>.summary.json` (aggregate; every
  number in the table above).
- Driver commands and exit codes: `raw/driver.log`; smoke: `raw/smoke.jsonl`,
  `raw/smoke.summary.json`.
- Decision record: `docs/adr/0024-brute-force-scan-cost-measured.md`.
