# 55 — Dedup · `nearDuplicateScan.truncated: false` claims full coverage over a shard that was never read

Status: done — 2026-10-26. Landed as **`34a5fa3`** (`fix(dedup): count unreadable shards instead of
claiming full coverage`) on branch `issue-55/dedup-completeness`, base `c4cd25b`. This record's status
line and `## Comments` entry follow in the record-only commit that names `34a5fa3`.

## PREMISE — the chain, source-read end to end in this tree

| step | evidence |
|---|---|
| a failed `SELECT *` is swallowed as an **empty** row list | `crates/mem-store/src/vector.rs:173-178` — `get_all_memories` ends `.await.unwrap_or_default()` |
| the dedup store calls it, and **already documents** the consequence | `crates/mem-web/src/api/deduplication/store.rs:81-85` — *"an unreadable shard is indistinguishable from an empty one here, so the run's report is partial for it — no deletions, no groups, no error surfaced"* |
| zero rows ⇒ `survivors.len() <= 1` ⇒ early `continue` that does **not** set `truncated` | `crates/mem-web/src/api/deduplication/mod.rs:220-223` |
| `truncated` is a plain `bool`, always present, never skipped | `crates/mem-web/src/api/ports.rs` (`NearDuplicateScanReport`), `crates/mem-web/src/api/dto.rs` (`NearDuplicateScanDto`) |

So on a shard whose rows fail to load, the pass reports `pairsScanned: 0`, `truncated: false`, and no groups —
byte-identical to a genuinely empty shard.

## Why this is a defect and not a nit

`truncated` is the entire reason the field exists. Issue 54b's blueprint states its purpose: *"a consumer that
reads it can tell 'no near duplicates' from 'did not look'."* On an unreadable shard it tells the consumer
**"did look, everywhere, and found nothing"** — the one direction the signal must never lie in. The report's
other fields cannot disambiguate it either: `pairBudget` and `pairsScanned` are both consistent with an empty
shard.

## What a fix must decide — the scope ceiling is the point

`store.rs:81-84` calls `get_all_memories` a *fenced* module, so the cheap fixes are constrained:

- **A — surface the read failure through `DedupStore`.** Correct and expensive: `memories()` already returns
  `Result<Vec<DedupMemory>, PortError>` (the open error is mapped), so a row-read failure could map to
  `PortError::Store` — but `get_all_memories` returns `Vec<Row>`, not a `Result`, so this needs a `mem-store`
  signature change (the fence) or a new fallible variant beside it.
- **B — count unreadable shards in the report.** Additive and cheap at the mem-web layer:
  `unreadableShards: u64`, forcing `truncated: true` when non-zero — but it has nothing to count unless the
  store can be asked whether the read succeeded, which is exactly what A provides.

The honest minimum is therefore *A's fallible read plus B's report field*. Record whichever is chosen with its
reason, and do not silently widen `DedupStore`'s trait: issue 54b forbade that for its own wave, and this issue
is where that ceiling is deliberately re-opened — or explicitly declined.

## Not a regression from 54b — 54b only made it consequential

The swallow predates this wave (`mem-store`), and so does the comment at `store.rs:81-84`. 54b's contribution
is the field that turns a documented "the report is partial" into a machine-readable claim that is false. That
is why this is filed separately rather than as a rework: fixing it requires the store-layer decision 54b's
scope ceiling excluded.

## Gate

Every claim above is `READ` from this tree at `5c06275`. What is **not** established: how often a shard's
`SELECT *` actually fails in production, and whether `PortError::Store` on one shard should abort the pass or
degrade per shard. Both are `needs_validation`; neither blocks writing the failing test.

## Comments

### Outcome (record-only; implementation commit `34a5fa3`)

**Design resolution — A's signature plus B's field; C rejected.** `get_all_memories` becomes
`pub async fn get_all_memories(db: &dyn Db) -> Result<Vec<Row>, StoreError>` (body:
`db.query(GET_ALL_MEMORIES, Vec::new()).await`), not a sibling `try_` variant: ADR-0013 D-4 already
names a swallowed failure a defect, a sibling would leave the banned shape in production and guarantee
the next reader reinvents this bug, upstream `getAllMemories` throws, and the blast radius was three
sites. `NearDuplicateScanReport` gains `unreadable_shards` (wire key `unreadableShards`) **inside**
`nearDuplicateScan`, beside the flag it qualifies. `truncated` becomes true whenever the counter is
non-zero but is **not** folded into it: `truncated` still means *budget exhaustion* as issue 54b pinned
it (`pairsScanned == pairBudget` is a false corollary of every unreadable shard), so the counter is the
only key that separates "we stopped spending" from "we could not see". C (abort with
`Err(PortError::Store)`) was rejected because the pass mutates as it walks (each shard's exact
deletions are awaited and committed before the next shard is read), so a mid-pass abort would answer
an error over deletions already committed in earlier shards; the per-shard read fault follows the
in-file precedent of the per-row delete fault instead.

**Pre-change behaviour, observed not inferred.** Both runs were taken against `c4cd25b` with the new
field absent from the test bodies (both are recorded verbatim in the wave ledger,
`.tmp/progress/20261026-1213-issue-55-dedup-completeness.ledger.md`, which is gitignored):
the dropped-table shard answered `truncated: false` with `pairsScanned: 0` and no groups or deletions
— `truncated: false` over a shard whose rows were never read; the fake store's read fault answered
`Err(PortError::Store(..))` for the **whole** pass — the abort that the new arm replaces. AC1/AC3/AC4/
AC5/AC6's RED at base is a compile error (`E0609`, the field does not exist), which proves wiring and
suppresses every assertion in the file; it is not presented as a behavioural observation.

**Divergence (recorded):** upstream aborts the pass on a shard read failure and answers an error over
deletions already committed; this build counts the shard, forces `truncated`, and continues.

**The `open_shard`-on-garbage probe was not taken.** Whether a shard file written with garbage bytes
(or truncated mid-page) makes `open_shard` itself fail is not covered by any evidence here. The plan
does not depend on it: the DROP-based fixture forces the open-succeeds-then-query-fails path
unconditionally, and the read-fault arm is separately proven with a fake.

**Adjacent residual (recorded, not a work item):** the per-row delete fault in the same loop
(`crates/mem-web/src/api/deduplication/mod.rs:238-247`) is logged and swallowed without withdrawing any
coverage claim — the same shape as this issue, one layer over. It is 43.2 / ADR-0018 territory, not
fixed here and not precedent for widening scope. Likewise `get_distinct_tags` and
`get_project_path_counts` (`crates/mem-store/src/vector.rs:180-207`) still swallow a query failure into
an empty result; they are out of scope and recorded as residual, not fixed.

**Tests.** `crates/mem-web/tests/dedup_unreadable_shard.rs` (new, 3 — counted-and-truncated, the pass
degrades rather than aborting, and the mirror-image guard that a genuinely empty shard still reports
full coverage); `crates/mem-store/tests/vector_store_subset.rs` (one new failing-read case);
`crates/mem-web/tests/dedup_service.rs` (`FakeDedupStore::fail_memories_on` plus the AC4 case, and
issue 54b's truncation assertions kept byte-identical except one added `unreadable_shards == 0`);
`crates/mem-web/tests/dedup_endpoint.rs` (the pinned `nearDuplicateScan` key set gains only
`unreadableShards`, asserted `0`). `cargo test --workspace` green; both clippy gates and
`cargo fmt --all --check` green; `crates/mem-web/tests/api_endpoint_contract.rs` and
`api_route_auth.rs` byte-identical. No dependency, manifest, toolchain or `.cargo/` change; no config
key — the field reports what happened, it does not choose anything.
