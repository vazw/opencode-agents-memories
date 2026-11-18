# 148 — the issue-145 runbook's delivery reading is dead on the current host build

Status: landed — runbook rewritten to the nonce instrument, 2026-11-19

*(Filed from a live probe, not from reading the runbook. The runbook was not wrong about its own era; the host changed
under it. Fixed the same day in `docs/evidence/issue-145/README.md`.)*

## The defect

`docs/evidence/issue-145/README.md` decides delivery with one predicate, repeated in Q6 (`:57`), Q7 (`:100`) and
Q8 (`:106`), and classified `miss-host` against it (`:157`, `:71`):

```sql
m.type='synthetic' and m.data like '{"text":"<memory_context>%'
```

On the current host build that predicate returns **zero for every session, delivered or not**, and would label every
child `miss-host`.

MEASURED 2026-11-19 against `~/.local/share/opencode/opencode.db` (read-only):

| reading | value |
|---|---|
| rows matching the runbook predicate exactly | **8** |
| distinct sessions | **8** |
| newest such row | `2026-10-11 22:47:50` |
| rows since 2026-10-11 | **0** |

Eight sessions in mid-October wrote the block as a `type='synthetic'` row; none has in the five weeks since, across
189 `inject.received` events. My own session (which receives an inject block every turn) has **0**. So the host
stopped persisting the block this way; the runbook outlived the behaviour it encodes.

## Why the obvious repair is worse

`type='synthetic'` is **not** a memory row type on this build — it is mostly subagent completion notices
(`{"metadata":{"source":"subagent","childID":…,"state":"completed"}}`), 2477 of them in my session alone.

Dropping the `m.type='synthetic'` term to "find the block somewhere" is exactly what runbook trap A forbids, and the
trap understates the problem: `data like '%<memory_context>%'` returns **212** synthetic rows in my session and every
one is a *completion notice carrying the child's answer*, because the child quoted the tag back. A loosened query
measures the conversation, not the injection. Loosening it trades a false negative for uncontrolled false positives.

## What actually settles delivery: a nonce

A token that exists only inside the block, and an instruction to quote it:

1. Write a unique string to memory (`memory add`, project scope), e.g. `PROBE-NONCE-<8 hex>`.
2. Spawn a fresh child and ask it to quote any `PROBE-NONCE-<8 hex>` it finds, or answer `NONE`.
3. Echo ⇒ delivered. `NONE` ⇒ not delivered. Then remove the memory entry.

MEASURED, this method: `PROBE-NONCE-7f3a91c2` came back with provenance the prompt never contained — *"found in the
injected `<memory_context>` block, in the `project_agent_session_history` section under 'Project facts' (memory
relevance 72%)"* — while the log for that same child read `inject.received block_bytes:6950`. Nonce since removed
(`deleted: 1`).

## A second instrument defect worth recording

Asking a model **whether** it can see a block is unreliable: a child on the same build answered `<memory_context>`
**absent** while `inject.received block_bytes:7083` was already in the log for its own session. Asked instead to
produce a token, the next child found it immediately. Runbooks here must ask for output, never for introspection.

## Fix

Replace Q6/Q7/Q8's database reading with the nonce method, and rewrite the `miss-host` definition (`:157`) so it
cannot be decided by a query that returns zero on this host. Keep the `type='synthetic'` note as history with the
2026-10-11 cutoff, not as an operative check. Do not loosen the predicate.

## Not a product defect

Nothing in `plugin/` or the engine is wrong here. The wrapper emits `inject.received` with `host_contract` and
`block_bytes` (189/189 lines), zero sink failures, and delivery demonstrably works. Only the evidence procedure is stale.

## What the fix changed

`docs/evidence/issue-145/README.md`, same day:

- **Q1** replaced: log-side calibration (`host_contract` present, an `inject.received` with `"level":"info"`) instead of a
  store-wide count that returns zero on this build.
- **Q1b added**: instrument-side calibration. One throwaway child must echo a fresh nonce before any measured child runs;
  a `NONE` from it records `uncalibrated (instrument)` and the run stops. This separates "instrument broken" from
  "delivery broken", which the old single calibration could not.
- **Q6** replaced: the nonce echo, written *after* the compaction boundary so the post-compaction block is its only
  possible source. The pre-fix version read a row that never exists.
- **Q7** replaced: ask the child to name its own last action and compare with the owner-recorded token, instead of reading
  an action token out of a row that is never written.
- **Traps A–D** rewritten. A and B are the dead predicate and why loosening it is worse; C is the measured introspection
  failure; D keeps the pre-existing contamination rule.
- **Reading rule** re-anchored on behaviour: `miss-host` is now "engine returned a block, child could not produce a token
  that exists only inside it", which is decidable on this build.
- **`stale` promoted to the predicted reading**, with `recent-action-delivered` named as its inverted world. The old text
  treated `stale` as an extension-worthy failure, which would have extended every run to N = 10 forever.
- **Two stale readings removed**: the header's pid pins, and `session_v2.time_compacting` as a compaction signal —
  MEASURED `NULL` through three compactions on a child that compacted for real. The `compaction` row count is the
  signal.

Deliberately not done: no regression test pins the runbook's prose. A markdown guard for "the dead predicate must not
return" would be the only thing worth adding, and it is not worth a Rust suite entry today. If this recurs, that is when
to add it.
