# 0023 — Retired-shard reaping: row first, files under the engine-lock proof

- **Status:** Proposed.
- **Date:** 2026-10-30.
- **Builds on:** `docs/adr/0022-one-active-shard.md` (one active row per `(scope,
  scope_hash)`; a zero-active tuple reactivates its highest row),
  `docs/adr/0019-erasure-posture.md` (A14/R-19-9, amended here for the reap case),
  `docs/adr/0018-shard-facts-source-of-truth.md` (the shard file owns its count).
- **Blueprint:** `bp-20261029-2044-issue43-w7-reaping-r4` (sha256 `ea0d7f1af425`,
  SYS-APPROVED 2026-10-29T20:55Z).
- **Wave:** issue 43 W7, base `4307662`. W5 (prompts v2) is out of scope.

## Context

Retention and delete already empty a shard logically; the files remain. Every
scope-wide search, retention run and reconcile then opens those dead files. W7
removes them — the only wave in issue 43 that destroys data: no undo, no
rollback, and the report is the operator's record of what survived.

The wave rests on the invariant ADR-0022 made a database constraint: every
`(scope, scope_hash)` tuple has one active row, and a zero-active tuple
reactivates its **highest** row on the next write (D-22-5). Reaping therefore
never touches an active row and never removes the highest row of a zero-active
tuple. The second safety layer is issue 46: no unlink of a database or its
sidecars without the engine lock plus a `Free`/`Missing` database-file probe.
The third is the `-tshm` liveness sensor: turso's multiprocess shared memory is
queried at byte 0 (`F_OFD_GETLK`/`F_GETLK`, issue 60 — queried, never taken) and
`Held`/`Unknown` skip.

Owner rulings 1–15 (`.scratch/rust-port/issues/43-database-base-hardening.md`,
block "W7 OWNER RULINGS 2026-10-30") bind the surface: an explicit CLI
subcommand, dry-run by default, `--apply` alone as the confirmation, the
deletion unit is the shard, the store owns the safety conditions, and the yield
is cleanup after deletion, not space reclamation.

## Grounds

| claim | verdict |
|---|---|
| `COUNT(*)` from the shard file is the only zero proof | **MEASURED (W2/ADR-0018).** `shard_facts::read_stored_count` returns `0` when its row is absent (`shard_facts.rs:62-68`, `.unwrap_or(0)`), so it would call every pre-v3 shard empty; `SHARD_STATS_DDL`'s contract says no row is written by the v3 migration. |
| a read-only turso open is a clean zero-count proof | **MEASURED (this wave).** The T3 fixture inserts one row with `vector::insert_vector`, never runs `sync_count`, and reads `COUNT(*) = 1` through a read-only open; the registry and `shard_stats` both read 0. |
| turso opens `-tshm` `O_RDWR` even on a read-only database open | **MEASURED (turso 0.8.3).** `turso_core-0.8.3/database.rs:2952-2967` → `storage/shared_wal_coordination.rs:1000-1005` → `io/unix.rs:50-57`; a `0444` sidecar fails the count open with EACCES (`unopenable`) before the probe runs — which is why the `Unknown` arm is a unit-test seam, not a fixture. |
| a live writable connection makes the byte-0 `-tshm` query answer `Held` | **MEASURED (this wave).** T7 holds a shared `F_RDLCK` OFD lock on byte 0 and the probe answers `Held`; the CLI's ready-gate reconcile left cached writable handles and the probe answered `Held` for an otherwise eligible candidate until the CLI closed them. |
| a clean close never checkpoints, so `-wal` survives | **MEASURED (this wave).** The T1 fixture captures `-wal` 74,192 B and `-tshm` 86,016 B beside a 4,096 B `.db` after the writable creation; the dry-run CLI child leaves the same set. |
| the ready-gate legacy step can adopt an orphan file before the report | **READ (blueprint and `legacy_migrator.rs:934, :981`).** `reconcile_shard_registry` re-registers every parsed shard file; the store-level dry run never runs it. |
| `-shm` is a target but not a fixture guarantee | **MEASURED (this wave).** The T1 fixture set has no `-shm`; the removal still attempts it (`NotFound` is success). |
| the legacy marker branch requires a complete per-shard sidecar | **MEASURED (this wave, CLI-14).** A shard created after the dry-run child has no `.turso-migrate.json`, so `all_shards_complete` is false and the apply child re-runs `migrate_storage`, writing that sidecar inside the scope dir — the `0o555` directory then fails the ready gate (EACCES, exit 74) before the reap. |

## Decisions

**D-23-1 — The predicate is five conditions, fail-closed; execution order and skip-reason precedence are separate; `skipped` lists every considered row.**
*Execution order:* `is_active` → `HighestWithoutActive` (registry-only) → read-only open + `COUNT(*)` → drop the handle → `-tshm` probe → (dry run stops here) → row delete → **re-probe `-tshm`** → file removal. *Skip-reason precedence* (what the report names when several would fail): the first condition in that order. *Report contract:* `skipped` is every registry row considered and not reaped — active rows included — with the precedence's reason; `candidates` are the rows that passed the predicate. At the **predicate snapshot** a row is in exactly one of `candidates`/`skipped`; on apply a candidate that fails (`RowGone`/`FileDeleteFailed`) **stays in `candidates` and is additionally listed in `skipped`**, and a reaped candidate stays in `candidates` and appears in `reaped`. The exclusive-or is a predicate-snapshot statement, never an apply-time one. The two lists match at snapshot time; keeping them separate makes the probe's late position explicit instead of implying it is evaluated before the file is even opened.

**D-23-2 — The zero-count proof is `COUNT(*)` from the shard file; the stored-count read-back cannot be the oracle.**
`shard_facts::read_stored_count` returns 0 when its row is absent; that collapse is exactly why it cannot be the oracle: every pre-v3 shard — the oldest debris — would read as empty. `SHARD_STATS_DDL`'s contract says no row is written by the v3 migration and a `0` without a measurement would be a W7 trigger. The reaper re-measures `COUNT(*)` (`shard_facts::read_count`) in the same read-only open; it never trusts the registry's `vector_count` nor an absent `shard_stats`. *Rejected:* `read_stored_count` and the registry count.

**D-23-3 — The unit is the shard: registry row first, then the file set; the removal reporting is additive.**
The row is deleted first (confirmed by `changes == 1`); only then does the 46-claimed `remove_sqlite_database_reporting` unlink `.db` → `-wal` → `-shm` → `-tshm`. This function is **new and additive**: `remove_sqlite_database` and `unlink_sqlite_files` are untouched, so no shipped caller's stop-set changes. The new function probes once at the top, attempts the `.db` first, and a `.db` failure returns with that one failure and leaves every sidecar alone: a clean close never checkpoints (W0), so a `-wal` is load-bearing data of a surviving database. Once the `.db` is gone, each sidecar is attempted independently; `NotFound` is success exactly as `unlink_sqlite_files` treats it; every other failure is collected (one log line per failed file). The shard is "reaped" only when the row and every pre-existing file are gone; otherwise it is `skipped [file_delete_failed]`. `bytes_freed` sums only paths that were **regular files** before and are gone after, so a directory at a sidecar target is a failure but never bytes. *Rejected:* reusing `delete_shard` (it unlinks first, then deletes the row — the inverse order, and no 46 claim); mapping the old all-or-nothing function's first failure to `Err` (it would change the shipped stop-set); a sidecar-only sweep (46's split hazard).

**D-23-4 — `-tshm` dies only with its database; ADR-0023 amends ADR-0019 A14 for the reap case.**
It is never deleted as debris and never under a live holder; it is removed only as the last element of a shard's file set whose `.db` is already gone. This reconciles the two rulings: the sensor ("retain permanently", `43:106`; ADR-0019 D-19-9/A14) and the debris finding (`43:447`). **ADR-0019 A14's "retained permanently" is narrowed, with an amendment note in `0019-erasure-posture.md`, for the reap case**, citing owner ruling 3 as the authority: the erasure sweep and debris never delete a `-tshm`; a reap may delete it only as the last element of its own shard's file set, after the `.db`, under the 46 proof, never alone and never under a live holder. *Rejected:* a sidecar-only sweep — issue 46's measured split (a live holder keeps the old inode, the next opener creates a new `-tshm`) and the sensor's own purpose make it the one deletion that is never legal alone. *Rejected:* leaving A14 unamended — it would contradict a `-tshm` deletion that this ADR authorises.

**D-23-5 — One root-wide entry point; the candidate set is the registry, post-ready-gate at the CLI.**
`reap_retired_shards(dry_run)` is the only API: no per-shard API, no plan/apply pair (the two acts have a TOCTOU window, ruling 11). A file with no row is invisible **to the primitive**; a filesystem scan is rejected because it has no registry truth, races the reconciler and would make the second run's no-op depend on the filesystem. `dry_run` is a parameter, not a second act: the store-level dry run takes both locks, builds the same report, mutates nothing, and returns before the first row delete. **End-to-end, however, the CLI's candidate set is the registry after the ready gate**, and the ready gate's legacy step reconciles (`legacy_migrator.rs:934`, `:981`), so it can adopt an orphan file as a row before the report: a CLI dry run is not side-effect-free (it may add registry rows; it never removes files). *Rejected:* claiming the CLI dry run is a pure read.

**D-23-6 — The highest row of a zero-active tuple is the reactivation target (ADR-0022 D-22-5).**
ADR-0022 guarantees at most one active row per tuple and reactivates the **highest** row when none is active; reaping it would make a lower retired shard the write target (and could reap the only row able to accept the next write). In a tuple **with** an active row no reactivation can fire, so its highest inactive row may be reaped (ruling 5). *Rejected:* applying the highest rule unconditionally — it skips the common production case forever.

**D-23-7 — Concurrency: engine lock + operation lock + `-tshm` queried twice.**
The engine lock is issue 46's binding structural proof (any unlink needs `ExclusiveRoot` + a `Free`/`Missing` database probe); the operation lock is the inter-operation mutex (ruling 7); the `-tshm` query is the liveness sensor for a **foreign** flag-on holder that takes neither of our locks. The probe queries (`F_OFD_GETLK`/`F_GETLK`, issue 60) and never takes; `Held` and `Unknown` skip. It runs **twice**: in the predicate (before the row is touched, so an obvious holder costs no registry write) and again **after the row delete, immediately before the first unlink** (so a holder that arrived while the read-only count open was in flight is seen). The window between the second probe and the first unlink is the residual (46's no-identity-recheck gap). A flag-off **holder** is invisible to the `-tshm` sensor; a flag-off engine of this build is caught by the engine lock, and a writable foreign flag-off holder by the 46 database probe. The class invisible to all three is a foreign **read-only** opener, which takes no lock; the dry run is the backstop for it. A **cached writable handle in the same process** also answers `Held` — the CLI therefore closes the ready gate's cached connections before the predicate (a measured requirement, see "Implementation notes"). *Rejected:* operation-lock-only reaping — it would delete files a live engine has open, and it cannot satisfy issue 46's proof for a single unlink. *Rejected:* a single probe in the predicate — a late flag-on holder would be invisible.

**D-23-8 — The CLI is dry-run by default; `--apply` alone is the confirmation; the exit-code map is this plan's decision, aligned to the house.**
`opencode-agents-memory --mode=reap-shards [--apply] [--storage-root <path>]`. **Exit codes:** 0 success including nothing to do; **1** a file-operation failure with a completed report (at least one `FileDeleteFailed` — nothing in `reembed`/`purge` covers this class); **64** usage (`reembed`'s invalid scope and `purge`'s invalid scope/confirmation both use 64); **74** store/operation failure including a lost engine lock (`reembed` returns 74 on a lost lock; `purge` uses 74 for `Failed`); **75** busy, i.e. the operation lock/legacy step is held (`purge` uses 75 for `Busy`). `--apply=<value>` and `--apply <token>` (non-flag) are 64. No `--confirm` string, no interactive prompt, no two-act design (ruling 14). One JSON line on stdout (`{"event":"reap.completed","dry_run":…,"candidates":[…],"reaped":[…],"skipped":[…],"bytes_freed":…}`); `structured_stderr` for refusals (`reap.refused`, `reap.busy`, `reap.failed`) and one `reap.file_delete_failed` line per failed file. The dry-run default is **not** a promise of a pure read: the ready gate reconciles before the report (D-23-5), so a CLI dry run may add registry rows — never remove files. *Rejected:* inventing an owner exit-code ruling (none is recorded) and inheriting 74/75 wholesale (the completed-report file failure is not an `Err`; 1 is the distinct code).

**D-23-9 — Both crash windows are benign and named.**
Row-first: a crash between the row delete and the file delete leaves the intact file, which the reconciler re-adopts on the next open (as inactive when it is below the tuple's highest, or skipped by D-22-8 when it is the highest beside an active row). File-first would leave a row pointing at a missing file — the worse window, which is why the file-first mutant is the T10 oracle. A partial file deletion leaves orphan sidecars (R-23-1); they are unreachable while the `.db` is absent, **not permanently inert** — a later `.db` minted at the same path can pair with a stale `-wal`, and issue 53 measured a stale `-tshm` inert, not a stale `-wal`. No undo; nothing is rolled back.

**D-23-10 — What W7 does not delete, and what the yield is.**
It does not delete active shards, shards with rows, live-held shards, the highest row of a zero-active tuple, unopenable/corrupt shards, `.bak` files (ADR-0019 D-19-6's erasure sweep owns them), files with no registry row, `metadata.db`, the vault, `user-prompts.db`, `episodic.db`, `facts.db`, or any non-shard file. The yield is **cleanup after deletion, not space reclamation** (ruling 15): retention/delete already emptied the shard logically; reaping stops every scope-wide search and retention run from opening a dead file. A shard that still holds live rows is never reapable, and the bytes counter is the apparent size of removed empty files, not reclaimed blocks.

## The `skipped` contract (stated once)

`skipped` lists **every registry row that was considered and not reaped** — active
rows included — with the fixed reason precedence `active` → `highest_without_active`
→ `unopenable`/`non_empty` → `live_tshm_holder`/`tshm_probe_unknown`. At the
**predicate snapshot** every considered row is in exactly one of
`candidates`/`skipped`; on apply, a candidate that fails (`RowGone`/`FileDeleteFailed`)
**stays in `candidates` and is additionally listed in `skipped`**, while a reaped
candidate stays in `candidates` and appears in `reaped`. The exhaustive-or clause
is therefore a predicate-snapshot statement, never an apply-time one: T10's
`candidates == [id]` plus the same id in `skipped [file_delete_failed]` is the
intended shape.

The stable tokens (`ReapSkipReason::as_str`): `active`, `non_empty`,
`highest_without_active`, `unopenable`, `live_tshm_holder`,
`tshm_probe_unknown`, `row_gone`, `file_delete_failed`.

## The `-tshm` seam and the injectable decision

`tshm_verdict_with(ProbeOutcome) -> Option<ReapSkipReason>` is total:
`Free | Missing => None`, `Held => Some(LiveTshmHolder)`,
`Unknown { errno } => Some(TshmProbeUnknown { errno })`. `tshm_verdict(db_path)`
calls `probe_database_file(&sidecar_path(db_path, "-tshm"))` and passes the
outcome to it. Both call sites (the predicate and the post-row-delete re-probe)
use `tshm_verdict`, so the mapping has one registration; a single-site mutation
is not an equivalent mutant.

A real `-tshm` fixture cannot produce `Unknown`: turso 0.8.3 opens the sidecar
**O_RDWR even on a read-only database open**
(`turso_core-0.8.3/database.rs:2952-2967` →
`storage/shared_wal_coordination.rs:1000-1005` → `io/unix.rs:50-57`), so a
`0444` sidecar fails the **count open** with EACCES (`unopenable`) before the
probe runs. The `Unknown` arm is pinned by the `#[cfg(test)]` unit test
`test_tshm_verdict_maps_unknown_to_a_skip_with_the_errno`, which calls the pure
seam with an injected `Unknown { errno }`; it asserts `Unknown` and
`Free`/`Missing` only — **not** `Held` — so M7 stays T7-only and M7b is the
unit-test cell.

## What it does not delete

Active shards · shards with rows · live-held shards · the highest row of a
zero-active tuple · unopenable or corrupt shards · `.bak` files (ADR-0019
D-19-6) · files with no registry row · `metadata.db` · the vault ·
`user-prompts.db` · `episodic.db` · `facts.db` · any non-shard file.

## Rejected alternatives

- **Automatic reaping on open** — the write path becomes a destroyer; no dry
  run; every open pays a full scan.
- **A filesystem-scan candidate set** — no registry truth, races reconcile,
  breaks ruling 8 and the second-run no-op.
- **A plan/apply pair** — TOCTOU between the acts (ruling 11).
- **`.bak` in scope** — ADR-0019 D-19-6; a `.bak` is the only copy.
- **A sidecar-only sweep** — issue 46's split hazard; `-tshm` never dies alone.
- **Delete by the registry's `vector_count` or `read_stored_count`** — ruling
  4; an absent row collapses to zero.
- **Reap the highest in a zero-active tuple** — D-22-5 says it is the
  reactivation target.
- **`delete_shard` reuse** — inverse order and no 46 claim.
- **A per-shard/per-scope API** — no atomic root-wide snapshot.
- **Trash/undo** — no undo is the wave's premise.
- **An interactive confirmation** — ruling 14.
- **A single `-tshm` probe** — a late flag-on holder would be invisible.
- **A side-effect-free CLI dry run** — the ready gate reconciles.
- **A 0/1/2 exit-code "ruling"** — none is recorded; the map is this plan's
  decision.

## Residuals

- **R-23-1 — orphan sidecars after a partial file deletion.** If the `.db` is
  gone and a sidecar unlink fails (or the process dies mid-set), `-wal`/`-tshm`
  remain. While the `.db` is absent no opener derives them, but they are **not
  permanently inert**: a later `.db` minted at the same path (a new shard at
  the same index, a registry re-adoption) can sit beside a stale `-wal`; issue
  53/W0 measured a stale `-tshm` inert, **not** a stale `-wal`. *Inverted
  world:* a recreated shard at the same path reading a stale `-wal` (or a scan
  naming an orphan sidecar as live data) — then the remedy is to remove the
  orphan sidecars before re-creating the shard.
- **R-23-2 — the late-holder window and the read-only foreign opener.** A
  flag-on holder that arrives after the second `-tshm` probe and before the
  first unlink is not seen (46's no-identity-recheck gap); a foreign
  **read-only** opener is invisible to all three layers because it takes no
  lock. *Backstop:* the dry run and the operator stopping the engine.
  *Inverted world:* a report of a shard deleted under a late-arriving or
  read-only foreign holder.
- **R-23-3 — the re-adoption limit.** A surviving file after a failed `.db`
  unlink is re-adopted by the reconciler only when D-22-8 can activate it;
  when it is the tuple's highest and an active row remains below, reconcile
  skips it and the file stays registered nowhere (inert until an operator
  acts). *Inverted world:* an inventory report naming an unregistered
  `*_shard_*.db` with no row.
- **R-23-4 — unopenable shards accumulate.** A corrupt row is never reaped; it
  needs a manual registry/file decision, and every scope-wide read still opens
  it (failing closed). *Inverted world:* a scan that reports the same
  `unopenable` id on every run.
- **R-23-5 — registry-only means orphan files are never reaped by the
  primitive; the CLI may adopt them.** A file whose row was lost waits for
  reconcile (which the CLI's ready gate runs) to re-adopt it; until then the
  primitive is blind to it, and after adoption it is an ordinary candidate.
  *Inverted world:* an orphan file that outlives N CLI dry runs while the
  registry has no row, or a dry run that changes the candidate list by
  adopting one.
- **R-23-6 — `bytes_freed` is apparent bytes, not disk blocks.** It counts
  removed empty regular files; it is not a reclamation promise and not the
  user's data (already deleted).
- **R-23-7 — non-Unix is a no-op.** No probe ⇒ `Unknown` ⇒ every candidate
  skips (fail-closed); on non-Linux Unix the same-process OFD fixture is not
  expressible, so T7 is Linux-only.
- **R-23-8 — the operation lock is advisory.** A non-cooperating process can
  still write; the engine lock and the two probes are the real guards, and
  the lock's job is to serialize our own operations.

## Security and edge cases

- **Never delete an active row.** The `is_active` guard is first; ADR-0022's
  partial unique index is the backstop, and the reaped row is never the
  reactivation target of a zero-active tuple.
- **Row first, and the `.db` before its sidecars.** A failed `.db` unlink must
  leave the `-wal` alone (committed frames live there):
  `remove_sqlite_database_reporting` attempts the `.db` first and stops the set
  on that failure; only after the `.db` is gone may the sidecars be attempted
  independently. `NotFound` on any target is success; `bytes_freed` counts
  only pre-existing **regular files** (`is_file()`), so a directory at a
  sidecar target is a failure, never bytes.
- **`remove_sqlite_database` is untouched.** The reporting function is
  additive; the shipped all-or-nothing body and its callers keep exactly their
  behaviour.
- **`-tshm` is queried twice.** One helper, two call sites. Probe only after
  the read-only count handle is dropped; never take the lock.
- **The count is the file's `COUNT(*)`.** Do not read `shard_stats` or the
  registry's `vector_count` as a fallback.
- **Fail closed on every unproven state:** missing file, unopenable file,
  count query error, probe `Unknown`, `RowGone`, an engine lock held by
  someone else. A skip is always safer than a delete.
- **The CLI dry run is not side-effect-free.** The ready gate's legacy step
  reconciles and can adopt an orphan file into the registry before the report;
  the store dry run mutates nothing. Never promise the operator a pure read.
- **The registry is same-uid-trusted.** A hand-edited `metadata.db` can point a
  row's `db_path` at another file inside the root; `FileClaim`'s containment is
  enforced, and a same-uid actor who can edit the registry can already delete
  the file. A symlinked storage root is the same pre-existing lexical
  containment assumption `archive_shard` already makes.
- **The wave destroys data and has no undo.** The report's `skipped` reason is
  the operator's only record of what survived; the CLI prints one stderr line
  per file failure and the JSON report always names the skips, active rows
  included.

## Implementation notes (measured this wave)

- **The CLI closes the ready gate's cached connections before the predicate.**
  The ready gate's legacy step opens every shard read-write through the
  connection manager and caches the handles; a cached writable handle makes
  the `-tshm` byte-0 query answer `Held` **for the reaper's own process**, so
  no candidate was ever reported until `connections.close_all()` ran between
  `ensure_ready()` and `reap_retired_shards`. Closing mutates no file; the
  primitive reopens each candidate read-only for its count.
- **CLI-14's fixture records the post-dry-child candidate's complete
  `.turso-migrate.json`.** The legacy marker branch runs only when
  `all_shards_complete` finds a complete sidecar for every shard file; a
  candidate created after the dry-run child has none, so the apply child
  re-runs `migrate_storage` and writes that sidecar inside the `0o555` scope
  dir (EACCES, exit 74) before the reap. The fixture writes the exact body
  `migrate_memory_shard` writes for an empty metadata-valid shard, which is the
  precondition the blueprint states. This does not weaken the test: exit 1 with
  `reap.file_delete_failed` and no `reap.failed` remains the discriminator.

## The battery and the acceptance claim

Ten store tests (`crates/mem-store/tests/retired_shard_reaping.rs`), one
`tshm_verdict_with` unit test, and six CLI contract tests
(`tests/reap_cli.rs`). The Step-9 mutation battery is the oracle: ten mutants
(M1–M10) plus the two branch mutants M3b (absent `shard_stats` is not a zero)
and M7b (`Unknown` is a skip). Each of the twelve cells reddens exactly its
named test (the ten integration tests, or the unit test for M7b) and **zero**
CLI tests. `RowGone` (`changes == 0`) is the only `testable: no` branch:
unreachable under the engine lock + operation lock, implemented fail-closed.
The partial `bytes_freed` path (`.db` gone, a sidecar unlink fails) is
`testable: no` without a hook that also reddens M1; keeping the ten-mutant
exclusivity is worth more than pinning a post-condition arithmetic.

## Exit-code map (this plan's decision)

| code | meaning |
|---|---|
| 0 | success, including nothing to do |
| 1 | a completed report with at least one `FileDeleteFailed` |
| 64 | usage (`--apply=<value>`, `--apply <token>`, a foreign scope/confirmation flag) |
| 74 | store/operation failure, including a lost engine lock (`reap.refused`) |
| 75 | busy: the operation lock or the ready gate's legacy step is held (`reap.busy`) |

The map is aligned to the house — `reembed` and `purge` use 64/74/75 — and is
**not** an owner ruling: the recorded W7 block has no exit-code item. Code 1
is the distinct class for a completed report whose file removal failed; it is
not an `Err`.
