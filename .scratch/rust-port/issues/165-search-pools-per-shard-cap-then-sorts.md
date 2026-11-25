# 165 — `/api/search` can starve the true top-k: it pools a per-shard cap and then sorts

Status: measured — residual starvation confirmed, but the binder is the candidate cap `k`, not the per-shard `truncate`; fix shape rewritten below for replanning
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

## Re-measured 2026-11-25 after issue 169 — the binder is `k`, not `truncate`

Report: `.tmp/progress/issue-165-residual-measurement.md`. Read-only, two §2a unit copies (`43` dbs +`43` WALs each,
`0` failures, `43/43` integrity ok), scratch engine port 3773 (pid 14702 stopped, port re-confirmed free), live
engine 8953 and host 8878 never signalled. Measurement taken on a HEAD (`dfe3dc5`) rebuild — see the staging note
at the end.

169 made the score a function of (memory, query), which is what made this question answerable: before it, the score
moved with `page_size`, so candidate-set membership and the per-shard `truncate` could not be told apart. They can now.

**Result: 10 true-top-20 rows across two frozen queries (7 on q1, 3 on q2) score above the `pageSize=20` cut and are
absent from it.** Worst pair, q1: rank 2 at `pageSize=200`, `auto-capture-fb923c74…994d5f`, similarity
`0.526946152930124`, against the `pageSize=20` 20th result's `0.467455756059875`. q2: `0.5453899894917825` against
`0.5241423225104129`.

**And the binder is not the mechanism this issue names.** `truncate(limit)` starved nothing: every shard contributed
exactly `min(limit, rows)` rows at both page sizes (Σ = 387 / 1595, matching the observed totals). The starved rows
were never candidates at all. All ten live in `project_fd2bfb63318dfd29_shard_0` (2994 rows) and each scores above
that shard's own contributed 20th (`0.4600335307087969` q1 / `0.5241423225104129` q2), so any one of them would have
displaced what was served — but none entered the candidate set, because the keyword channel supplies ids under
`LIMIT k` and these sit at scan positions 54–2437.

**Verified in the source, twice** (`crates/mem-store/src/vector.rs`): `:476` `let k = (limit.saturating_mul(2)).max(32)`
→ `k = 40` at `pageSize=20`; `:483` feeds candidates from `keyword_scores(db, query_text, container_tag, k)`; and
`sql.rs:333-337` / `:340-345` are `SELECT id FROM memories WHERE content LIKE ? … LIMIT ?` with **no `ORDER BY`**.
So the keyword channel contributes at most `k` ids per token, chosen in an unspecified scan order.

A second property follows and is worth recording separately from the starvation: because that order is unspecified,
the 40 ids chosen are stable only while the storage layout is unchanged. Same query, same score, possibly a different
served top-20 after a write or compaction. Not proven to flap here — that would need a mutating write, which this
run was correctly not allowed to make — so it is stated as a consequence of the SQL, not as a measurement.

## Fix shape, rewritten

The earlier fix shape in this issue (merge each shard's top-`page_size`) is already exactly what `truncate` does and
is therefore not the fix. Two candidates, and the choice is a measurement:

1. **Widen keyword candidate membership to the honest match set.** The bump is now a `CASE WHEN LIKE` over already-
   decided ids, so the honest set is "every row matching a token in this shard", clamped by the existing 1000-id
   clamp. Cost was measured *as a rejected alternative for 169*: membership grows to the clamp, ~`+3.2 ms` of hydrate
   against a 25.7 ms scan and 20.3 ms hydrate at `pageSize=20`, on the bench fixture (955 ids). That is the number
   this issue now has to either accept or beat.
2. **Order the candidate query deterministically and cap it there** (`ORDER BY id` or by a stable key). This makes
   membership reproducible but does not stop starvation — it converts a moving cut into a fixed one. It would be a
   partial fix that a criterion could mistake for the whole one.

The criterion that separates them: same two queries, same two page sizes, and the count of true-top-20 rows scoring
above the `pageSize=20` cut must go to **zero**, not merely stop moving. A fix that only stabilises the cut fails.

## Staging note — the fix is not in any deployed engine

MEASURED at `dfe3dc5`: `plugin/engine/linux-x64/opencode-agents-memory` is dated `2026-11-24 20:30:01 +0700` and
contains **zero** occurrences of `keyword_bump_sql`; `target/release/opencode-agents-memory` is `2026-11-24 20:37:38`
and equally stale. The 169 commit is `2026-11-25 18:46:12 +0700`. So issue 169 is fixed in source and absent from
every built engine, and the numbers above came from a fresh HEAD rebuild into scratch.

That is a packaging gap, not a code defect, and restaging is an owner action — the standing directive for this queue
excludes running `scripts/stage-engine.sh`. Recorded here so a later reader of "169 shipped" does not assume the
stored engine carries it.
