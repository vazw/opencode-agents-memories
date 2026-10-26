# 55 — Dedup · `nearDuplicateScan.truncated: false` claims full coverage over a shard that was never read

Status: needs-triage — 2026-10-26. **The completeness signal added by issue 54b cannot see the store's own
swallowed read failure: an unreadable shard returns an empty row list, the shard takes the "no scan work"
early `continue`, and the report answers `truncated: false` — a definitive claim of full coverage over a shard
that was never read.** Reachability is a source read (chain below); the runtime frequency is `needs_validation`.

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
