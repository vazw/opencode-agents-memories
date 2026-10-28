# 46 — Storage · The sidecar-deletion contract: exclusivity is asserted, refused, or the file is not unlinked

Status: done — 2026-10-28 — wave r5 landed on `issue-46/sidecar-deletion-contract` (commits `42d2bfd` … `10a5dd9`); G1/G2 and R1 measured under `## Wave r5 measurements`.

## PREMISE — `needs_validation` (read before implementing)

Source: `.tmp/scratch/opsec-design-concerns-run-1/REPORT.md` §3.2, verdict `OVERSTATED` (the brief's
call-site list was materially wrong, and the tree is **worse**, not better);
record `mem-store/sqlite-files/remove-tshm-under-live-holder`.

**The rule, from an earlier measurement (issue 43, routed default).** `-tshm` is the sensor, not debris:
a live multiprocess holder holds a lock on byte 0, so unlinking it under a live holder leaves the file on
the old inode while the next opener initialises a **new** one — two disjoint coordination regions over one
WAL. `43-database-base-hardening.md:95-99`.

**The contradiction in our tree.** `crates/mem-store/src/sqlite_files.rs:27`
`SIDECAR_SUFFIXES = ["-wal", "-shm", "-tshm"]`; `:63-75` `remove_sqlite_database` unlinks the database
**and all three** sidecars; `:48-58` `copy_sqlite_database` *copies* `-tshm` to the destination. Nothing at
the function states a precondition, and the module opts out of dead-code checking entirely
(`:15` `#![allow(dead_code)]`).

**The per-caller truth (this table is the corrected one — do not reuse the brief's list).**

| # | call site | target | lock / precondition | verdict |
|---|---|---|---|---|
| 1 | `shard_path_migration.rs:137` in `remove_file_helper` (`:130`), reached from `recover_interrupted_swap` (`:273`) at `:155,:156,:164,:191`, run as the `path-migrate-recovery` ready step (`ready.rs:170`) | **live** shard paths | **none** — only in-process `close_connection` | **UNGUARDED** |
| 2 | the **same** call expression (`:137`), reached from `swap()` (`:822`) at `:841`, which only `migrate()` calls (`:764`) after the operation lock at `:725-727` | **staged** (`.path-migrate-<nonce>.tmp`, `:655-658`) — a second entry path, not a distinct site | advisory `TursoOperationLock` (`:726`, `"path-migrate"`) | advisory only |
| 3 | `legacy_migrator.rs:255` in `restore_from_backup` (`:243`), via `migrate_memory_shard` (`:372`) ← `migrate_directory` (`:813`), inside `migrate_storage` (`:869`) called at `:863` — after the lock at `:856` | live path | `LegacyMigrationLock` | advisory only |
| 4 | `legacy_migrator.rs:681` (`recover_one_swap`, `:647`) | staged, nonce-named | none needed — no third-party holder can exist | safe by construction |
| 5 | `legacy_migrator.rs:694`, `:707` (`recover_one_swap`) | **live** `expected` path | **none** — `recover_interrupted_reembed_swaps` (`:620`) runs at `:843`, *before* the lock at `:856` | **UNGUARDED** |
| 6 | `schema_migrations.rs:88` `restore_from_backup` (`:87`), called at `:171` and `:490` | **live** path, **while the caller holds an open `Db` on it** | **none** | **UNGUARDED + self-holder** |
| 7 | `encryption_migrator.rs:348`, `:355`, `:386` | staged, then live, then live (verify-failure restore) | advisory op lock (`:404`, `"tursodb-encrypt"`) | advisory only |
| 8 | `migration_service.rs:443` | staged, nonce-named | op lock (`:266`) + closed handle (`:438`) | safe by construction |

**Summary: three unguarded, three advisory-lock-only, two safe by construction.** The eight rows cover
**ten** production call expressions in **five** files — row 7 merges three `encryption_migrator`
expressions, and rows 1 and 2 are the same expression reached by two different entry paths. The two lock
mechanisms are advisory in the precise sense that only cooperating operations check them
(`operation_lock.rs:188`, `:249`), and a normally running engine holds neither — so **no caller has
provable exclusivity**, which is why three rows say "advisory only" rather than "guarded".

**The sharpest site is #6, and it is provable in-process.** `apply_schema_migrations` takes
`db: &dyn Db` (`schema_migrations.rs:95-99`), so restoring cannot close the handle it holds — the
function's own comment records the divergence: *"upstream closes the handle before the backup copy; a
borrowed `&dyn Db` cannot"* (`:115-120`). After `restore_from_backup` unlinks and renames, the live handle
still points at the **deleted inode**; further writes through it are lost.

**What is measured, and what is not.** Every call site above is source-inspected; no runtime demonstration
of a coordination split exists in this wave, and **no test deletes `-tshm` under a live holder** — the
only remover test (`crates/mem-store/tests/sqlite_handle_release.rs:278`) deletes files with no holder,
and `turso_capabilities.rs` walks sidecar bytes for canaries but never deletes under a holder.

**The upstream half of the mechanism — verified at turso HEAD `aa93d669` (sources at
`.tmp/scratch/turso-head-aa93d669/`) and in the shipped `turso_core-0.8.3`.** The engine cannot notice the
unlink, and four facts say so:

1. **The mapping is cached for the `Database` lifetime.** `shared_wal_coordination:
   OnceLock<Arc<MappedSharedWalCoordination>>` (`core_database.rs:693`, initialised `:838`), set once and
   returned early on every later call (`:3058-3060`; `0.8.3 database.rs:631`, `:777`, `:2947-2949`, `:2974`).
2. **Nothing re-checks the file's identity.** `dev()` / `ino()` / `metadata()` / `file_id` return **zero**
   hits in `core_storage_shared_wal_coordination.rs`, in both versions. (The in-process *registry* uses
   `io.file_id` in `core_database.rs`, but that is a different mechanism and does not guard the path.)
3. **The engine never unlinks `-tshm` itself.** In `core_storage_shared_wal_coordination.rs` there is no
   filesystem removal at all (the only `.remove(...)` calls are in-memory maps, `:278`, `:741`), and the one
   `io.remove_file` in `core_database.rs` targets the **WAL** (`0.8.3 :2245`; HEAD `:2321`), not the
   coordination file. So every deletion of `-tshm` is **ours**. Deleter and victim are different processes,
   and the victim cannot notice.
4. **An unlink can silently downgrade a reader, which is a consequence this issue did not have before.** A
   read-only multiprocess opener that finds no `.tshm` does not fail: it degrades to the legacy read-only
   WAL path (`0.8.3 database.rs:2952-2965`; HEAD `:3051-3072`), in the code's words — *"Read-only opens
   cannot create `.tshm`. If no shared coordination file exists, degrade to the legacy read-only WAL path
   rather than failing the open. This keeps binding-level option plumbing advisory for readers while
   writable opens still enforce the stricter multiprocess contract."* So unlinking does not only split two
   multiprocess regions; it can leave a **reader** coordinating through the legacy path while a writer keeps
   the new one — a mixed-mode split produced by the unlink itself.

`.tshm` is also not disposable by shape: `TSHMWAL\0` magic, a format version, a separate backfill-proof
version, and runtime resizes via `shared_wal_set_len` (`0.8.3 shared_wal_coordination.rs:39-43`, `:944`,
`:2649`). Cross-ref **45.A5**: our own mode marker must not live in it either.

⇒ **46.6's test must assert the split is silent.** The holder keeps working through its old mapping with no
error on either side, and the next opener initialises a fresh `.tshm`. A test that expects an error would be
testing a mechanism that does not exist; a test that expects turso to repair the file would never pass.

**The gate — both must pass before this issue is done.**

- **G1 (pre-fix observation, recorded in this issue).** A test that holds a real `Db` handle open on a
  database, runs the schema-restore path, then writes through the **old** handle and reopens the path: the
  write is not visible and the file on disk is the backup's. If the write *is* visible, this half of the
  premise is refuted and site #6 downgrades to documentation.
- **G2 (positive control + inversion arm).** A test that unlinks `-tshm` under a live flag-on holder child
  (spawn harness: `turso_capabilities.rs:2229`) and shows the split — next opener initialises a fresh
  `-tshm`, pre-existing coordination state is gone. The inversion arm: the same test with the fix's
  exclusivity guard in place must refuse **before** the unlink, and must go red if the guard is disabled.

**The inverted world — what would refute this issue.** Every caller provably holds exclusivity (then this
degrades from hazard to missing-precondition documentation, and only the doc + the token land), **or** a
demonstration that unlinking a `-tshm` under a live holder is harmless. Neither may be assumed: the
advisory locks do **not** exclude a live reader, which is why they are labelled advisory and not guarded.

## What to build

1. **Turn the precondition into a type, not a comment.** `remove_sqlite_database` (and the sidecar legs of
   `copy_sqlite_database`) should require a proof of exclusivity it cannot be called without — the repo's
   own precedent is decision 2's typed refusal and ADR-0013's panic-free posture. Two constructors:
   - **structural** — from `EngineLock` (`engine_lock.rs:149`), which already proves we own the root;
   - **runtime** — from the two-directional probe in **issue 45**, which proves no foreign holder exists.
   A caller that can obtain neither is a caller that must not unlink. This is the deep-module move: the
   contract becomes unrepresentable-if-violated instead of remembered.
2. **Fix the three unguarded sites.** Recovery must take the operation lock *before* its first unlink (or
   refuse), and the schema-restore self-holder must stop deleting a file it has open — which requires a
   structural change, because `apply_schema_migrations` cannot close a `&dyn Db`. Hoist the restore above
   the open, or reopen after restore; do not paper over it with another checkpoint.
3. **Answer the copy/rename halves of the same contract.** `copy_sqlite_database` (`:48-58`) copies a
   `-tshm` from a possibly-live source; `rename_sqlite_database` (`:33-44`) deletes the destination's
   sidecars first. Both are the same hazard with a different verb, and both are currently unspecified.
4. **Name what `-tshm` reaping is and is not.** Issue 43 records two things that must be read together:
   "retain permanently — it is the sensor" (`:95-99`) and "permanent debris (~86 KiB per database), W7 must
   reap them or ADR-0019 must accept them by name" (`:365-367`). This issue does **not** decide reaping
   (that is W7 / ADR-0019's); it decides the *contract under which any* deletion of `-tshm` is legal.

## Acceptance criteria

- [ ] **46.1** The contract is written at the function: what the caller must hold, what happens if it does
      not, and the four lock classes it can be satisfied by (engine, operation, legacy-migration, probe).
      The doc comment no longer says "upstream rules kept byte-for-byte" without saying what that cost us.
- [ ] **46.2** `remove_sqlite_database` cannot be called without the exclusivity proof — a type, not a
      comment. All **ten** production call expressions across the five caller files are updated
      (`shard_path_migration.rs:137`; `legacy_migrator.rs:255,681,694,707`; `migration_service.rs:443`;
      `encryption_migrator.rs:348,355,386`; `schema_migrations.rs:88`), the five helper call sites at
      `shard_path_migration.rs:155,156,164,191,841` each satisfy it, the test caller at
      `crates/mem-store/tests/sqlite_handle_release.rs:278` still compiles, and a compile error is the test
      for a new caller.
- [ ] **46.3** `copy_sqlite_database` and `rename_sqlite_database` carry the same precondition or an
      equally explicit statement of why they differ (a copy of a live source is a snapshot, a rename is
      not).
- [ ] **46.4** `schema_migrations.rs:88` no longer unlinks under a live handle: the restore happens with no
      handle on that path, or the caller reopens afterwards. `:115-120`'s divergence comment is removed or
      rewritten to describe the new structure — it currently documents the bug.
- [ ] **46.5** `shard_path_migration.rs:137`'s recovery path and `legacy_migrator.rs:694,707` take the
      operation lock before their first unlink, or refuse with a typed outcome. `legacy_migrator.rs:843`'s
      early recovery **stays where it is**: `:845-848` requires recovery before `run_engine_migration`, and
      the engine migration before the legacy lock — so the fix is an advisory lock acquired **before**
      `:843` (the operation lock, a distinct operation name such as `"legacy-reembed-recover"`), scoped to
      the recovery call and released after it, not a move under `:856`. Moving it under `:856` inverts the
      order that comment documents, and holding it across `:848` buys nothing: `run_engine_migration`
      documents that it never takes the operation lock (`engine_migrator.rs:17-18`).
- [ ] **46.6** A regression test unlinks `-tshm` under a live holder and pins the split (G2), plus its
      inversion arm; and a second test proves the guard refuses before touching the file (no partial
      unlink: `-wal`/`-shm` still present after a refusal).
- [ ] **46.7** **G1 and G2 recorded in the issue body** before implementation: the pre-fix self-holder
      write loss, and the split. Neither may be described as "would" — the numbers/observations go in.
- [ ] **46.8** The probe reuses issue 45's seam. No second probe implementation; if 45 has not landed, this
      issue's detection leg waits and says so, and the contract + test + unguarded-site fixes still land.
- [ ] **46.9** No path outside a documented migration window unlinks `-tshm`; presence of `-tshm` is never
      used as a liveness signal (that claim was rejected — `43:157`).
- [ ] **46.10** The contract's doc text records **why** the unlink is unsafe, citing the upstream half above
      (cached mapping, no identity re-check, engine never unlinks, read-only degrade), so no caller may
      assume turso will notice or repair. `-tshm` is named as turso-owned and format-versioned there, which is
      also why the *mode marker* (45.A5) is ours and lives elsewhere.

## Blocked by / Blocks

- **Blocked by:** nothing for `46.1`–`46.7`. `46.8`'s detection leg is blocked by issue 45 (the probe seam).
- **Blocks:** nothing directly, but **every W6/W7 file-destroying wave inherits this contract** — issue
  43's W7 (retired-shard reaping) and issue 39's sweep both delete database files. Quote this issue when
  they land.

## Comments

- Filed 2026-10-25 from the opsec design-concerns audit (concern C5, verdict `OVERSTATED` — core fact true,
  evidence list materially wrong), per the audit's §6 recommendation 3.
- **Corrected before filing, per the brief's AUDIT RESULT block**
  (`20261025-design-concerns-audit-brief.md:38`): four of the brief's cited lines are `use` imports
  (`shard_path_migration.rs:26`, `legacy_migrator.rs:39`, `migration_service.rs:31`,
  `schema_migrations.rs:26`), three are renames (`legacy_migrator.rs:263,498`, `migration_service.rs:556`),
  `schema_migrations.rs:81` is a **copy** (the remove is `:88`), and `encryption_migrator` was omitted
  entirely. The brief's headline ("four reachable code paths") is replaced by the table above — **three
  unguarded, three advisory-only, two safe by construction** — which is the table's own arithmetic (§3.2);
  the audit's §7 prose says "four advisory-lock-only" and is one high, the same slip this correction list
  fixes in 46's summary line.
- **Upstream is equally broken and gets no defence from the parity:**
  `example/opencode-mem/src/services/turso/sqlite-handle-release.ts:56` unlinks db + `-wal` + `-shm` +
  `-tshm` identically. Owner decision 3 (`43:45`) says upstream is a reference, never a co-tenant — so
  "faithful port" is a finding, not a justification.
- **Not re-litigated here:** issue 43 decisions 1–9; the rejected `-tshm` claims (`43:154-157`);
  `-tshm` reaping policy (W7 / ADR-0019 own it).
- The `46.4` self-holder is the architect's emphasis, not the audit's: the audit ranked the two recovery
  paths first ("two recovery paths are unguarded", `REPORT.md:60`), but site #6 is the only one whose
  hazard is provable inside one process, with no second process needed — which makes it the cheapest
  gate and the one least likely to be argued away.

- **2026-10-27 — triaged: hazard confirmed, reuse point absent.** Re-derived at HEAD: ten unlink sites across
  five files, `remove_sqlite_database` still takes a bare path with no proof token, `SIDECAR_SUFFIXES` covers
  `-wal`/`-shm`/`-tshm`, and `schema_migrations.rs` unlinks a database out from under its own borrowed
  `&dyn Db`. Two gaps before a wave: the stated detection leg (`-tshm` offset-0 probe, two-directional) was
  deliberately **not** built — issue 45 records it superseded — and the G1 self-holder write-loss / G2 `-tshm`
  split measurements are unrun.

## Agent Brief

> *This was generated by AI during triage.*

**Category:** bug

**Summary:** make the sidecar-deletion contract enforceable — but one ruling comes first: what counts as
exclusivity now that the `.tshm` probe direction was refused.

**Current behaviour (verified at `f41b9ef`):**
- `crates/mem-store/src/sqlite_files.rs`: `SIDECAR_SUFFIXES = ["-wal", "-shm", "-tshm"]` (`:27`);
  `remove_sqlite_database` takes a bare `&Path` and unlinks the database plus all three sidecars (`:63`–`:76`),
  with no precondition; the module still opts out of dead-code checking (`:15`).
- **Ten** production call expressions across five files: `schema_migrations.rs:88`;
  `shard_path_migration.rs:137`; `legacy_migrator.rs:255,681,694,707`; `migration_service.rs:443`;
  `encryption_migrator.rs:348,355,386`. (The last three moved from 335/342/373; the body is corrected.)
- The self-holder is real: `apply_schema_migrations` takes `db: &dyn Db` (`schema_migrations.rs:95`) and its
  own comment documents the divergence — *"upstream closes the handle before the backup copy; a borrowed
  `&dyn Db` cannot"* (`:115`) — then restores over the live path at `:88`.
- No exclusivity proof exists anywhere in the store; `remove_sqlite_database` is not wired to either
  candidate.

**The blocking premise — the two-directional probe does not exist.** Issue 45's owner decision O3 says
*"Probe A is not implemented. A legacy open is refused by construction"* (`45:197`), and its own close-out
says *"45.3 — superseded by O3; not implemented. The `.tshm` offset-0 probe ships in neither direction"*
(`45:413`). What shipped is **Probe B**, whose documentation names one direction only: *"The exclusive byte
lock was taken and released: **no legacy holder**"* (`crates/mem-store/src/open_guard.rs:756`). A live
multiprocess holder — which is what our own engine is — is not detected by it. AC 46.2's "runtime proof from
issue 45's two-directional probe" is therefore unsatisfiable as written.

**The ruling — pick one:**
1. **Structural + legacy (recommended).** Require `EngineLock` (`crates/mem-store/src/engine_lock.rs:70`,
   `acquire_engine_lock`) **and** `ProbeOutcome::Free` (`open_guard.rs:756`) before any unlink; accept a
   foreign multiprocess holder as an out-of-scope residual, because the storage root is ours by the same
   assumption the store already makes. This is the strongest proof that exists, and it degrades honestly
   rather than claiming a probe the tree does not have. It requires hoisting the engine lock above the
   recovery paths, which currently take only the operation lock (`migration_service.rs:266`).
2. **Re-open 45's O3.** Add the `.tshm` offset-0 probe back. That is a separate wave against the open guard;
   46 waits for it.
3. **Refuse-unlink.** No proof ⇒ no unlink: return a typed refusal and leave the database and its sidecars in
   place. The conservative option, and the only one that needs no new probe and no lock hoist — but it
   changes what the migration/recovery paths can do on a retry.

**Why this is `ready-for-human`:** option 1 accepts a residual the issue currently promises to close
("nothing proves no other process holds them"). That is a contract decision, not an implementation detail.
Once ruled, the wave is fully specified.

**Measurements that must precede implementation (the issue's own gate):**
- **G1** — self-holder write loss: hold a real `Db`, run the schema-restore path, write through the old
  handle, reopen. Expected: the write is not visible and the file is the backup's. In-process, cheap, and
  provable today. **Unrun.**
- **G2** — `.tshm` split under a live multiprocess holder, plus its inversion arm. The child-process harness
  exists (`crates/mem-store/tests/turso_capabilities.rs:2361`). **Unrun.**

**Key interfaces:** `remove_sqlite_database`, `copy_sqlite_database`, `rename_sqlite_database`,
`EngineLock`/`acquire_engine_lock`, `ProbeOutcome`/`probe_database_file`, `apply_schema_migrations`,
`restore_from_backup`.

**Acceptance criteria:** the issue's 46.1–46.10, with two amendments after the ruling:
- **46.2** — the proof is the ruled one (option 1: `EngineLock` + `ProbeOutcome::Free`), not "issue 45's
  two-directional probe"; the ten sites are enumerated at their current lines.
- **46.8** — the probe **is** issue 45's Probe B (legacy-only); the `.tshm` direction is either accepted as a
  residual (option 1) or deferred to its own wave (option 2). Do not claim it is reused when it was never
  built.

**Out of scope:** `-tshm` reaping policy (W7 / ADR-0019 own it); the open guard's probe changes unless
option 2 is ruled; issue 43 decisions 1–9.

- **2026-10-28 — owner ruling: option 1.** Exclusivity is `EngineLock` **and** `ProbeOutcome::Free` before any
  unlink. A foreign multiprocess holder is an **accepted residual**, stated in the contract and in ADR-0020's
  or the contract's doc text — not a claim the tree can prove. The engine lock must be hoisted above the
  recovery paths (`migration_service.rs` currently takes only the operation lock). G1 (the self-holder write
  loss) runs first in the wave; G2 and its inversion arm follow. The `.tshm` probe stays unimplemented; issue
  45's O3 is not re-opened.

## Wave r5 measurements (2026-10-28)

**G1 — self-holder write loss (pre-fix, MEASURED).** A real v1 shard kept open through `DbHandle`; a
failing v2 migration with `db_path: Some(..)`; then a write through the old handle; reopen. Literal
observation (`cargo test -p mem-store --test sidecar_deletion_contract -- --exact
test_g1_self_holder_write_loss_is_silent --nocapture`):

```text
[G1 OBSERVED] write_error=none user_version_after_reopen=1 row_ids_after_reopen=["row1"] restored_inode=2082964
```

The write is acknowledged and silently lost; the path is the backup's inode. The premise holds. After the
fix the replacement test reports the inversion: `[G1-INVERSION] row_ids_after_reopen=["row1", "row2"]
inode_unchanged=true`.

**G2 — `.tshm` split under a live multiprocess holder (MEASURED).** Child process (this test binary
re-executed) holds a multiprocess handle; the parent raw-unlinks the database and all three sidecars, the
child writes again, the next opener creates a new `-tshm`. Literal observation:

```text
[G2] probe under a live multiprocess child = Free
[G2] TSHM_INO before=34:2085028 (len 86016) AFTER=ok TSHM_INO after=34:2085032 (len 86016)
```

**R1 resolved:** the probe cannot see a foreign multiprocess holder (`Free`), so exclusivity is the engine
lock plus the absence of a legacy holder — the residual stands as written.
