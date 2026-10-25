# 45 — Security · Pre-open mixed-version guard: the engine's oracle cannot fire on any open path we use

Status: needs-triage — 2026-10-25. **The mixed-mode data-loss window is unguarded on the only
production open path, so owner decision 7 (issue 43) is unimplemented by construction.** The premise is
`needs_validation`, not confirmed — the gate in `## PREMISE` must pass before this issue is called done.

## PREMISE — `needs_validation` (read before implementing)

Source: `.tmp/scratch/opsec-design-concerns-run-1/REPORT.md` §3.1, verdict `SOUND` with two wording
corrections; record `mem-store/turso-open/mixed-version-guard-unreachable`.

**The claim.** turso already implements the oracle that refuses a mixed legacy/multiprocess open, and it
cannot run on our path.

| step | evidence |
|---|---|
| every production open goes through one seam | `crates/mem-store/src/db.rs:320` (`impl ConnectFactory for TursoConnectFactory`), `:327` `turso::Builder::new_local` |
| the builder delegates to `rsapi`, which supplies `OpenOptions::storage` unconditionally | `turso-0.8.3/src/lib.rs:329-355`; `turso_sdk_kit-0.8.3/src/rsapi.rs:1033-1034` |
| the oracle only runs when `storage` is **unset** | `turso_core-0.8.3/database.rs:1222` — `if options.storage.is_none() { resolve_default_storage(..) }` |
| the oracle itself | `turso_core-0.8.3/database.rs:1153`, `:1163`, `:1173` |
| no direct `turso_core` dependency, no call to any of the five entry points | `crates/mem-store/Cargo.toml`; `grep -rn "turso::core" crates src` → no call |
| the multi-process flag is **on** by default on every non-Windows platform | `crates/mem-store/src/db.rs:258-262` (`ConnectOptions::for_config`) |

**What is measured, and what is not.** The call chain is source-inspected end to end; the row-loss
sink is a pinned measurement from issue 43's probe wave (`b42905c`): cross-process mixed mode silently
lost committed rows with `integrity_check` returning ok, caused by the *other side's commit*, not by its
open. **Not** measured: the same loss reproduced through `TursoConnectFactory` in this wave, and the
absence of any other oracle entry point at runtime. Both are gated below.

**The gate — all four must pass before this issue is done.**

- **G1 (pre-fix observation, recorded in this issue before any fix lands).** With a live flag-on holder
  child (spawn harness precedent: `crates/mem-store/tests/turso_capabilities.rs:2229`
  `kill_writer_child_with_an_open_transaction`, and the flag-off option builder at `:2012`
  `single_process_options`), an open through `TursoConnectFactory` **succeeds**, and the pinned row loss
  reproduces. If that open already refuses, the premise is refuted — see the inverted world, stop, do
  not write the guard.
- **G2 (positive control, both directions).** After the fix, the same harness observes a *typed refusal*
  for a live flag-on holder (the `.tshm` offset-0 lock is held) **and** for a live flag-off holder (the
  database file is locked). Inversion arm: disable one probe and exactly that direction's control must go
  red — one control cannot cover the other, because the sidecar sets are disjoint.
- **G3 (negative control).** No holder ⇒ open succeeds; and a **stale** `-tshm` with no holder ⇒ open
  succeeds. Presence of `-tshm` is not liveness — that claim was rejected by an earlier opsec run
  (`43-database-base-hardening.md:157`) and must not creep back in as a heuristic.
- **G4 (residual honesty).** The post-open re-probe narrows the TOCTOU window; it does not close it. The
  issue must state what remains uncovered (a holder that arrives between the post-probe and the first
  write), in one sentence, rather than claiming the window is shut.

**The inverted world — what would refute this issue.** A reachable open path in this tree that leaves
`OpenOptions::storage` unset, **or** any turso 0.8.3 entry point that runs the oracle outside
`resolve_default_storage`. Then the fix is a regression test on an existing refusal, not new code, and
this issue should be closed as measurement. Settled by G1 plus `cargo tree -i turso_core` plus a grep for
the five entry-point names in `crates/` and `src/`.

## What to build

Implement owner decision 7 (`43-database-base-hardening.md:56`) **in our own pre-open code**, because the
engine's oracle is unreachable from our seam.

**Where the seam goes — one seam, not five.** `ConnectFactory::connect` (`db.rs:301`) is the funnel every
database open in the tree passes through: shards, `metadata.db`, `user-prompts.db`, `user-profiles.db`,
`episodic.db`, `facts.db`. A guard placed here is deep — the seam already has two implementations
(`TursoConnectFactory` in production, fakes in tests) and every current and future open inherits it —
instead of five call-site patches that the next open path will silently miss. The probe's platform
knowledge (which syscall, which offset, which file) stays inside one new module; nothing else in the tree
learns about `-tshm` byte 0.

**The probe is two-directional, because the two modes share no sidecar.**

1. **`.tshm` offset-0 byte lock** detects a live *multiprocess* authority. Attempt the same lock turso's
   `detect_open_mode` attempts (`turso_core-0.8.3/storage/shared_wal_coordination.rs:1060`; `F_OFD_SETLK`
   on Linux, `F_SETLK` elsewhere); success ⇒ no such holder, release immediately.
2. **Exclusive lock on the database file itself** detects a live *legacy* (flag-off) holder, which holds
   `{db, -wal}` and never touches `-tshm` (`database.rs:985-1005`).
3. **Post-open re-probe** on the same sequence turso uses: pre-probe → open → post-probe.

**Unknown ⇒ refuse.** A probe that cannot answer is a refusal, not a pass. The refusal is a typed
`StoreError` variant (ADR-0013 posture; naming precedent `StoreError::MigrationLocked`, `error.rs:80`) and
it must **not** claim the holder's identity — an OFD lock carries no `l_pid`. Name the path and the
detected direction; "unknown" is its own outcome, distinct from "held".

**The self-conflict case must be decided, not discovered.** `F_OFD_SETLK` conflicts between two *open
file descriptions*, including two in the same process — and the *holder* is exactly such a description: in
multiprocess mode our own engine holds a shared lock on `-tshm` byte 0 for as long as the database is open.
So a second open of a database this process already holds would be refused by our own guard, with the same
error it uses for a foreign holder. The `ConnectionManager` cache (`db.rs`) masks most of this in production
— which is precisely why it would surface in a test or in a future concurrent reader rather than at review.
This is opsec's blocker 3 in its own words — *"no `l_pid` under OFD → structural clearance, not identity"*.
So: the guard must clear own-process descriptions **structurally** (a process-local registry of open paths,
probed once and released at the last handle), and the issue must state which mechanism it chose — a rule the
next concurrent opener cannot violate.

**Enumerate the open paths first; that is the guard's first job per decision 7.** Today the known
openers are the engine (`crates/mem-bridge/src/engine.rs:1063`, which holds `.engine.lock`, D-39), the
`purge-session` mode of the same binary (`src/main.rs:328`; takes `mem_episodic::gate::try_acquire` plus
`acquire_turso_operation_lock`, **not** the engine lock — `crates/mem-bridge/src/purge.rs:1-20`), the
startup ready steps (`crates/mem-store/src/ready.rs:150-185`), and tests. The guard must cover every row
of that table or refuse the row; a guard that only re-checks the engine path guards nothing new. Decision
8's purge amendment (`43:88`) is **not** this issue and is not re-decided here.

**Platform matrix.** `ConnectOptions::for_config` enables multiprocess WAL only on non-Windows
(`db.rs:258-262`), so a `-tshm` produced by a build of ours cannot exist on Windows — the `.tshm` leg is
vacuous there by construction, and the DB-file leg is already enforced by the engine's own exclusive lock.
State that as the recorded rationale rather than letting "unknown ⇒ refuse" brick Windows. Non-Linux unix
must take the probe path with `F_SETLK` and be tested, not assumed.

**Cost.** Two `fcntl` calls per open, both released before the open proceeds. State the measured number in
the issue; do not guess it.

## AMENDMENT 2026-10-25 — re-verified at turso HEAD `aa93d669` (sources on disk)

This changes **no fix-list item**. It upgrades one citation and adds four constraints to the probe. Sources
the owner fetched: `.tmp/scratch/turso-head-aa93d669/` (`sdk-kit_src_rsapi.rs`, `core_database.rs`,
`core_storage_shared_wal_coordination.rs`). Every claim below was checked there **and** in the shipped
`turso_core-0.8.3` / `turso_sdk_kit-0.8.3` in the cargo registry, because HEAD is not what we run.

**A1 — the premise holds at HEAD too, so a 0.9 upgrade will not close this issue.**

- `rsapi::open` builds `OpenOptions::new(SqliteDialect).storage(db_file).flags(…).db_opts(…).encryption(…).page_codec(…)` and calls `Database::open_async` (`sdk-kit_src_rsapi.rs:1034-1045`). `storage` is always `Some`: either the caller's `db_file` (`:1006-1011`) or `DatabaseFile::new(io.open_file(…))` (`:1007`).
- `resolve_default_storage` — the only caller of either probe (`core_database.rs:1214`, `:1224`, `:1234`) — is called from exactly two places: `Database::open` at `:1284`, **guarded by `if options.storage.is_none()`** (`:1283`), and `do_open` at `:1436`.
- `Database::open_async` (`:1314`) requires `storage` (it returns `InvalidArgument("OpenOptions::storage is required for Database::open_async")` when unset, `:1321-1328`) and **never** calls `resolve_default_storage`. Our path supplies storage, so it lands here with the oracle unreachable.
- Shipped-version parity, so this is not a HEAD-only regression: `turso_core-0.8.3/database.rs` has the same shape — `open` `:1216`, resolve only under `storage.is_none()` `:1222`, `open_async` `:1253`, `do_open` `:1369`.
⇒ **A 0.9 upgrade does not fix issue 45.** The guard stays ours, and `45.8`'s pre-fix observation is a
`0.8.3` observation that a future bump will not invalidate.

**A2 — in multiprocess mode turso deliberately takes no OS lock on the database file.** `rsapi.rs:1003-1005`
(`0.8.3`: `:1002-1003`), with the code's own reason at `:984-987`: *"taking the OS-level fcntl lock here
would block every other multiprocess process from opening the same file."* Two consequences for the probe:

- the flag-on holder holds **no** `fcntl` lock on `{db}`, so "detect a legacy holder" cannot reuse turso's
  own lock — the probe must take its **own** exclusive lock and release it. That is exactly what turso's
  own multiprocess-direction probe does: `probe_flags = (flags | Create) & !NoLock & !ReadOnly;
  io.open_file(path, probe_flags, true)` (`0.8.3 database.rs:995-1003`; HEAD `:1052-1060`).
- it restates `43:60` in code: *the flag-on holder holds none* — not a doc claim, a line we can cite.

**A3 — the read-only exemption is asymmetric. The guard must mirror it, not flatten it.** *(code read, both
versions — not a runtime observation)*

| direction | guard | read-only |
|---|---|---|
| legacy opener rejecting a live multiprocess authority | `reject_live_multiprocess_wal_for_legacy_open`: `enable_multiprocess_wal \|\| is_memory_like \|\| !supports \|\| !path_allows` ⇒ `Ok` (`0.8.3 :939-945`; HEAD `:1000-1007`) | **no exemption** — a legacy read-only opener is still refused with `LockingError` (`0.8.3 :960-967`) |
| multiprocess opener rejecting a live legacy DB-file lock | `reject_live_legacy_wal_for_multiprocess_open`: `!enable_multiprocess_wal \|\| flags.contains(ReadOnly)` ⇒ `Ok` (`0.8.3 :991-993`; HEAD `:1050-1052`) | **exempt** — a read-only multiprocess opener never refuses a legacy holder |

The engine's posture is **read-only opens are protected, not protective**: a read-only open can be *refused*
as a victim, but it does not *refuse* as an aggressor. The coordination side says the same in prose
(`0.8.3 database.rs:2959-2963`: *"This keeps binding-level option plumbing advisory for readers while
writable opens still enforce the stricter multiprocess contract."*).

A guard that refused **every** read-only mixed open would diverge from the engine it wraps; a guard that
exempted **every** read-only open would miss the direction the engine still enforces. **Rule for this
issue:** probe A (live multiprocess authority ⇒ refuse) applies to **every** open; probe B (live legacy
DB-file lock ⇒ refuse) applies to **writable** opens only. Unknown ⇒ refuse is a *writable-open* rule; a
read-only open either skips B and records the divergence, or refuses — the blueprint decides, not the code.

**Our tree opens no read-only database today**, so the asymmetry is a contract constraint now and a
behaviour question only when a read-only opener appears (a backup/export path is the obvious future one):
`turso::Builder::read_only` exists (`turso-0.8.3/src/lib.rs:284`) and `TursoConnectFactory::connect` never
calls it (`db.rs:320-338`). Encode the distinction in the probe's signature (`open_flags` or a `Writable`
token), not in a comment.

**A4 — the published documentation contradicts the code, which is why this issue stays open despite looking
settled.** *(doc-sourced — from the owner's ingest; the `.mdx` is not on disk here, so this is a
documentation claim, not a source read.)* The docs say the open path *"probes for an incompatible live
opener and fails fast"* and *"Mixing modes is rejected"* (`multiprocess-access.mdx:139-145`, `:78`), and
*"read-only opens are advisory"* (`:148` — half true, see A3). The code says otherwise in both versions
(A1). Record the contradiction in ADR-0025's Grounds: a reader who trusts the docs will believe the guard is
redundant.

**A5 — `.tshm` is turso-owned, format-versioned and resized at runtime, so decision 7's marker must not live
inside it.** `SHARED_WAL_COORDINATION_MAGIC = b"TSHMWAL\0"`, `SHARED_WAL_COORDINATION_VERSION = 1` ("bump
whenever persisted layout changes") and a separate backfill-proof version
(`0.8.3 storage/shared_wal_coordination.rs:39-43`, identical at HEAD); the header is version-checked on open
(`:438-446`) and the file is resized with `shared_wal_set_len` (`:944`, `:2649`). A marker written into it
would be clobbered by a resize, invalidated by a version bump, or read back as `Corrupt`. The marker
(`{pid, start_time, mode}`) therefore lives in a file **we** own — `.engine.lock`-style, in the storage
root — and ADR-0025 fixes its name and format. Not in the database header, not in the WAL, not in `.tshm`.

**One row changes in `45.1`'s open-path table, and it is the vault's:** `mem-vault/src/store.rs:112-114`
refuses to open when encryption is disabled and `:271` forces `database_encryption_enabled = true`, so the
vault is the one open path that is **always** encrypted and therefore always in the combination issue **53**
is about — it cannot be config-flipped to flag-off. Its row is `covered` either way; do not hard-code the
assumption that it stays flag-on (53 may remove the combination there).

## Acceptance criteria

- [ ] **45.1** The open-path table exists in this issue before any code: every path that opens a database
      (engine, `purge-session`, each ready step, each test harness class), with the lock it holds today and
      the guard's disposition for it (`covered` / `refused` / `safe by construction` with reason).
- [ ] **45.2** The guard lives at `ConnectFactory::connect` (or a seam that provably covers it) — not at
      individual call sites. A new test opens through `TursoConnectFactory` and is guarded without any
      call-site change.
- [ ] **45.3** `.tshm` offset-0 probe: an exclusive byte lock is attempted and released; a live
      multiprocess holder ⇒ refuse. `-tshm` presence alone ⇒ **pass** (G3).
- [ ] **45.4** DB-file exclusive-lock probe: a live flag-off holder ⇒ refuse, proven against a real child
      process, not a mock.
- [ ] **45.5** Post-open re-probe runs; G4's residual sentence is written; a refusal after open drops the
      handle and leaves the database unchanged (`integrity_check` ok, no row loss against the pre-open
      snapshot).
- [ ] **45.6** The self-conflict rule is implemented and tested: a second open of a database **this process**
      already holds does not make the guard refuse (the test opens the same fixture twice without closing
      the first handle).
- [ ] **45.7** Typed refusal variant exists, is `#[non_exhaustive]`-consistent with `StoreError`, keeps
      `StoreError` ≤ 128 bytes (see `43:240-247` — the shipped error is 112), and carries no pid claim.
- [ ] **45.8** **G1 and G2 recorded**: the pre-fix unguarded open and its row loss in the issue body; the
      two positive controls plus their two inversion arms as tests. A control that cannot be inverted is
      not a control.
- [ ] **45.9** `purge-session` is covered or refused by the guard. No behaviour change to the purge itself
      (decision 8 owns that).
- [ ] **45.10** Platform matrix stated and tested: Linux `F_OFD_SETLK`, other unix `F_SETLK`, Windows
      recorded as vacuous-by-construction with the `db.rs:258-262` rationale. Untested platforms are
      named as untested.
- [ ] **45.11** No new dependency: `libc` is already a workspace dependency (`Cargo.toml:47`, used by
      `mem-profile`); it is added to `mem-store` only. Any other crate needs its own justification.
- [ ] **45.12** The probe cost per open is measured and recorded.
- [ ] **45.13** **(A3) Writability is part of the probe's contract.** Probe A runs for every open; probe B
      only for a writable open. A decision-table test covers `writable × read-only` against both directions
      without needing a filesystem race, and a read-only open that skips B records that divergence (citing
      `0.8.3 :991-993`). No read-only opener is added in this issue — the rule exists so the first one
      inherits it.
- [ ] **45.14** **(A5) The mode marker lives in a file we own.** A test asserts `.tshm` is byte-identical
      across a guarded open (hash before/after), and that the marker is absent from `.tshm`'s header region
      and from the database file. ADR-0025 records the location decision; this issue's code must not need a
      turso format version to read its own marker.
- [ ] **45.15** **(A4) The doc-vs-code contradiction is recorded** in ADR-0025's Grounds with both citations
      (docs `multiprocess-access.mdx:139-145`, `:78`, `:148` — labelled doc-sourced; code A1's line pairs),
      so the next reader cannot conclude the guard is redundant. No code change.

## Blocked by / Blocks

- **Blocked by:** nothing. ADR-0025's *text* is separately gated by issue 43's opsec report §9 items 4–5
  (`43:159`); this issue's code is not.
- **Blocks:** ADR-0025's guard section (the mechanism description is written from this issue's code, and it
  must carry A3's writability rule and A5's marker-location constraint); issue 46's detection leg (`46.8`
  reuses this probe — do not build a second one). **Cross-reference:** issue **53** may remove the
  encryption × multiprocess combination for the vault, which would shrink the matrix this guard covers; the
  guard must not assume the vault stays flag-on.

## Comments

- Filed 2026-10-25 from the opsec design-concerns audit (`.tmp/scratch/opsec-design-concerns-run-1/`,
  `VERDICT: APPROVED`, concern C1, verdict `SOUND`), per the audit's §6 recommendation 1.
- **Corrected before filing, per the brief's AUDIT RESULT block** (`20261025-design-concerns-audit-brief.md:34`):
  the `.tshm` open mode is **derived** by probing the lifetime byte lock at offset 0, not persisted; a
  legacy holder does **not** hold `-tshm`; therefore the probe must be two-directional with a post-open
  re-probe. The brief's original "`do_open` is the test-only twin" is also wrong — `do_open` is not
  cfg-gated (`turso_sdk_kit` enables `turso_core/conn_raw_api`), it is simply not called from this tree.
- **Not re-litigated here:** issue 43 decisions 1–9; and the four `-tshm` claims an earlier opsec run
  rejected (`43:154-157`). "Presence is liveness" stays rejected.
- **Measurement provenance:** the row-loss sink is issue 43's pinned probe (`b42905c`), quoted not
  re-measured by the audit (`REPORT.md:383-385`). That is why this issue is `needs_validation`.
- The architect's `45.6` (self-conflict) is an addition to the audit's scope, not a correction of it: the
  audit's two-directional probe is necessary but would brick the same-process double-open paths unless the
  self-reference rule is decided with it.
