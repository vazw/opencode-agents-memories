# 169 — `/api/search` similarity is not a function of (memory, query): it changes with `pageSize`

Status: ready-for-agent
Filed: 2026-11-25, while measuring issue 165 (`.tmp/progress/issue-165-starvation-measurement.md`)

## The observation

The same memory, against the same frozen query, scores differently depending on how many results the caller asked
for. Tag-scoped to the victim's own container tag, one memory `mem_1793150114376_0nne5nfju`:

| `pageSize` | `k` per column | rank in shard | similarity |
|---|---|---|---|
| 10 | 80 | absent | — |
| 20 | 160 | 6 | `0.47174846977396917` |
| 30 | 240 | 2 | `0.5217484697739692` |
| 40 | 320 | 1 | `0.5717484697739692` |
| 60 / 100 / 200 | 480 / 800 / 1600 | 1 | `0.5717484697739692` (saturated) |

`+0.05` per candidate-budget step, then flat. MEASURED, one scratch engine on a store copy, port 3761.

## The mechanism, `crates/mem-store/src/vector.rs:389-412`

`keyword_scores` runs one `LIKE` scan **per token** with `LIMIT k`, unordered, and each in-limit hit adds
`1 / tokens.len()`. A memory sitting deeper in the scan than row `k` misses that token's bump entirely, so its score
depends on where it happens to fall in an unordered scan whose size is `k`.

`k` comes from `vector.rs:430` — `(limit.saturating_mul(2)).max(32)` — and on the tag branch from the `page_size * 2`
limit passed at `memories.rs:997`, so `k` is `max(4*page_size, 32)`. Caller's page size therefore reaches the score.

Two consequences, both user-visible:

1. **Pagination is unsound.** `total = entries.len()`, then `skip(offset).take(page_size)` (`memories.rs:1047-1054`).
   If the score of a row changes between two calls with different `page_size`, rows can cross the boundary: paging
   through results can duplicate a row on one page and omit another entirely.
2. **Rank is not comparable across calls.** The rehearsal's own rank-20 diagnostic read `0.4689909533215277` where
   the saturated score is `0.5717` — a reader comparing the two would conclude the ranking moved.

This is the root cause of issue 165's starvation: a row excluded from the candidate set is excluded from the *score*,
not merely from the ordering, so a per-shard top-k merge cannot repair it.

## Fix shape

Make the keyword channel exact and order-independent: drop `LIMIT k` from the per-token scan (or replace it with a
deterministic bound that cannot depend on `page_size`, e.g. a `SUM`/`COUNT` aggregate in SQL over the token pattern),
so a row's bump total is a function of (row, tokens) only. Then `k` governs how many rows are *returned*, not what
they score. Do not simply raise `k`: that moves the cliff and leaves the score a function of the caller's page size
until the cliff passes the deepest row, which is `LIMIT` dependence with a bigger constant.

Record the cost: the per-token scan becomes unbounded work over the tag's rows. Measure it at the current store
(`5486` memories / 36 shards, tag-scoped) and report the delta rather than pinning a number.

## Acceptance criteria

1. One memory's similarity for one query is identical across `pageSize` 10, 20, 40 and 200, measured on a store copy.
   RED today: `0.4717 / 0.4717 / 0.5717 / 0.5717`.
2. Two consecutive calls with different `page_size` over the same result set return pages that neither duplicate nor
   omit a row — checked by paging the full set at `page_size=10` and comparing the id set against one call at 200.
3. The tag-scoped query latency at the current store is recorded before and after, not pinned, and no shard is read
   more rows than the fix's own stated bound.
4. A test seeds a token match whose row position exceeds the old `k` and asserts the bump is still collected —
   RED on today's `LIMIT k`.

## Inverted world

Closed as *measurement, not a deliverable* if a run shows the score is already stable across `page_size` (i.e. the
`LIMIT` cannot bind at this store's scale and the variation above came from a different channel, such as the content
cosine's `k` or the tag branch's `limit`), or if the aggregate cost is measured to be prohibitive at the current store
size and the cheaper fix is to make the ranking channel ignore keyword bumps at scoring time and apply them only as a
tie-break.

## Not in scope

- Issue 165's residual top-k merge question, which is re-measured after this lands.
- Ranking quality, the embedding model, and issue 167's e5 re-trial.
