# 39 — W8.1 · Make `POST /api/cleanup` real (retention cleanup)

Status: done — 2026-11-10 (reviewed APPROVED: `blueprint-20261110-issue39-retention-hybrid-r2.review-1.md`, ADR-0032)

## PAUSE NOTE — 2026-10-25 (resolved 2026-11-10)

**Two independent reviews of the database design handover converged on the same finding: as specified, this
feature does nothing today.** `linked_memory_id` has **zero writers**, so phase 1 reads an empty set, phase 2
stamps nothing, and every memory expires on `updated_at` alone. Shipping a v3 migration, an ordering
invariant and a new eligibility term for a guaranteed no-op is the "specification that does nothing" failure
this repo has already paid for.

**Both reviewers independently proposed the same replacement:** record the prompt's creation time on the
memory row **at capture time** (capture already has its source prompt in hand), then compute eligibility
entirely inside the shard:

```
updated_at < cutoff AND (source_prompt_created_at IS NULL OR source_prompt_created_at < cutoff - retention)
```

That removes phase 1, the stamp, `max()`, the ordering invariant and the dependency on `linked_memory_id` —
and it stores a fact rather than a computed deadline, so it survives a `retention_days` change. Prompt
deletion then stays exactly as upstream does it.

**The trade-off the owner must settle before any blueprint:** after an offline gap longer than 2× retention,
prompt and memory are deleted in the same run, which literally violates "should not die at the same moment".
The event-based design rescues that case; the capture-time design does not. Is the requirement a *wall-clock
window* or an *observed event*? For a local-first tool that may not run for months, this is a product
decision, not an implementation detail.

**One defect in the ratified design, confirmed by both reviewers:** stamping to `now` means every failed run
pushes the window further out — repeated crashes make a memory immortal. Any revised design must stamp a
deterministic function of the prompt (`created_at + retention`), never wall-clock time.

The three-phase content below is retained as the record of what was decided and why; it is **not** approved
for implementation.

**Profile invalidation is inherited, not optional (47.1, 2026-10-25).** When this issue lands the prompt
deletion path it must cite **47.1 Leg B (B2′)** and decide whether retention pruning joins the profile
invalidation. `delete_old_prompts` (`prompt.rs:384`) has zero callers today, so 47.1 deliberately deferred
that case here rather than guess; until it is decided, material from a retention-pruned prompt may persist
in a `user_profiles` row — a named residual in 47.4, not silence. A deleted prompt that predates the
profile's `last_analyzed_at` must erase the profile **and** its `user_profile_changelogs` snapshots
(`profile_data_snapshot` holds full profile text).

## What to build (SUPERSEDED — see pause note)

`POST /api/cleanup` is an unconditional 501 stub taking no `State`
(`crates/mem-web/src/api/maintenance.rs`, `pub async fn cleanup() -> Response`). Implement retention
cleanup behind `MaintenancePort::cleanup()`, rewrite the handler to call it, widen the report 3 → 5
fields, and fix `maintenance_failure`'s `PortError::Unavailable → 500` conflation to 501.

## STORAGE DESIGN — B, RATIFIED (2026-10-25)

**The divergence:** a prompt-linked memory's retention **restarts when its prompt expires**.

**Mechanism — a nullable column, not a side table:**

`ALTER TABLE memories ADD COLUMN prompt_expired_at INTEGER` — shard migration **v3**, nullable, no
default, **no index** (the sweep already full-scans `updated_at < cutoff`, and an index on the stamp
cannot narrow it).

**Eligibility** (evaluated inside the shard, one predicate):

```
updated_at < cutoff AND (prompt_expired_at IS NULL OR prompt_expired_at < cutoff)
```

strict `<` on both sides, so `prompt_expired_at == cutoff` is still protected.

**`NULL` means "never restarted by a prompt"** — today's state for every row, and the pre-feature
behaviour. Never coerce `NULL` to `0`: numerically equivalent, but this repo has already paid for
"absent and empty became identical". `IS NULL` is a first-class branch.

**Replaces "one atomic batch" with an ordering invariant.** Three idempotent phases:

1. **Read** expired links: `SELECT DISTINCT linked_memory_id FROM user_prompts WHERE created_at < ? AND linked_memory_id IS NOT NULL`. (Split `expire_old_prompts` into this read and a later delete so the adapter can interleave.)
2. **Stamp + sweep, per shard:** ensure v3; `UPDATE memories SET prompt_expired_at = max(COALESCE(prompt_expired_at, 0), ?) WHERE id IN (…chunked…)`; then select the eligible set, skip pinned, delete + `decrement_vector_count`. Both statements in one `BatchMode::Write` per shard (single file, so genuinely atomic).
3. **Delete prompts + the existing write-back** (`DELETE FROM user_prompts WHERE created_at < ?`; clear `linked_memory_id` on prompts whose memory was deleted).

**Why that is crash-safe** — write this sentence into the ADR, verbatim:

> Every interleaving leaves stamps ≥ intended and prompts ≥ intended, and re-running with `max` cannot shorten a window: the failure mode is "a prompt lingers one run", never "a memory dies early".

## Why not A (the previous ruling) — recorded so it is not re-derived

The earlier architecture ruling chose a `prompt_memory_expiry` side table in `user-prompts.db`, on the
grounds that only one database can hold the stamp and the prompt delete in one commit. It was
**superseded** on 2026-10-25 after a second ruling with new evidence:

- A **is** genuinely atomic (`Db::batch(…, BatchMode::Write)` → `transactional_batch(Immediate)`, already
  used by `schema_migrations` and `capture_inbox::mark_done`). The ruling did not rest on a hollow premise.
- But **that atomicity is not load-bearing for the rule** — the rule needs "a linked memory's retention
  restarts", not "the row disappears in the same instant as the stamp". Ordering satisfies it.
- A's costs are **permanent and silent**: a stamp row outliving its subject, a prune rule that must stay
  rule-preserving after every edit, fail-open if `user-prompts.db` is lost, and a predicate split across
  two files because `ATTACH` is unavailable in this engine.
- **`ATTACH` is refused at parse time** by turso 0.8.3 (`--experimental-attach` required), and the engine
  opens **WAL by default**, where SQLite's cross-file super-journal guarantee does not apply. Any design
  premised on a cross-database transaction is unavailable here, not merely awkward.

**Rejected alternatives** (for the ADR's Grounds section): A (above); C — never delete expired prompts and
derive expiry from `created_at + retention` (strictly shallower, but it changes upstream's prompt-deletion
semantics, keeps full prompt content ~2× longer, and forces every prompt query to filter tombstones);
falsifying `updated_at` (destroys the user-visible last-updated value).

## Decisions that survive the re-scope (do not re-open)

1. **Protection is implemented now**, even though `linked_memory_id` is never written at runtime
   (`link_memory_to_prompt` has zero callers). Tests seed the column directly.
2. **A live prompt protects nothing** — only an expired one grants a window (upstream-identical). Cleanup
   also **clears `linked_memory_id`** on prompts whose memory it deleted.
3. **`CleanupReport` widens 3 → 5**: `deleted_count`, `user_count`, `project_count`, `prompts_deleted`,
   `protected_memories_skipped`. `ai_sessions_deleted` / `linked_memories_deleted` stay out — neither can
   be truthfully populated (no AI-session store exists).
4. **`expire_old_prompts` replaces `delete_old_prompts`** (zero callers; leaving it creates a seam that
   deletes links without stamping). Now split: a read (`expired_linked_memory_ids`) and a delete
   (`delete_expired_prompts`), because the phases interleave.
5. **Run guard** (`AtomicBool`) — concurrent runs would double-count and make the report false data.
6. **Manual endpoint only**; no scheduled tick (issue 41).
7. **`auto_cleanup_enabled` keeps upstream's `true`** — nothing reads it in this wave, and the "deletes on
   first boot" risk belongs to issue 41 where the timer is decided. Do not touch
   `DEFAULTS.auto_cleanup_enabled` or `config_defaults.rs`.
8. **The cleanup run is the sole writer of `prompt_expired_at`.** Capture's future
   `link_memory_to_prompt` wiring must never write it — the stamp records *expiry*, not *link*. (Glossary
   enforcement, same as A's rule.)
9. **An imported memory starts with `NULL`** (a full fresh window): the stamp is derived lifecycle state
   and is never exported. The portability bundle's explicit field list already excludes it.

## Included defect fix

`/api/migration/*` answers **500** where it should answer **501**: `maintenance_failure` funnels
`PortError::Unavailable` through `port_failure` → `internal_error()`, while `session_failure` correctly
maps the same condition to `not_implemented`. Make `maintenance_failure` mirror `session_failure`, with a
per-endpoint 501 sentence.

## Upstream behaviour (`example/opencode-mem/src/services/cleanup-service.ts`)

Cutoff from `autoCleanupRetentionDays`; collect pinned ids across **all** user and project shards; expire
old prompts; protect; per shard select `id, container_tag, is_pinned WHERE updated_at < cutoff`, skip
pinned/protected, delete vector + `decrementVectorCount`, count `_user_`/`_project_` by `container_tag`;
`VACUUM` the prompts DB **best-effort** (upstream catches and logs "skipped (DB busy)"); answer the report.

## Measured surface (2026-10-25, HEAD `8650cae`; re-locate by symbol, never by line)

| piece | state |
|---|---|
| `MaintenancePort::cleanup()` | exists, default body `Err(PortError::Unavailable)` — keep it (that is the degrade path's 501) |
| `CleanupReport` | 3 fields today, widening to 5 |
| `delete_old_prompts` | returns `u64` only, **1 occurrence in the tree** (its own definition) — to be replaced |
| `linked_memory_id` | column exists (`sql.rs:53`, index `:132`); **never written at runtime** |
| `memories` | per-shard; `id TEXT PRIMARY KEY`; **schema becomes v3** — this wave does touch it |
| batch primitive | `Db::batch(…, BatchMode::Write)` → `transactional_batch(Immediate)` (`db.rs`), already used by `schema_migrations` and `capture_inbox::mark_done` |
| shard schema application | `ShardManager::ensure_shard_schema` is **`pub`** and version-gated — the sweep calls it per shard (migrate-on-touch). Do **not** make `open_shard` migrate: that would turn every read into a potential write. |
| shard enumeration | `ShardManager::get_all_shards(scope, None)` |
| deletion primitives | `vector::delete_memory_by_id`; `ShardManager::decrement_vector_count`; `UserPromptManager::vacuum` |
| SQL placement | v3 statements + the two `memories` scans in a new `crates/mem-store/src/prompt_expiry_sql.rs` (`episodic_sql` precedent). **`sql.rs` is 100 % upstream-cited — do not touch it, and do not amend the frozen v1 `memories` DDL**: the column arrives as v3, exactly like `session_id` arrived as v2. |
| migration churn | `schema_migrations_runner.rs` `version == 2` ×2 → 3, `columns.len() == 18` → 19; `schema_migrations.rs` must **filter** (`m.version <= 2`) rather than bump `applied_v2`, preserving that test's intent |

## Out of scope

- Scheduled cleanup → issue 41. Deduplication → issue 40. Wiring capture's `link_memory_to_prompt`.
- `ai_session_retention_days` is parsed and unused; no AI-session store exists. Do not report
  `ai_sessions_deleted: 0`.

## Acceptance criteria

- [ ] `POST /api/cleanup` → 200 + the 5-field report on a seeded store; no `unwrap`/`expect` on the path
      (ADR-0013); degrade path (no composition) → 501 with the documented message.
- [ ] **Every pre-existing row is `NULL` after migration, and an expired memory with `NULL` is deleted** —
      this is the regression that matters, because today's entire store is in that state.
- [ ] A memory with `prompt_expired_at >= cutoff` and an old `updated_at` **survives**; with
      `prompt_expired_at < cutoff` it is **deleted**; at exactly `== cutoff` it **survives**.
- [ ] Monotonic: a later stamp cannot shorten a window (`max` semantics); duplicate links cannot violate a
      key (there is none).
- [ ] Stamps are written **before** prompts are deleted; a crash between phases re-stamps idempotently and
      can only lengthen protection. (No claim of cross-file simultaneity — see the ADR sentence.)
- [ ] Deleting a memory clears `linked_memory_id` on prompts that referenced it.
- [ ] `vector_count` decremented exactly once per deletion; no orphan vectors.
- [ ] `POST /api/migration/*` → **501** (not 500) when the service is unavailable.
- [ ] v2 → v3 migration: one open upgrades; second apply is a no-op; a pre-v3 row reads `NULL`.
- [ ] Config untouched (`auto_cleanup_enabled` stays `true`); no new dependencies; manifests byte-identical.

## Test plan (five local storage tests + the issue's endpoint tests)

`crates/mem-store/tests/prompt_expiry_migration.rs` — v2 file → v3 in one open; fresh root reaches 3 in one
open; second apply returns 3 with zero work; a pre-v3 row reads `NULL`.

Shard-level retention (seed a shard, set/clear the column, run the sweep — **no prompt store needed**):

1. `updated_at < c AND prompt_expired_at IS NULL` ⇒ deleted (every existing row is this shape).
2. `prompt_expired_at >= c` with old `updated_at` ⇒ survives.
3. `prompt_expired_at < c` ⇒ deleted; `== c` ⇒ survives.
4. Monotonicity: stamp(10) then stamp(5) leaves 10.
5. Migration contract (above).

Endpoint tests (`crates/mem-web/tests/cleanup_retention.rs`, real shard + real prompts DB): the five-row
matrix (expired+unlinked deleted; expired+pinned survives; expired+live-stamp survives; expired+stale-stamp
deleted; young+live-stamp survives) with the 5 report fields and registry counts; the boundary pair; the
degrade 501; the migration-endpoint 501; the dangling-link clear; and the run guard under `tokio::join!`.

## Known risks

1. **Three phases, each idempotent** — a crash between them leaves prompts alive with stamps written; the
   next run re-stamps and deletes. Never a memory deleted early.
2. **`prompt_expired_at` is derived lifecycle state** — it must not be exported, and import yields `NULL`.
3. **Orphan stamps are impossible** (the column dies with the row) — this deletes A's risk 1 and risk 3
   entirely.
4. **Clock discipline** — read `now` once per run and reuse it for both `cutoff` and the stamp.
5. **Migrate-on-touch cost** — the first sweep after the upgrade migrates every shard it visits; later runs
   are no-ops. Accept it; do not hoist the migration into `open_shard`.

## ADR


**ADR numbering resolved 2026-10-25.** Hardening (issue 43) owns **ADR-0017 through ADR-0024** — it is
the active effort and its waves are in flight. The `docs/adr/0017-linked-memory-retention-restart.md`
named below therefore **renumbers when this issue is re-planned**; do not create it as 0017. The
decision it records (retention restarts at prompt death) is unchanged — only its number moves.

**`docs/adr/00XX-linked-memory-retention-restart.md`** (rewritten, not amended — the earlier draft described
a table). Status Accepted. The decision sentence names the **column** and the **ordering invariant**, and
the Grounds section records A and C as rejected with the reasons above.

## Comments

- Split out of issue 37 on 2026-10-25; grilled; architecture ruled A; **re-ruled B** the same day after
  external + vault research and a prototype that measured turso's actual engine (WAL by default, `ATTACH`
  refused, `ADD COLUMN` 1.2 ms / 20k rows). Two of the inputs to the first ruling were wrong: absence of a
  `journal_mode` pragma does not imply `DELETE`, and absence of a raw `execute_batch` does not imply no
  transactional batch.
- Sibling issue 40 covers `POST /api/deduplicate`.
- Landed 2026-11-10 via blueprint `blueprint-20261110-issue39-retention-hybrid-r2.md` (stamp `838b3b50ddbd`, SysEng approved). Owner ruled Hybrid retention design (capture-time stamp `source_prompt_created_at` + deterministic $k=2$ grace $p + 2r > c$ derived from stored facts); vector.rs-local constants preserve golden SQL freeze; 47.1-B2′ profile invalidation join implemented before prompt deletion. Full gates and reviewer APPROVED (`blueprint-20261110-issue39-retention-hybrid-r2.review-1.md`). Recorded in ADR-0032.
