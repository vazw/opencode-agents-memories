# 169 — `/api/search` similarity is not a function of (memory, query): it changes with `pageSize`

Status: done — similarity is now a function of (memory, query); six files; AC1–AC4 met with cited proofs
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
   RED today: `0.4717 / 0.4717 / 0.5717 / 0.5717`. **MET** — victim `mem_1793150114376_0nne5nfju` is `0.571748`
   at all four page sizes, tag-scoped and global; re-measured independently at review-2, all eight responses
   byte-identical to round 1.
2. Paging neither duplicates nor omits a row. **MET, REFORMULATED** — set equality between a paged run and one
   `pageSize=200` call is unreachable: `total = Σ_shard min(limit_s, rows_s) + prompts` with `limit_s = page_size`,
   while a score cliff needs `rows > k ≥ 32`, so the two pools differ in size by construction, not by the defect.
   Replaced by the order/partition invariant (victim and rival both asserted present in the `page_size=10` served
   set, order flipping today and equal under the fix). Gate round 1 ruled the reformulation sound; the asserted
   precondition is what keeps it from being vacuous.
3. Tag-scoped latency recorded before and after, not pinned. **MET** — narrow bump statement through the turso `Db`
   seam: 1.8 / 4.4 / 7.9 / 19.5 ms at 100 / 240 / 400 / 1000 ids against a 25.7 ms scan and 20.3 ms hydrate,
   `+4.4 ms = +9.6 %` of the `ps=20` shard path; gate round 2 re-measured 1.8 / 4.2 / 7.1 / 18.1 ms. Keeping
   `AND container_tag = ?` would have cost `+51.5 %`, which is why its absence is a binding, executable condition
   (re-adding it makes row 7 emit 9 placeholders instead of 8 and row 8 lose `(id=?)` at 100/1000 ids).
4. A test seeds a token match deeper than the old `k` and asserts the bump is still collected. **MET** — RED on
   today's `LIMIT k`, GREEN on the merged-candidate bump; the bench twin that reimplemented the old score had to be
   updated in the same commit, and the twin-reverted arm reds (`narrow arm missing w8-0000590`), so the parity
   assertion now discriminates.

## Inverted world

Closed as *measurement, not a deliverable* if a run shows the score is already stable across `page_size` (i.e. the
`LIMIT` cannot bind at this store's scale and the variation above came from a different channel, such as the content
cosine's `k` or the tag branch's `limit`), or if the aggregate cost is measured to be prohibitive at the current store
size and the cheaper fix is to make the ranking channel ignore keyword bumps at scoring time and apply them only as a
tie-break.

## Not in scope

- Issue 165's residual top-k merge question, which is re-measured after this lands.
- Ranking quality, the embedding model, and issue 167's e5 re-trial.

## Landed 2026-11-25 — blueprint `bd93b69c230c`, plan gated twice, review-1 REJECTED, rework, review-2 APPROVED

Six files, `+576/−4` plus a 422-line new test:

| file | Δ | what |
|---|---|---|
| `crates/mem-store/src/sql.rs` | `+23/−0` | `keyword_bump_sql(id_count, token_count, filtered)` — one statement, `CASE WHEN content LIKE ? OR IFNULL(tags,'') LIKE ?` summed per id, no `LIMIT`, no `ORDER BY` |
| `crates/mem-store/src/vector.rs` | `+54/−2` | `keyword_bumps(&ids)` scores the already-merged candidate set; `k` still governs membership |
| `crates/mem-store/tests/turso_vector_search.rs` | `+321/−0` | RED rows: shard-level determinism, paging invariant, deep-match bump |
| `crates/mem-store/tests/golden_sql.rs` | `+174/−0` | builder shape, placeholder count, denied clauses |
| `crates/mem-store/tests/search_cost_bench.rs` | `+4/−2` | the twin scored with the old `LIMIT k`, so parity could only pass while both sides shared the defect |
| `crates/mem-web/tests/search_score_determinism.rs` | new, 422 | API-level four-page-size determinism and page/order invariant |

The shape the gate accepted is behaviour-preserving: same `k`, same membership, same denominator, same
`IFNULL(tags,'')` predicate, `old ≤ new` for every row the old form scored, delta only for `LIMIT`-truncated rows.

### Three measurement errors this wave caught, all in plans or reports rather than code

1. **Units off by 1000×**, and the direction of the conclusion with them. My measurement report's cost table read
   microseconds where the numbers were milliseconds. The blueprint inherited it and concluded "+14 µs against
   5 827 µs, negligible". Re-measured through the turso `Db` seam: the added query is ~13.9 ms at 400 ids against a
   5.8–7.8 ms hydrate, i.e. roughly 2× the hydrate and +26 % of the shard path. That reopened the design decision,
   and the decision then survived only in the narrow form that drops `AND container_tag = ?` (+9.6 %). A cost claim
   with the wrong exponent is a design change disguised as a footnote.
2. **Two placeholder counts, both wrong.** The blueprint expected 8 where the `(2,3,true)` probe emits 9; the plan
   then expected 9 where the shipped builder emits 8. Either would have reddened correct code. Fixed by deriving the
   count from the emitted string rather than from a formula, and by separating the shipped shape from the probe.
3. **The bench fixture count, three times wrong.** The blueprint said 5 matching rows, the gate measured 58, and the
   coder's own bench-run DB showed 1219/1215/1219 of 2000 against `k=80`. Both earlier figures understated it by two
   orders of magnitude and both reached the right conclusion — which is why the count is now re-derived from the
   fixture, not inherited from either report.

### The bench was vacuous in the way this thread keeps finding

`search_cost_bench.rs:996-1027` asserts real-vs-twin parity at `≤1e-12`, and the twin reimplemented the score with
the same `LIMIT k` defect. It could not fail for this issue. The twin now calls `keyword_candidates` and
`keyword_bumps`, and the discriminating arm is recorded: revert only the twin's three lines and the bench reds
(`narrow arm missing w8-0000590`). The id differs between the coder's run and the reviewer's because two panic
closures sit in one loop over a `HashSet`; iteration order picks which fires first. Both arms are real failures, not
the same arm twice.

### Pre-existing reds, measured at base `7e63977` in a separate worktree

`cargo test --workspace --no-fail-fast` rc 101 with four reds, all reproduced at base and none attributable to this
change: `mem-core/config_census::test_census_covers_all_schema_fields` (103 vs 102),
`config_resolution::test_safe_cleanup_defaults…`, `cleanup_tick_latency` (idle-host cap under loadavg 0.63 3.36 7.18),
`production_api_wiring::test_composition_root_passes…`. 2765 tests passed.

### New, filed separately

`cargo audit` flags `RUSTSEC-2026-0592` against `url 2.5.8`. Pre-existing, no dependency change in this diff.
Filed as issue 170 rather than smuggled into this commit.
