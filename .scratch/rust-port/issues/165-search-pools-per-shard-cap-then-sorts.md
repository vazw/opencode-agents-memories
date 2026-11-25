# 165 — `/api/search` can starve the true top-k: it pools a per-shard cap and then sorts

Status: needs-triage
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
