# 165 — `/api/search` starves the true top-k: the candidate cap `k` drops true-top-20 rows

> Filename kept for link stability. Retitled 2026-11-25: the earlier title ("pools a per-shard cap and then sorts")
> described a mechanism that measurement has now ruled out three times. See the two "Measured" sections below.

Status: ready-for-agent — fix shape = one blend-ordered statement per shard (measured exact, 122.6 ms vs 209 ms today); the widened-cap shape in the frontier section is SUPERSEDED
`ids.truncate(1000)` clamp; both are required, neither alone reaches zero starvation
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

## Frontier measured 2026-11-25 — the cost of zero starvation, and the second binder under it

Report: `.tmp/progress/issue-165-recall-frontier.md` (177 lines). HEAD `abf257c`, tree clean, release build from
HEAD (the staged engine is stale — see the earlier staging note), port 3791 free at end, live 8953/8878 never
signalled. Recall measured against a **scored full-shard reference**: all 2994 rows of `…shard_0` given the blend,
ordered, top 20 retained. Harness fidelity was checked rather than assumed — its `k=40` shard 20th
(`0.4600335307087969`) reproduces the earlier engine run bit-for-bit, and five repeats of one request give one
distinct SHA-256.

| `k` | recall of full-scan top-20 | shard latency (median of 5, warm) |
|---|---|---|
| 40 (today) | 12/20 | 73.1 ms |
| 80 | 13/20 | — |
| 160 | 18/20 | — |
| **320** | **20/20** | **238.8 ms** |
| 640 | 19/20 | — |
| 1280 | 18/20 | — |
| 2994 (uncapped) | 18/20 | — |

Whole-request, measured not extrapolated: `pageSize=160` (`k=320`) **504.5 ms**, `pageSize=200` **512.4 ms**, against
today's `pageSize=20` `242.7 ms` with 7 true-top-20 rows missing. The size-blind extrapolation (36 × 238.8 ms =
8.59 s) is **unsafe and was discarded** — the median shard holds 10.5 rows — so the request figure is the one to use.

**The curve is non-monotone, and that is the finding, not a flaw in the measurement.** Recall rises 12 → 13 → 18 →
20 → then FALLS to 19 → 18 → 18 as `k` grows past 320. Cause: `vector.rs:504` `ids.truncate(1000)` cuts in insertion
order (content hits, then tags hits, then keyword ids), so a larger `k` pushes keyword-only candidates past position
1000 and drops them. Widening alone therefore **cannot** reach zero starvation, and a fix that only raises `k` will
look better at 320 than at 1290.

**Fix shape, measured rather than argued.** Two coupled changes, and the criterion tests both:

1. **A candidate floor of 320**, not a multiplier: `k = (limit * 2).max(32)` → a form whose floor is 320, so that
   `pageSize=20` (the default path, the one that was losing 7 of 20) gets the same candidate width as
   `pageSize=160`. Cost, measured: shard path 73.1 → 238.8 ms; whole request 242.7 → 504.5 ms.
2. **Remove the insertion-ordered `truncate(1000)`.** It must either go — chunking hydration and the bump statement
   into id batches, if the bind limit is the reason it exists — or be replaced by a bound that cannot drop a
   higher-scoring row for a lower-scoring one. Which of those it is depends on why 1000 exists; the comment says the
   "frozen hydrate builder clamps the same way", so the bind limit has to be established before choosing. A
   deterministic *order* before the truncate is **not** sufficient: ordering by id would make the cut reproducible
   while still cutting true-top-20 rows, which is the partial fix this issue has already rejected once.

**Acceptance criterion, now costed** (replacing my earlier uncosted "must reach zero"):

1. `pageSize=20` serves 20/20 of the scored full-shard reference top-20 on the two frozen queries, at HEAD, measured
   — RED today at 12/20 (q1) with 7 rows scoring above the served 20th.
2. Recall is non-decreasing in `k` across 320 / 640 / 1280 / uncapped: no value gives 20/20 and no larger value gives
   19/20. RED today (20 → 19 → 18 → 18), and this row is the one that keeps the clamp from being "fixed" by masking.
3. The whole-request latency at `pageSize=20` is recorded before and after, not pinned, and the *change* is stated:
   measured today 242.7 ms → 504.5 ms at `k=320`. A fix that also removes the cost is welcome; a fix that hides it
   is not.
4. The two frozen queries AND one query chosen by the implementer return the same recall result — a single-query
   fixture cannot distinguish a real fix from a query that happened to work.

## Still open after this

- Issue 169's fix is in source and in no built engine (see the staging note). Nothing above reaches a deployed
  `plugin/engine/` until it is restaged, which is an owner action.
- Whether `k=320`'s 2.1× request cost is acceptable is a product call, recorded here with its numbers so it can be
  reversed on evidence rather than argued from intuition.

## Superseded 2026-11-25 — the blend-ordered arm wins, and it is cheaper than today

Report: `.tmp/progress/issue-165-blend-ordered-measurement.md`. The owner declined to accept the widened cap
unexamined and asked for the blend-ordered alternative to be measured first. It was, and it dominates:

| arm | whole request (27 tagged shards, 5518 rows) | true top-20 | exact? |
|---|---|---|---|
| **A** — today, per-channel `k=40` union | 209.0 ms | **13/20** | no |
| **B** — `k=320` + clamp neutralised | 452.1 ms | 20/20 | yes |
| **C20** — one blend-ordered statement per shard | **122.6 ms** | **20/20** | **yes, `max |Δ| = 0.0`** |
| **C200** — same, N=200 | 123.5 ms | 20/20 | yes |

So the fix does not have to buy correctness with latency. C is **1.70× faster than today** and **3.69× faster than the
widening it replaces**, while being the only exact arm. Release build, 1 warmup + 5 timed runs.

**Exactness is measured over every row, not just the served page.** The SQL blend was compared with the Rust blend
over all 2997 rows of `…shard_0`: `max |Δ| = 0.0`, zero mismatches. Top-20 and top-200 are identical id-by-id *and*
similarity-by-similarity, and the served page is 20/20 in order with worst aligned delta 0.0 — in a store where the
`word.contains(tag)` branch actually fires (`c` ⊂ `across`/`crates`), so the comparison is not passing by that branch
being dead.

**The implementation detail that decides it: the embedding must bind as an F32-LE blob.** The same statement written
with `vector32(?)` costs 302–307 ms, because the text→F32 parse is redone per row inside `CASE`/`MAX`. Blob binding
brings the large shard to 67.8 ms. A coder who uses `vector32(?)` will produce a correct, slow fix — so this belongs
in the plan as a forbidden form, not a footnote.

Cost per row: 22.6 µs, split LIKE 15.7 µs (70 %), content cosine 4.9, tag overlap 1.6, tags cosine 0.4, sort 0.3.

## Fix shape, replacing the earlier one

Delete the per-channel candidate union and the `ids.truncate(1000)` clamp for the search path: per shard, one
expression computes the blend over the shard's rows and returns the top `offset + limit` ids ordered by it. Both the
`k` floor and the clamp become unnecessary because the candidate set *is* the served set — which is why this is a
deletion rather than a widening.

**`total` becomes a real number.** Today `total` is the pooled candidate count (`387`), which is an artefact of the
cap and moves with the caller's page size. An exact `COUNT(*)` over the same filter costs **1.24 ms for all shards,
1.0 % of C**, and reports `5518`. That is a user-visible change and must be stated in the API contract and covered by
a test, not slipped in: a client that paginates by `total` currently cannot reach beyond row 387 at all.

**Known ceiling, recorded so it is not discovered as a surprise:** C is O(rows) with a LIKE-dominated constant. It is
projected to meet the widened cap at roughly 5× the largest current shard and to lose near the 50 000-row store cap.
The one-line upgrade at that point is a cosine-ordered inner `LIMIT` before the LIKE terms.

## Acceptance criteria, rewritten for C

1. For **both** frozen queries plus one query chosen by the implementer, the served `pageSize=20` page equals the
   full-scan reference top-20, id-by-id **and** similarity-by-similarity, measured through HTTP (the arm above was
   measured on a harness; the harness↔engine offset is 10–14 % on A/B and is stated, not hidden).
2. Served similarity matches the reference to within a stated tolerance (the harness measured `0.0`), so a fix that
   reorders correctly but changes the blend fails.
3. Whole-request latency at `pageSize=20` is measured at HEAD before and after, not pinned, and does not regress
   against today's 209 ms. C's own figure must come from the engine's HTTP path, not the harness.
4. `total` equals the count of rows in scope, is stable across `pageSize`, and a client paging with it reaches every
   row exactly once — the property today cannot satisfy at `total=387`.
5. No `vector32(?)` binding on the hot path: a test or a reviewed line pins the F32-LE blob form, because the text
   form is silently 2.5× slower and functionally identical.

## Remaining open, unchanged

- Issue 169's fix is in no built engine (see the staging note); restaging is an owner action.
- Issue 170 (dependency patches) ships first, as a smaller wave.
