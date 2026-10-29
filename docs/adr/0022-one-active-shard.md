# 0022 — One active shard per `(scope, scope_hash)` is a database constraint

- **Status:** Accepted (owner acceptance, 2026-10-30) — residuals R-22-1…R-22-5 accepted as stated; R-22-2 is tracked as issue 77.
- **Date:** 2026-10-29
- **Builds on:** `docs/adr/0018-shard-facts-source-of-truth.md` (the shard owns its facts; the registry
  count is a cache), `docs/adr/0021-schema-version-discipline.md` (`max_known` is mandatory and a newer
  file is refused before the pending check).
- **Blueprint:** `bp-20261029-0937-issue43-w4-one-active-r4` (sha256 `86303d660aec`, SYS-APPROVED
  2026-10-29).
- **Wave:** issue 43 W4, base `e13c036`. W5 (prompts v2) and W7 (reaping) are out of scope.

## Context

`metadata.db.shards` has `UNIQUE(scope, scope_hash, shard_index)` and a **non-unique** index on
`(scope, scope_hash, is_active)` (`sql.rs::SHARDS_ACTIVE_INDEX`). Nothing stops a second `is_active = 1`
row for one scope, and the write contract only ever says *at most one* by convention. Two opposite
failures are reachable:

- **Two actives.** `register_existing_shard(…, is_active = true)` upserts `is_active =
  excluded.is_active`; the path-migration rollback, the inventory registration and the legacy reconcile
  all call it, and upstream has the same defect. `get_active_shard` returns the highest `shard_index`
  (`ORDER BY shard_index DESC LIMIT 1`), so the lower active row is orphaned-active forever.
- **Zero actives.** The shipped rotation is *deactivate, then create*. A crash — or just a failed
  create, e.g. the target index was pre-registered — leaves no active row. `get_active_shard` returns
  `None`, the resolver calls `create_shard(…, 0)`, the row at index 0 still exists, the insert hits the
  shard-index unique constraint, and the dedup returns that **inactive** row as the write shard. Writes
  land in a retired shard and the scope never rotates again. "One active" without the zero-active half
  is only "at most one".

This ADR makes the invariant a database constraint (a partial unique index in `metadata.db` v2) **and**
a code rule (one-transaction rotation; a zero-active scope reactivates its highest row through the same
validation as any other write shard), with typed conflicts instead of raw engine text.

## Grounds

Every claim below was re-derived from the tree and from turso 0.8.3 probes run on 2026-10-29 (probe
source: `.tmp/scratch/proto-turso-probes/src/bin/probe_w4.rs`).

| claim | verdict |
|---|---|
| turso 0.8.3 supports a partial unique index and enforces it | **TRUE.** `CREATE UNIQUE INDEX … ON shards(scope, scope_hash) WHERE is_active = 1` over two actives is refused `Error::Constraint("UNIQUE constraint failed: shards.(scope, scope_hash)")`; enforcement survives close/reopen; three inactive rows in one tuple and another tuple's active row are accepted. |
| the migration can repair then create the index in one transaction | **TRUE.** `transactional_batch(Immediate)` with `[UPDATE repair, CREATE UNIQUE INDEX]` commits both (`results = 2`); re-running the same batch is a no-op and commits. |
| a failed batch rolls back its earlier statements | **TRUE.** `[UPDATE deactivate, INSERT duplicate key]` → `BatchStatementFailed`, and the deactivate was rolled back (the row was still active). This is the atomicity the rotation depends on. |
| the "two active rows" window is the only one | **FALSE.** Rotation is deactivate-then-create, so a failure leaves **zero** actives; the dedup then returns the inactive row as the write shard. W4 must fix both halves. |
| the highest-index repair rule is arbitrary | **No.** It is what the code already applies in three places: `GET_ACTIVE_SHARD`'s `DESC LIMIT 1`, the legacy migrator's highest-index map, and the inventory default. |
| `CHECK (is_active IN (0,1))` is a cheap rider | **FALSE.** `ALTER TABLE … ADD CONSTRAINT` is a syntax error in this engine; a CHECK needs a full table rebuild. Rejected. |
| a constraint inside `BatchMode::Write` keeps `is_constraint` | **FALSE.** It surfaces as `BatchStatementFailed`; `is_constraint == false`, while `StoreError::is_unique_violation()` (substring) is still true. Classification must re-read the registry, never branch on `is_constraint`. |
| the fixtures can build the pre-migration state through the live connection | **TRUE.** `DROP INDEX IF EXISTS idx_shards_one_active`, `PRAGMA user_version = 1`, and plain `UPDATE shards SET is_active = 0` all succeed; re-creating the index over two actives is refused again. |
| shipping v2 under `METADATA_MAX_KNOWN = 1` is survivable | **FALSE, the wave's sharpest edge.** `apply_schema_migrations` refuses at `current > max_known` **before** the pending check, and `initialize_metadata_db` re-inits on `reset()`/close, so the first reopen dies with `SchemaNewerThanBuild { found: 2, supported: 1 }`. |
| a v1→v2 metadata migration can take the runner's backup | **FALSE in practice.** `requires_backup(Some(path), 1)` sees the file, and callers without the engine lock get `NoExclusiveRoot` **before** the batch. `db_path: None` is required, not stylistic. |
| `reconcile_shard_registry` is safe under the new index | **Not in the ordinary case, yes in one specific one.** Registration runs in filename order and only the highest file is registered active; the incumbent has already been deactivated. The conflict needs the incumbent's registry row to survive while its file is absent from the parsed set — then nothing deactivates it and the discovered higher-index file cannot be activated. |

## Decisions

**D-22-1 — Enforce at most one in the database with a partial unique index.**
`CREATE UNIQUE INDEX IF NOT EXISTS idx_shards_one_active ON shards(scope, scope_hash) WHERE is_active =
1`, created in `metadata.db` v2. *Rejected:* code-only enforcement — it cannot bind the import and
path-migration writers, which are exactly the ones that create a second active row.

**D-22-2 — Repair ambiguous registries, do not refuse.**
The v2 batch deactivates every active row that has a higher-index active in the same tuple, then creates
the index in the **same** transaction. The survivor is provably the row `get_active_shard` was already
returning. *Rejected:* fail-closed with a typed error naming the duplicates — it bricks every affected
root with no in-tree remedy. The index remains the backstop: if the repair is insufficient, the batch
rolls back and the file stays v1.

**D-22-3 — v2, its ceiling and `db_path: None` land together.**
The migration version, `METADATA_MAX_KNOWN = 2` and the backup-free option are one change: any split has
a state where a real root cannot reopen (v2 under a v1 ceiling) or cannot migrate (`db_path: Some`
returns `NoExclusiveRoot` before the batch for callers without the engine lock). *Rejected:* a separate
ceiling step — its gate is impossible without a red full suite or a scoped gate that hides the brick.

**D-22-4 — Rotation is one transaction, as a new explicit primitive.**
`rotate_shard(scope, hash, retire_shard_id, next_index)` initializes the file, then runs one
`BatchMode::Write` batch `[MARK_SHARD_INACTIVE_IN_SCOPE, CREATE_SHARD]`. *Rejected:* teaching
`create_shard` to retire the previous active — it would silently retire a live shard from a method whose
upstream contract is "insert", and it would make the conflict unreachable and therefore untestable.

**D-22-5 — The reactivation path is the ordinary `Some` path.**
When no row is active, `resolve_write_shard` activates the highest `shard_index` row and then
`continue`s into the existing `Some` arm, so the reactivated shard passes `is_shard_valid`,
`prepare_write_shard` (W3's fingerprint refusal), the count sync and the cap check. *Rejected:* a
reduced tail inside the activation helper — two paths to keep in sync, and a foreign-fingerprint or full
shard could accept writes. *Rejected:* creating index 0 when none is active — it resurrects the oldest,
out-of-order row.

**D-22-6 — A second activation is a typed conflict, classified by re-reading the registry.**
Both the target key row and the tuple's active row are probed, and a success is never returned while a
*different* row is active. The caller's mode then chooses the variant: `Dedup` (`create_shard`) and
`Activate` (the `register_existing_shard` upsert) report `ActiveShardConflict`; `Occupied`
(`rotate_shard`) reports `ShardIndexOccupied`, because a rotation never treats an existing row as
success. *Rejected:* key-first classification — the measured case (row 1 pre-created inactive, row 0
active, upsert index 1 → active) raises a constraint while the key row exists, and a key-first
classifier returns `Ok(inactive row)`, silently dropping the activation. *Rejected:* parsing the
message into a verdict — the message only decides "worth probing".

**D-22-7 — Metadata v2 takes no pre-migration backup.**
`init_metadata_db` passes `db_path: None`. Nothing is deleted; the only mutation is an `is_active` flag
on a redundant row; the batch is atomic. *Rejected:* keeping `db_path: Some`, which makes the hot open
path a settle site (`NoExclusiveRoot` without the engine lock, `MigrationRestorePending` with it) for no
recoverable state.

**D-22-8 — `reconcile_shard_registry` skips a discovered file it cannot activate, and only that case.**
On `ActiveShardConflict` the reconcile warns (file path + incumbent) and continues; every other error
still aborts. The ordinary re-registration never needs it: the ascending filename order deactivates
every lower file before the highest registers. *Rejected:* repairing at reconcile (activating the
discovered file would change the write target on every open based on file presence); *rejected:* a
two-pass registration (all `false`, then the highest `true`) — it has no RED oracle of its own, because
without the index the single-pass order already ends one-active. The lexical-inversion residue is
R-22-5 and is healed by the resolver on the next write.

## Residuals

- **R-22-1 — the index is an engine capability.** turso 0.8.3 partial indexes are measured above; a
   future bump may change enforcement or the constraint message. *Inverted world:* T1 fails first,
   before any data is touched.
- **R-22-2 — the three `register_existing_shard` call sites change failure mode.**
   `shard_path_migration.rs:218` (rollback of an archived target) now fails with `ActiveShardConflict` →
   `rollback_failed` → the journal is kept → `recover_interrupted_swap` returns
   `Internal("Failed to recover path-migrate swap …")` → the store refuses to open; `:619` (inventory
   registration) aborts the migration with the typed conflict; `legacy_migrator.rs:848` is the reconcile
   site, now skipped (D-22-8) rather than fatal. The failed attempt consumes the archive, so the **next**
   open returns early from `rollback_archived` and completes: the cost is one refused start, not an
   unopenable store. *Inverted world:* an operator reporting "refuses to open — Failed to recover
   path-migrate swap" names a conflicting activation during rollback; the remedy is to retry, and only a
   retry that also fails calls for inspecting the journal and registry by hand. *Trigger to act:* the
   first such report. Tracked as **issue 77**, which also carries the state a test must build.
- **R-22-3 — `reassign_shard_scope` is unchanged.** A path migration moving an active shard into a tuple
   that already has one surfaces the engine's constraint; the path-migration path already aborts on any
   registry error. *Inverted world:* a path-migration abort citing a unique constraint on `shards` names
   a genuinely ambiguous target.
- **R-22-4 — mem-web is unchanged.** `store_failure` maps `ActiveShardConflict` through the wildcard to
   the generic 500; it is unreachable under the scope write lock, and its Display carries the tuple and
   both indices for the log. *Trigger to act:* a field log showing one.
- **R-22-5 — reconcile skips an unresolvable activation, and one ordering leaves zero active.** The skip
   leaves the discovered file unregistered until W7 reaps or an operator inspects it; the registry stays
   truthful about what it has. Separately, a **lexical inversion** (`shard_10` sorts before `shard_2`)
   makes the highest register before the incumbent's `is_active = 0` upsert: the active insert is
   skipped and the later `false` upsert leaves the tuple zero-active at rest. Nothing corrupts: the
   resolver reactivates the highest registered row on the next write (D-22-5), and the reconcile runs
   again on the next open. Not fixed here — a two-pass ordering has no RED oracle of its own, so it
   would be an untested restructure. *Inverted world:* an inventory report naming an unregistered shard
   file, or `get_active_shard` returning `None` for a populated scope right after an open, is this path
   — not data loss, and the next write repairs it.
