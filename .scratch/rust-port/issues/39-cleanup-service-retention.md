# 39 — W8.1 · Make `POST /api/cleanup` real (retention cleanup)

Status: ready-for-agent — 2026-10-25 (grilled to completion; architecture ruled by `architect`;
all seven design decisions closed below. Blueprint may proceed.)

## What to build

`POST /api/cleanup` currently returns the documented 501 stub (D-15):
`pub async fn cleanup() -> Response { envelope::not_implemented(CLEANUP_UNAVAILABLE) }` — the handler
takes **no `State`**, so no port can reach it. Implement retention cleanup behind
`MaintenancePort::cleanup()`, rewrite the handler to call it, and fix the `maintenance_failure`
conflation (below).

## The divergence this issue exists for (read this first)

Upstream protects a memory for **one cleanup cycle** if its prompt is deleted in that same run — an
artefact of deleting the prompt and reading its links in one pass. The owner ruled to **diverge**:
**a prompt-linked memory's retention restarts when its prompt expires.**

Mechanism (architecture ruling, `architect`, 2026-10-25 — option A, chosen because it is the only one
that can record the expiry and delete the prompt in **one commit**, since both rows live in one
database):

- New table in `user-prompts.db`:
  `CREATE TABLE IF NOT EXISTS prompt_memory_expiry (memory_id TEXT PRIMARY KEY, expired_at INTEGER NOT NULL)`
- Written in the **same atomic write batch** as the prompt `DELETE` (`db.batch(…, BatchMode::Write)`),
  statements: prune → un-stamp → stamp (`INSERT … SELECT DISTINCT linked_memory_id, ?`) → delete.
- Read as a filtered set; **pruned by the same predicate that governs eligibility**, so a row older
  than the cutoff is indistinguishable from an absent row and its deletion is rule-preserving, not
  data loss.
- Eligibility: `updated_at < cutoff AND (no row OR expired_at < cutoff)`. Boundary is strict `<` on
  both sides, so `expired_at == cutoff` is **protected**.

DDL goes through the prompts DB's own established idiom (`CREATE TABLE IF NOT EXISTS` on open, like
`ensure_user_prompt_columns`) — **no** `PRAGMA user_version` bump and **no** shard migration. Failure
to create must be loud, not tolerated.

## Seven decisions taken (do not re-open)

1. **Protection set is implemented now**, even though `linked_memory_id` is never written at runtime
   (see Measured surface). Tests seed the column by direct SQL; no dependency on capture wiring.
2. **Grace rule = retention restarts at prompt death** (mechanism above).
3. **A live prompt protects nothing.** Only a dead one grants a window. This is upstream-identical
   and is what the owner's rule says; the AC must state it in that direction. Additionally, cleanup
   **clears `linked_memory_id`** on any prompt whose memory it deletes, so no dangling reference is
   left for the UI to render.
4. **`CleanupReport` widens 3 → 5 fields**: `deleted_count`, `user_count`, `project_count`,
   `prompts_deleted`, `protected_memories_skipped`. `ai_sessions_deleted` and
   `linked_memories_deleted` stay out — neither can be truthfully populated in this port.
5. **`expire_old_prompts(cutoff: i64, now: i64) -> Result<ExpiryOutcome, ProfileError>` replaces
   `delete_old_prompts`.** The old method has **zero callers** (verified); leaving both would create
   a seam that deletes links *without* stamping, and the next wave reaches for the shorter name.
   `ExpiryOutcome { prompts_deleted: u64, protected_memory_ids: HashSet<String> }` — the returned set
   is the query the tick already ran, so cleanup needs no second round trip.
6. **Ship the run guard.** Nothing prevents two concurrent `POST /api/cleanup` runs; both would
   delete and both would count, inflating the report. One `AtomicBool`/`try_lock` in the adapter.
7. **Manual endpoint only.** No scheduled tick in this wave (`auto_cleanup_enabled` is read but not
   yet acted on). Follow-up issue 41 carries the scheduler.

**`auto_cleanup_enabled` keeps its upstream default `true`** (owner decision, 2026-10-25 — reverses an
earlier decision to flip it off). Nothing reads the flag in this wave: the manual endpoint does not
consult it and no scheduler exists yet, so it cannot delete anything. The "deletes on first boot"
risk belongs to issue 41, where the timer and the default are decided together. **Do not touch
`DEFAULTS.auto_cleanup_enabled` or `crates/mem-core/tests/config_defaults.rs`.**

## Included defect fix

**`/api/migration/*` answers 500 where it should answer 501.**
`maintenance_failure` (in `crates/mem-web/src/api/maintenance.rs`) funnels
`PortError::Unavailable` through `port_failure` → `internal_error()` (500), while `session_failure`
correctly maps the same condition to `not_implemented` (501). Make `maintenance_failure` mirror
`session_failure`. This was found while splitting issue 37.

## Upstream behaviour to match (`example/opencode-mem/src/services/cleanup-service.ts`)

Cutoff from `autoCleanupRetentionDays`; collect pinned ids across **all** user and project shards;
expire old prompts; protect; per shard select `id, container_tag, is_pinned WHERE updated_at < cutoff`,
skip pinned/protected, delete vector + `decrementVectorCount`, count `_user_`/`_project_` by
`container_tag`; `VACUUM` the prompts DB (**best-effort**: upstream catches failure and logs
"skipped (DB busy)"; mirror that, do not fail the run); answer the report.

## Measured surface (2026-10-25, HEAD `9f4853b`; verify, do not inherit)

| piece | state |
|---|---|
| `MaintenancePort::cleanup()` | exists, default body `Err(PortError::Unavailable)` (`crates/mem-web/src/api/ports.rs`) |
| `CleanupReport` | `{ deleted_count, user_count, project_count }` — widening to 5 |
| `UserPromptManager::delete_old_prompts` | returns **`u64` only**; reads no links; **zero callers**. To be replaced. |
| `linked_memory_id` | column exists (`crates/mem-store/src/sql.rs:53`, index `:132`); **never written at runtime** — `link_memory_to_prompt` (`crates/mem-profile/src/prompt.rs:359`) has zero callers, upstream calls it from `auto-capture.ts:187` |
| `user-prompts.db` | separate DB, owned by `UserPromptManager` (`USER_PROMPTS_DB_NAME`, `prompt.rs`) |
| `memories` | per-shard DB (`sql.rs:64`), golden-SQL tested, versioned migrations — **not touched by this issue** |
| `ShardManager::get_all_shards(scope, Option<&str>)` | exists (`shard_manager.rs:415`); `None` = upstream `""` wildcard |
| deletion primitives | `mem_store::vector::delete_memory_by_id(db, id)`; `ShardManager::decrement_vector_count(shard_id)`; `UserPromptManager::vacuum()` |
| config | `auto_cleanup_enabled`, `auto_cleanup_retention_days` parsed — no new config |
| SQL placement | net-new statements belong in a **new `crates/mem-store/src/prompt_expiry_sql.rs`** (+ `pub mod`), following the `episodic_sql` precedent. **Not** in `golden_sql.rs` — `sql.rs` is 100% upstream-cited and must stay so. |

## Out of scope (state it, don't silently omit)

- **Scheduled/daily cleanup** → issue 41.
- **`ai_session_retention_days`** is parsed and **never used**; there is no AI-session store anywhere
  in the port. Known gap, not this wave; do not report `ai_sessions_deleted: 0`.
- **Memory deduplication** → issue 40.
- **Wiring capture's `link_memory_to_prompt`** — the protection is built and tested now, but the
  column stays unwritten until that separate parity gap is closed.

## Acceptance criteria

- [ ] `POST /api/cleanup` answers 200 with the 5-field report on a seeded store; no `unwrap`/`expect`
      on the path (ADR-0013); degrade path still answers the documented 501 when there is no
      composition.
- [ ] An expired, unpinned, unlinked memory is deleted; a pinned one survives; a memory whose linked
      prompt expired **less than `retention_days` ago** survives; a memory whose linked prompt expired
      **more than `retention_days` ago** is deleted. (Note the direction: a *live* prompt protects
      nothing.)
- [ ] `prompt_memory_expiry` is written in the same atomic batch as the prompt delete; a second run
      with the same args is idempotent (`prompts_deleted == 0`, set unchanged, one row per memory);
      the row is pruned when its window ends.
- [ ] Duplicate links (two expired prompts → one memory) produce one row, no PK violation; a later
      expiry moves the stamp forward.
- [ ] Deleting a memory clears `linked_memory_id` on prompts that referenced it.
- [ ] `vector_count` decremented exactly once per deletion; no orphan vectors.
- [ ] `POST /api/migration/*` answers **501** (not 500) when the service is unavailable.
- [ ] Config untouched: `auto_cleanup_enabled` keeps its `true` default and no config test changes.
- [ ] No new dependencies; manifests byte-identical; production panic-free.

## Test plan (seeded directly — no capture wiring)

`crates/mem-profile/tests/prompt_expiry.rs` (real turso via `UserPromptManager::new`;
`INSERT INTO user_prompts (id, session_id, message_id, content, created_at, linked_memory_id)` because
`save_prompt` cannot set the link):

1. tick stamps and deletes — expired linked p1, fresh linked p2, expired unlinked p3 → p1/p3 gone,
   `protected_memory_ids == {m1}`, row `(m1, now)`, `prompts_deleted == 2`.
2. idempotent second run — `Ok`, `prompts_deleted == 0`, set unchanged, exactly one row.
3. window ends — re-run with `cutoff = now + 1` → empty set **and** the row is gone (proves
   prune == eligibility collapse).
4. duplicate links — two expired prompts → one memory: one row, no PK error.
5. re-stamp — a later expiry moves `expired_at` forward.
6. empty link column — all `NULL` → empty set (today's whole world; upstream-parity guarantee).

`crates/mem-store/tests/` (extend `turso_auxiliary_db_upgrade.rs` or add `prompt_expiry_table.rs`):

7. `CREATE TABLE IF NOT EXISTS` applied twice is `Ok`; a legacy DB **without** `user_prompts` still
   opens (existing `turso_auxiliary_db_upgrade.rs:149` case must not regress).

`crates/mem-web/tests/` cleanup integration (real shard via `memory_shard_migrations`, registry via
`ShardManager`, expiry rows seeded directly):

8. five memories — expired+unlinked (deleted), expired+pinned (survives), expired+live-stamp
   (survives), expired+stale-stamp (deleted), young+live-stamp (survives); assert the 5 report fields
   and `vector_count` decremented exactly twice.
9. boundary — `updated_at == cutoff` not selected; `expired_at == cutoff` protected.
10. degrade + the 501 fix — `UnavailablePort` ⇒ cleanup 501; `PortError::Unavailable` on
    `/api/migration/*` ⇒ **501, not 500**.

## Known risks (from the architecture ruling)

1. **Fail-open on prompt-store loss** — if `user-prompts.db` is deleted, live windows vanish and
   linked memories revert to their own cutoff. No code path does this today.
2. **A future writer may mis-stamp** — when capture wires `link_memory_to_prompt`, someone may stamp
   at *link* time rather than *expiry* time. Mitigation: module doc + `mem-profile/CONTEXT.md`
   glossary line — "the expiry tick is the only writer of `prompt_memory_expiry`".
3. **Orphan rows** — UI deletion of a memory never clears its expiry row; harmless, self-pruning, and
   id reuse would fail *safe*.
4. **Clock discipline** — read `now` once per run and reuse it for both `cutoff` and the stamp.
5. **Non-atomicity across DBs** — the shard deletions cannot join the prompts DB batch; a crash
   mid-run leaves the stamp written and some memories undeleted. Harmless (the next run deletes
   them), but it means "one run" is not transactional across tiers.

## ADR

Record **`docs/adr/0017-linked-memory-retention-restart.md`**, status Accepted, single sentence:

> A prompt-linked memory's retention restarts when its prompt expires: the prompt's deletion stamps
> `prompt_memory_expiry(memory_id, expired_at)` in `user-prompts.db` in the same atomic write batch,
> and a memory is deletable only when both its own `updated_at` and that stamp are older than the
> retention cutoff.

Consequences to list: upstream's one-cycle protection becomes `retention_days`; the prompts DB gains
a table pruned by the predicate that governs eligibility; `delete_old_prompts` is superseded; the
expiry tick is the sole writer. No config default changes: `auto_cleanup_enabled` keeps
upstream's `true`, and the scheduler that would act on it is issue 41.

## Comments

- Split out of issue 37 on 2026-10-25. Issue 37's AC6 covered "wire sessions **and** maintenance";
  measurement showed `cleanup()`/`deduplicate()` take no `State` and are unconditional 501 stubs, so
  a port cannot make them real — the work is service implementation, not composition.
- Grilled to completion 2026-10-25 (six questions): protection set, grace rule, storage location
  (deferred to `architect`, ruled option A with a prototype), live-link semantics, report width,
  scheduling.
- Sibling issue 40 covers `POST /api/deduplicate`.
