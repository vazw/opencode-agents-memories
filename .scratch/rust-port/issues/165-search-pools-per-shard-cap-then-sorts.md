# 165 — `/api/search` can starve the true top-k: it pools a per-shard cap and then sorts

Status: measured — starvation confirmed but by a different mechanism than filed; two premises corrected; root cause filed as issue 169 and sequenced first
Filed: 2026-11-25, from the issue 28 copy-first rehearsal (run `20261125T035152Z`), cell S5

## The observation

A query frozen before the run (`97 sites across 8 crates`) did not return the pre-registered memory
(`auto-capture-a7df…88b17c`, which is byte-exact in the same store) **anywhere in the 20 returned results**
(`hits_with_target_id=0`). Rank 1 was an unrelated item at similarity `0.4974`.

Do not read this as a ranking-quality complaint yet — the query was five words over 5602 memories and the expected
result may simply rank below 20. What makes it a defect candidate is the mechanism the run read on the way through.

## The mechanism, `crates/mem-web/src/api/memories.rs:904-1013`

`GET /api/search` gathers hits from **every container tag at `page_size * 2` per shard**, pools them, and then
sorts. A per-shard cap applied *before* a global sort can drop a true global top-`k` member whenever one shard
holds more than `page_size * 2` better matches than the rest: those matches never enter the pool, so the global sort
cannot see them. This is the classic top-k merge error, and it gets worse as shard count and memory count grow —
5602 memories over 36 shards is the first scale at which it was noticed.

Not established: whether the missing memory was starved this way, or genuinely ranked below 20. The two are
distinguishable and only one of them is a bug.

## What would settle it

Take the target id's own similarity score from a by-id read (or a one-memory query), then ask whether any shard's
pool of `page_size * 2` could have excluded it while a lower-scoring item from another shard entered. If the
target's score exceeds the pooled minimum and it is still absent, the merge is starving it and the defect is real.
If its score is below the 20th returned, there is no defect and this closes as measurement.

## Fix shape, if the merge is starving

Raise the per-shard cap above `page_size` (it is already `2 ×`), or — better — have each shard return its own
top-`page_size` and merge those by score, which is exact for a top-k over disjoint partitions and costs one
`page_size` per shard instead of `2 ×`. Do not remove the cap: the live writer and the memory ceiling both depend
on it.

## Acceptance criteria

1. A test seeds one shard with more than `page_size * 2` strictly better matches than every other shard, and a query
   returns the correct global top-`page_size` — RED on today's pooling.
2. A query whose true match ranks below `page_size` returns the same set as the merged-per-shard method.
3. No shard is read more than `page_size` rows for one search, and the measured latency at 5602 memories does not
   regress — both recorded, not pinned.

## Inverted world

Closed as *measurement, not a deliverable* if the target's own similarity is below the 20th returned result: no
starvation occurred, the pre-registered query was simply weakly matched, and the pooling is correct for a top-k
over disjoint partitions at `2 ×` headroom.

## Not in scope

- Issue 164 (the rehearsal runbook's own four defects).
- Ranking quality in general: `0.497` as a top similarity over 5602 memories is an embedding-model property until a
  starvation case is measured.

## Measured 2026-11-25 — starvation confirmed, mechanism and target corrected

Report: `.tmp/progress/issue-165-starvation-measurement.md`. Read-only, against a store copy made by the runbook's
§2a unit-copy method (`43` dbs + `43` WALs, `0` failures, `43/43` integrity ok), scratch engine on port 3761, live
engine 8953 and host 8878 never signalled. Filed against `9eca19a`.

**Two premises in this issue were wrong, both from its own evidence.**

1. **The target id is transposed.** The pre-registered memory is `mem_1792819727024_p5ug3wy03`
   (`docs/evidence/issue-28-dropin-rehearsal/TRANSCRIPT.md:173`); `auto-capture-a7df…88b17c` is the **rank-1
   distractor** at `0.4974`, the very item this issue called "unrelated". Moreover `mem_1792819727024_p5ug3wy03`
   **does not exist in the store any more** — 0 hits across all 36 shard dbs, and no id with that prefix, with a
   known-present control returning 1 in the same scan. So its similarity cannot be measured and a different victim
   had to be used. The store has moved on (`5486` memories now vs `5602` at the run; `pageSize=20` total `387`
   vs `401`).
2. **The pool is `page_size` per shard, not `page_size * 2`.** `memories.rs:945` passes `page_size`; the `2×` at
   `:961` belongs to the prompts call and at `:997` to the tag-filtered branch only. The `2×` this issue read is
   `vector.rs:430`'s candidate over-fetch `k = max(2*page_size, 32)` per column (`max(4*page_size, 32)` on the tag
   branch), which is a *second, inner* cap that decides which rows are ever scored.

**The starvation is real, with a named victim and exact numbers.** Same frozen query `97 sites across 8 crates`,
global branch:

| reading | value |
|---|---|
| victim `mem_1793150114376_0nne5nfju` at `pageSize=200` | rank **1**, similarity `0.5717484697739692` |
| 20th result at `pageSize=20` | similarity `0.4479049149236542` |
| victim at `pageSize=20` | **absent** from the top 20 |

A memory scoring `0.5717` never entered a pool whose minimum was `0.4479`. The obvious objection — that the victim's
score is itself page-size dependent, so `0.5717` is not its `pageSize=20` score — is true and measured, and does not
rescue the pool: at `k=160`, a quarter of the budget behind `0.5717`, it still scores `0.4717 > 0.4479`.

**This issue's proposed fix would not fix it.** The victim is excluded by candidate-set membership at `k`, before any
`truncate(limit)`, so merging each shard's top-`page_size` by score never sees it. Acceptance criterion 3 is also
already satisfied returned-wise (`truncate(limit)` caps at `page_size`) while the candidate read is `2×` per column,
so the criterion as written cannot fail.

**Sequencing.** The `k`-dependence of the score is a separate defect and the root cause here — filed as issue 169.
Fix that first; then re-measure this one, because with a deterministic score the residual starvation is the
`truncate(limit)` cap this issue originally described, and criterion 1's fixture must be rebuilt against a row that
exists.
