# 45 — Security · Pre-open mixed-version guard: the engine's oracle cannot fire on any open path we use

Status: no triage label applies — this branch is **finished but uncommitted**, not landed. Branch
`issue-45/mixed-version-guard`, base `8a39e01`, 45 dirty paths, and **no commit exists**: the tree goes
back to a fresh reviewer before any commit, so no landing claim is available. (`ready-for-human` was wrong
at both ends — it means "requires human implementation"; the implementation is here.) The one open
residual is ADR-0025's *text*, owned by issue 43 (`43-database-base-hardening.md:159` gates it); issue 45's
inputs to it are recorded under `## ADR-0025 inputs` below.

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

## OWNER DECISIONS 2026-10-25 — grilling round (O1–O6), supersedes parts of this issue

Six decisions taken by the owner before implementation, each reached after the code was measured rather
than assumed. **Where a decision disagrees with the text above, the decision wins**; the text is kept as
history. The measurements that drove them are cited inline.

| # | decision | supersedes |
|---|---|---|
| **O1** | **Positive detection ⇒ refuse. "The probe cannot answer" ⇒ refuse too, but paired with an explicit override (O4).** A read-only degraded open was considered and **rejected**: the ready gate unconditionally appends three steps and two of them write (`ready.rs:226-246` — migrations, and interrupted-swap recovery which renames shard files), so a read-only database cannot complete `ensure_ready()`; `engine.rs:1270`'s `let db = self.store.ready().await?` then fails startup anyway, and skipping swap recovery would serve inconsistent shard paths while claiming to be up. | the posture is unchanged; this records the rejected alternative and why |
| **O2** | **Mirror turso's direction matrix exactly.** The live-multiprocess-authority probe applies **only when the opener is legacy** (`turso_core-0.8.3/database.rs:939`: `if opts.enable_multiprocess_wal \|\| is_memory_like(path) \|\| … { return Ok(()) }`); the legacy-holder probe applies only to a multiprocess opener (`:991`: `if !opts.enable_multiprocess_wal \|\| flags.contains(ReadOnly)`). | **"probe A … applies to every open"** — that rule refuses the *second legitimate multiprocess opener* (engine + `purge-session` today; any future concurrent reader), i.e. the configuration multiprocess WAL exists to serve. It is stricter than the engine in a way that breaks supported use. |
| **O3** | **Probe A is not implemented. A legacy open is refused by construction** with a typed `UnsupportedOpenMode` — the "refused" disposition `45.1` already permits per row. `ConnectOptions::for_config` pushes `MultiprocessWal` unconditionally on non-Windows (`db.rs:258-262`; `config` is not consulted — `let _ = &config.database_encryption_cipher`), so this tree cannot produce a legacy opener, and on Windows the `.tshm` leg is vacuous by construction. The `mode × writability → outcome` decision table is still implemented and tested, so the first legacy opener inherits a refusal rather than silence. | **`45.3`** (the `.tshm` probe), and the "probe A" half of **`45.13`** |
| **O4** | **One override, bypassing both outcomes**, named for what it does — it disables the mixed-version guard. Grounds: an OFD lock carries no `l_pid`, so "held" and "cannot lock" are not reliably distinguishable (a permission error or an unlucky filesystem is indistinguishable from a live holder), and a misclassification must never leave the user with no memory engine. Every bypass logs path + bypassed outcome + the fact that the override is active, and `health` reports it. | the unconditional **"Unknown ⇒ refuse"** sentence |
| **O5** | **The override is both an env var and a config key, env var winning when both are set:** `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` (precedent `OPENCODE_MEM_HANG_ON_SHUTDOWN`, `engine.rs:501`) and a config key. **The config key must carry a reader test** — issue 49's rule: a parsed-but-unread field is the exact defect that issue exists to remove, so a closed implementation with no reader fails review. | — (new) |
| **O6** | **`45.8` reduces to one integration direction** (live legacy holder ⇒ refuse) **plus** the decision-table test covering the multiprocess row. Two-directional positive controls are not meaningful when one direction cannot fire. The inversion arm stays: disable the probe, and that one direction's control must go red. | **`45.8`**'s "two positive controls plus their two inversion arms" |
| **O7** | **`45.6` is satisfied by proof, not by a mechanism.** Measured: `probe_b=Free` against our own **live multiprocess handle** — a multiprocess handle holds **no** lock on the database file, so probe B cannot see it. The distinct hazard the measurement did find is that **`close()` on a database file releases a POSIX lock the process holds on it**, and that requires an *in-process **legacy** handle*, which O3 refuses. Zero new state, zero per-open cost, nothing shipped that cannot fire. **The tripwire is structural:** the `UnsupportedOpenMode` test *is* the guard — any future change that admits a legacy opener fails it and must land the clearance mechanism in the same change. | the assumption (recorded here by me, and wrong) that self-conflict was "the main correctness risk of the remaining guard" |

**Unchanged by these decisions:** the seam (`ConnectFactory::connect`), the self-conflict requirement
(`45.6`), the typed-refusal constraints (`45.7`), the marker-location rule (A5 / `45.14`), the
`purge-session` requirement (`45.9`), the platform matrix (`45.10`), `libc`-only (`45.11`), and the cost
measurement (`45.12`).

**Corrected by measurement (2026-10-25), superseding a sentence previously written here.** It said the
self-conflict problem was "entirely a probe-B concern … our own cached handle is exactly such a
description", and that `45.6` was "the main correctness risk of the remaining guard". Both were **wrong**:
`probe_b` answers **`Free`** against our own live multiprocess handle, because a multiprocess handle holds
**no** lock on the database file. Under O2+O3 there is no self to conflict with, so `45.6` is **vacuous**
and is discharged by proof (O7) — not by a registry.

**A residual the A/B measurement did find, and the blueprint must carry:** `close()` on a database file
**releases a POSIX lock the process holds on it** — any fcntl variant, it is `close()` and not the lock
call. It is unreachable while O3 holds (it needs an in-process *legacy* handle), but it is the hazard the
clearance design existed to avoid, and it must be stated where the seam is described so nobody reinstates
a probe that opens and closes the database file while holding a lock on it.

**Disposition for `O3`'s only known cost:** two existing tests open *through the factory* with the
multiprocess feature removed (`crates/mem-store/tests/turso_capabilities.rs:2550`, `:2840`), and
`UnsupportedOpenMode` refuses exactly that. They must be changed to build their legacy handle through
`turso::Builder` **directly** — production code does not get a test-only escape hatch through the seam.

## A/B MEASUREMENT 2026-10-25 — design A wins (drafts + raw logs now tracked)

Two competing designs were drafted and scored against **one** discriminating measurement, per the
repo's standing workflow. Everything is tracked under `.scratch/rust-port/issue-45/` — `guard-a.md`,
`guard-b.md`, `measurement.md`, and `scratch/` (the harness, `apply-and-run.sh`, and six raw logs) —
because a ruling that lives only in `.tmp/` is one `rm -rf` from gone.

**Verdict: A, decided by one arm (T3).** Design B's own-process clearance table lets a **foreign legacy
holder of a sibling database in the same root** through, and the row is lost: `A=Refuse("legacy-authority")
B=Clear differ=true`, B path `final=[1,3]`, `lost_row_2=true`. A guard that clears on identity is a guard
that clears its own attacker.

**Reproduced, unfiltered:**

- **G1 holds.** The mixed open through `TursoConnectFactory` **succeeds**, and the opener's committed row
  is silently gone with `integrity_check = ok` (`t1`: `opener_before=[1,2]`, `final=[1,3]`,
  `lost_row_2=true`). The premise of this issue is confirmed by measurement, not by citation.
- G3 holds: stale `-tshm` with no holder ⇒ `probe_a=Free`.
- A2 holds as a **measurement** now, not a read: the flag-on holder holds no OS lock on the database file.
- A5 holds: `.tshm` is byte-identical across a guarded open (`before_len=86016 after_len=86016
  identical=true`).
- `size_of::<StoreError>() = 112` — `45.7`'s budget still fits.
- Cell 4 re-measured coherent (`[1,3,100]`, integrity ok), confirming O2 and refuting A3's original rule.

**Cost, measured (`45.12` — and the "two `fcntl` calls" estimate was an undercount):** probe B (one byte,
`F_OFD_SETLK`) mean **5.695 µs** / p95 5.640 / max 25.458; the lock+unlock syscalls alone 2.240 µs (OFD)
and 2.285 µs (`F_SETLK`); a bare open+close baseline 3.253 µs — each probe also opens and closes; marker
read+parse 7.115 µs; registry lookup 225.7 ns (`1735.5` ns with a per-op metadata call). **Under O3 the
guard runs one probe, not two.**

**What did not reproduce — recorded, not argued away:**

1. The self-conflict claim this issue's own O1–O6 block asserted is **false**; corrected in place above,
   discharged as O7 by proof rather than mechanism.
2. `O3` has an **unlisted cost**: two tests open through the factory with multiprocess removed
   (`turso_capabilities.rs:2550`, `:2840`); disposition stated above.
3. **`45.1`'s open-path table is incomplete**: it omits `encryption_migrator`'s four connect sites
   (including a staged temp path) and `mem-profile`'s per-operation coordination DB, and the largest test
   class **bypasses the seam entirely** — so the seam covers production, not every open in the tree. The
   table must say that, rather than imply the seam is universal.
4. A 0.9 upgrade still does not fix this issue (A1 of the amendment, re-confirmed at HEAD `aa93d669`).

**Rider (measured, not deferred):** A's registry is **not shipped**. Under O2+O3 its trigger — a live
legacy opener meeting a live multiprocess authority — cannot occur, and `probe_b` is `Free` against our
own handle. Its trigger binds the **first legacy opener** to land the clearance in the same change (O7).

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

## 45.1 — the open-path table (recorded before any code)

Every path that opens a database in this tree, the lock it holds today, and the guard's disposition for
it. Rows 9–10 are the two classes that make the seam *not* universal; row 12 is the Windows disposition.

| # | path | what it opens | lock held today | disposition | reason |
|---|---|---|---|---|---|
| 1 | engine `run_engine` → `composition` → `ConnectionManager` | shards, `metadata.db`, `user-prompts.db`, `user-profiles.db`, `episodic.db`, `facts.db` | `.engine.lock` (D-39) | **covered** | every open is `ConnectionManager::open_database` (`connection.rs:201-234`) → `TursoConnectFactory::connect` |
| 2 | `purge-session` (`src/main.rs` mode → `mem_bridge::purge::run:309-311` → `EpisodicStore`/`FactsStore` → their own `ConnectionManager`) | `episodic.db`, `facts.db` | `mem_episodic::gate::try_acquire` + `acquire_turso_operation_lock`; **no** engine lock | **covered** | same seam. It is the *legitimate second multiprocess opener* — O2 is what keeps it unrefused (probe B cannot see a multiprocess holder: `probe_b=Free`, `t1`). A refusal surfaces as `PurgeError::Failed` → exit 74. Decision 8's enqueue amendment is out of scope (45.9) |
| 3 | ready steps (`ready.rs`: `legacy_migration_step`, `path_migrate_recovery_step`, `shard_warmup_step`, + the encryption step) | row 1's DBs | engine lock (same process) | **covered** | they open through the same `ConnectionManager`; two steps write (`ready.rs:226-246`) |
| 4 | `encryption_migrator` — **5** connect sites in 3 functions: `is_already_encrypted:77`, `can_open_plain:93`, and inside `migrate_candidates`/`dump_and_reload_encrypted` the source open `:325`, the staged `*.encrypt-<millis>.tmp` open `:327`, the post-swap verify open `:363` | candidate DBs, source, staged temp file | engine lock; the staged path is a different file | **covered**, with a diagnosis gap fixed in Step 9 | the probe sees the path it is given; a fresh staged file is `Missing` ⇒ clear. `is_already_encrypted`/`can_open_plain` currently collapse **any** open error to `false`, so a guard refusal would be re-labelled "Cannot open … as plaintext" — Step 9 propagates it instead |
| 5 | `mem-profile` `LearningLock::open_coordination` (`learning_lock.rs:779-793`, connect at `:790`) | the per-operation coordination DB (encryption forced off, multiprocess kept) | none; same-process, multi-handle, once per operation | **covered** | multiprocess ⇒ probe B `Free`; a refusal reaches the caller as `ProfileError::Store` (`error.rs:16`) |
| 6 | `mem-vault` per-scope `vault.db` (`store.rs` `ready` → own `ConnectionManager`) | vault DBs | engine lock when in-engine | **covered** | always encrypted (`store.rs:112-114`, `:271`), i.e. permanently in issue 53's combination — do not assume it stays flag-on |
| 7 | `mem-profile`/`mem-skills`/`mem-episodic`/`mem-facts` store constructors | their DBs | none of their own | **covered** | all construct a `ConnectionManager` over the shared factory |
| 8 | tests opening through `TursoConnectFactory` (most suites) | fixtures | none | **covered** | same seam; no call-site change is needed for the guard to apply (45.2) |
| 9 | `crates/mem-store/tests/turso_capabilities.rs` raw `turso::Builder` handles — `open_single_process:1613`, `open_with_vacuum`, `open_encrypted_vacuum`, the mixed-mode cell matrix, the crash-probe child | fixtures | none | **safe by construction / not covered, deliberately** | the largest test class bypasses the seam on purpose: it measures the engine, not the guard. Any future *production* `turso::Builder` call leaves this contract (draft A §10.2) |
| 10 | three seam-based **legacy** test sites: `open_default` (`ConnectOptions::default()`, used by `test_new_database_defaults_to_wal_journal_mode`, AC1) and `open_single_process_db` (`single_process_options`, used at `:2550` and `:2840`) | fixtures | none | **refused by construction (O3)** | `UnsupportedOpenMode`. The issue names two sites; re-derived here as **three test functions through two helpers** (`grep -n 'open_single_process_db\|open_default('`). Step 5 moves them onto the raw builder; no test-only escape hatch is added to production code |
| 11 | a future read-only opener (backup/export) | — | — | **covered by contract** | `Writability` is in the probe's signature; behaviour untested until such an opener exists (45.13) |
| 12 | Windows | every open resolves to `WalMode::Legacy` (`for_config` never pushes `MultiprocessWal` there, `db.rs:258-269`) | `.engine.lock` where in-engine | **refused** (`UnsupportedOpenMode`) | O3 refuses legacy opens on every platform, so this is the Windows disposition — see the flagged consequence in the guard blueprint's SECURITY & EDGE CASES. The override is the escape |

## ADR-0025 inputs (45.14 / 45.15) — recorded here, ADR text still gated by 43:159

ADR-0025's text is gated by `43-database-base-hardening.md:159` (REPORT §9 blockers 4–12), so the inputs the
ADR will need are recorded here instead. No `docs/adr/0025-*.md` file is created by issue 45.

1. **A4's doc-vs-code contradiction.** Docs (`multiprocess-access.mdx:139-145`, `:78`, `:148` — *doc-sourced*,
   the `.mdx` is not on disk in this tree) say the open path "probes for an incompatible live opener and
   fails fast" and "mixing modes is rejected", with read-only opens "advisory". The code says the oracle is
   unreachable from our path: `turso_core-0.8.3/database.rs:1153`, `:1163`, `:1173` (the probes), `:1222`
   (`resolve_default_storage` only when `options.storage.is_none()`), `:991-993` (the read-only exemption),
   `:939-945` (the legacy-open leg), and at HEAD `aa93d669` `core_database.rs:1283-1284`, `:1314-1328`,
   `:1436`. A reader who trusts the docs believes the guard is redundant; the code says it is absent.
2. **A5's marker location and format.** File name `.wal-mode` in the storage root (a file we own), body one
   JSON record `{pid, timestamp, start_time, mode}` reusing the shipped `LockRecord` shape. It cannot live
   in `.tshm`: that file is turso-owned, magic/version-tagged (`shared_wal_coordination.rs:39-43`),
   version-checked on open (`:438-446`), and resized at runtime (`:944`), so a marker there would be
   clobbered, invalidated by a version bump, or read back as `Corrupt`.
3. **O2's direction matrix**, with A3's superseded "probe A every open" rule kept as history. Probe A (a live
   multiprocess authority ⇒ refuse) applies only to a **legacy** opener; probe B (a live legacy DB-file lock
   ⇒ refuse) applies only to a **writable multiprocess** opener (`0.8.3 :939`, `:991`). Under O3 probe A is
   not implemented at all — a legacy open is refused by construction — so probe A's rule lives in the
   decision table, not in a syscall.
4. **The POSIX close-releases-locks finding and the two rules it forces.** `close()` on a database file
   releases the process's own POSIX locks regardless of the fcntl variant (`t2 S4/S6/S7`); therefore (i) no
   probe may open and close a database file this process holds a lock on, and (ii) the day an in-process
   legacy handle exists, the identity-keyed clearance registry lands **before** the probe is allowed near it.
5. **The O7 vacuity proof and its precondition.** `probe_b = Free` against our own live multiprocess handle
   (`t1`) because a multiprocess handle holds no OS lock on the database file (A2). Precondition for the
   proof to hold: **no production `turso::Builder` call outside the seam** — the day one appears, the
   contract is void and the registry must land.
6. **O4/O5's override.** One override, env `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` winning over config key
   `allowUnguardedOpen`, bypassing both outcomes, logging path + bypassed outcome + override source, and
   reported by `health` as `open_guard.override_active`.
7. **The read-only divergence.** A read-only multiprocess opener skips probe B (it can be a victim, never an
   aggressor — `0.8.3 :991-993`), and the skip is a `Divergence::ReadOnlySkipsLegacyProbe` value in the
   decision core, not a comment. `turso::Builder::read_only` exists (`turso-0.8.3/src/lib.rs:284`) and the
   factory must apply `ConnectOptions::read_only` for the divergence to be honest.
8. **The G4 residual sentence, verbatim.** *What remains uncovered is a holder that arrives after the
   post-open re-probe and before our first write; the window is narrowed twice and never closed, because
   holding the lock ourselves would make us the holder the probe looks for.*
9. **Design A §11's registry trigger.** The identity-keyed clearance registry is not shipped; its trigger —
   a live legacy opener meeting a live multiprocess authority — cannot occur under O2+O3, and the first
   legacy opener must land the clearance in the same change.

## IMPLEMENTATION RECORD 2026-10-25 — dispositions, numbers, residuals

Implemented from the approved blueprint (`20261025-1651-issue-45-guard`, `SYS-APPROVED`), with the six
blocking Plan-Gate corrections applied — uncommitted, so nothing has landed. Every number below cites `measurement.md` / a raw log or the
test that printed it.

**45.3 — superseded by O3; not implemented.** The `.tshm` offset-0 probe ships in neither direction.
`t2 S1 probe_a=Held` and `t2 S2 probe_a=Free` are the reasons it would have been both
**self-conflicting** (our own live multiprocess handle holds that byte lock, so a second open of the
same file would refuse itself) and **non-discriminating** (the same lock answers the same way for a
foreign holder and for us). A legacy open is refused by construction with `UnsupportedOpenMode`
instead, and that refusal **is** the tripwire: the first legacy opener fails
`test_legacy_connect_options_are_refused_with_unsupported_open_mode` and must land design A §11's
identity-keyed clearance registry in the same change.

**45.6 — discharged by O7's vacuity proof, not by a mechanism.** `probe_b = Free` against our own
**live multiprocess handle** (`t1`, and re-measured by
`test_probe_reports_free_against_this_process_own_live_multiprocess_handle`), because a multiprocess
handle holds **no** OS lock on the database file (A2). Plus
`test_second_open_of_a_database_this_process_holds_is_not_refused` (the double open) and the
`UnsupportedOpenMode` tripwire as the structural guard. No process-local registry ships: under O2+O3
there is no self to conflict with.

**45.12 — the measured cost.** Probe B (one byte, `F_OFD_SETLK`) mean **5.695 µs**, p95 **5.640**,
max **25.458** (n=2000, debug profile); the lock+unlock syscalls alone **2.240 µs** (OFD) and
**2.285 µs** (`F_SETLK`); bare open+close baseline **3.253 µs**; marker read+parse **7.115 µs** ⇒
**≈12.8 µs per writable multiprocess open after the first**, **0** for read-only opens, **0** for legacy
opens (refused before any syscall). The first open per root additionally pays the marker's staging write
— `create_new` → write → `sync_all` → `rename` — not measured here.
*Correction carried from the Plan Gate:* the Δ12.8 µs figure was derived with the `F_SETLK` syscall
number while the shipped Linux path uses the OFD pair, so it is **≈1 % conservative** (2.240 vs
2.285 µs). Re-measure in a release profile before ADR-0025 quotes any absolute.

*Marker cost and failure mode (review round 3, scoped in round 4):* a writable open that must **create or
rewrite** the marker, or **cannot read** it, is refused when this process cannot write the storage root —
typed `OpenBlocked { Unknown { errno } }`, override the only escape. The exception is the live-marker reuse
path: a root whose marker this process already wrote and can still read returns `Ok` (measured: a writable
multiprocess open in a 0555 root with our own live marker succeeded, no override), so an unwritable root is
fatal only to the opens that must touch the marker. The marker write performs a blocking `sync_all` on the
calling (runtime) thread, once per marker write — the ≈12.8 µs figure does **not** include it.

**45.10 — the platform matrix, stated and tested.** Linux: `F_OFD_SETLK`
(`test_probe_reports_held_for_a_live_legacy_holder`, measured in both directions).
Other Unix: `F_SETLK`; the **syscall path is exercised on Linux** through `LockKind::Posix`
(`test_probe_posix_variant_reports_held_for_a_live_legacy_holder`,
`..._free_for_an_unheld_database_file`), while the platform *selection* is **untested** — no runner.
Windows: **refused by construction** (`db.rs:258-269`: `for_config` never pushes `MultiprocessWal`
there, so every open is `WalMode::Legacy`), with `OPENCODE_MEM_ALLOW_UNGUARDED_OPEN=1` /
`allowUnguardedOpen: true` as the documented escape; the Windows runtime is **untested**.

**45.8 / G1 — recorded, not re-run.** The pre-fix unguarded open *succeeds* and the opener's committed
row is silently lost with `integrity_check = ok`: `opener_before=[1,2] final=[1,3] lost_row_2=true`
(`.scratch/rust-port/issue-45/scratch/t1.log`). It is not re-run because the fix removes the state it
observes. G2 is the pairing of `test_factory_open_refuses_a_live_legacy_holder` (control) and
`test_override_opens_the_same_mixed_configuration_the_guard_refuses` (inversion arm), plus the
recorded manual mutation check: with the factory's probe replaced by a constant `Free` (`// INVERT`),
**exactly** the control goes red (`Ok` instead of `OpenBlocked`) and the suite is green again after the
revert.

**Which direction the shipped probe guards.** It is the **legacy-holder-vs-writable-multiprocess**
direction (`t3`'s), *not* `t1`'s. `t1`'s loss is the legacy-opener-vs-live-multiprocess-authority
direction, which O3 removes by construction (a legacy open never happens). Saying "the guard detects
`t1`" would be false.

**G4 residual sentence (verbatim).** *What remains uncovered is a holder that arrives after the
post-open re-probe and before our first write; the window is narrowed twice and never closed, because
holding the lock ourselves would make us the holder the probe looks for.*

Adjacent, also uncovered and stated wherever the seam is described: (i) a legacy handle created
**outside the seam** (a future production `turso::Builder` call) — outside the contract and destructive
to probe; (ii) a `.tshm`-only multiprocess authority is not detected, because probe A is not
implemented (O3: only the first legacy opener needs it, and `UnsupportedOpenMode` is the barrier until
then); (iii) read-only opens skip probe B — they can be victims, never aggressors (mirrors
`0.8.3 :991-993`), and the divergence is a `Divergence::ReadOnlySkipsLegacyProbe` value, not a comment;
(iv) a caller with no `storage_root` (`ConnectOptions::default()`, hand-built options) writes no marker;
(v) the marker's own liveness inherits issue 43's `hidepid=2` fail-open — a live foreign marker can read
dead and be replaced, so the marker is mode *truth*, never a clearance input; (vi) no holder identity is
claimed, ever.

**Probe-vs-in-process-legacy residual (round 4, measured at the public seam — documented, NOT fixed).**
An overridden legacy handle is a legitimate in-process legacy opener, because `allow_unguarded_open` is a
public per-call field (`ConnectOptions`, `config.rs:28` / `db.rs:257`). Measured sequence:
`probe_database_file` on that live path = **`Held`**; the probe's own open+close then releases *this
process's* POSIX lock (`t2 S4/S6/S7`), so the next probe = **`Free`**, and a subsequent **guarded** writable
multiprocess open therefore **proceeds** while the legacy handle is still live — the mixed-mode row loss this
guard exists to prevent. Pinned by
`test_probe_releases_an_in_process_overridden_legacy_lock_and_the_guarded_open_then_proceeds`, which must
**fail** if either remedy below lands, forcing the decision instead of a silent behaviour change.
*Seam:* reachable only through the per-call `ConnectOptions.allow_unguarded_open` field. *Why the engine
cannot reach it:* the engine resolves one `StoreConfig` per process and derives every `ConnectOptions` from
it (`for_config`), so no per-open divergence between legacy and multiprocess exists there; the sequence needs
a caller that builds two different option sets against the same root in one process. *Remedies left to the
owner:* (a) constrain the override to the process-wide legs only (env var + config key, no per-call field),
or (b) land design A §11's identity-keyed clearance registry before the probe may meet an in-process legacy
handle. Neither is taken here — both are design/API decisions outside this issue's approved scope.

**Marker footgun (Plan Gate residual 6, recorded in `open_guard.rs`'s module doc).** An *overridden*
legacy open writes `{"mode":"legacy"}` into the root's `.wal-mode` marker; a later **guarded** writable
multiprocess open **from another live process** then refuses with `ModeMarkerConflict` for the lifetime of
the process that wrote it. Two scoping exceptions, both measured, which is why the rule is not "every later
open": a later writable multiprocess open **in the same process** reuses its own marker and returns `Ok`
(`ensure_mode_marker`'s same-pid short-circuit), and an **overridden** reader demotes the conflict like any
other marker failure (the RC2 rule below). Reachable only when the override is used
(`OPENCODE_MEM_ALLOW_UNGUARDED_OPEN` / `allowUnguardedOpen`) and the writing process outlives the next
writable open — which is why it is a footgun and not a clearance input.

## REVIEW ROUND 2026-10-26 — three P0 blockers, fixed

The `code-reviewer` returned `STATUS: REJECTED`; a `debugger` measured each blocker and the fixes below
were applied in its prescribed order. **The code was the defect, not the records**, in all three.

**RC1 (security, P0) — the marker's staging write followed a planted symlink.** The staging file was
opened with `create(true).truncate(true)`, whose `O_CREAT|O_TRUNC` **follows** a symlink: a victim file was
cut to the marker body (`shipped truncate shape: victim bytes = b'X'`, a 24-byte victim reduced to 1) and
the following `rename` installed the symlink as `.wal-mode`. Fixed by `O_EXCL` (`create_new`) with the
`MARKER_STAGE` counter and `MARKER_STAGE_ATTEMPTS = 16`, cleanup **only** for a staging file this call
created (on `EEXIST` the file is another process's), and exhaustion as the **typed**
`OpenBlocked { Unknown { errno: Some(EEXIST) } }`. Measured counterpart: `create_new` on a planted symlink
→ `errno 17 EEXIST`, victim after: `b"ORIGINAL"`. A directory-relative `openat` was **not** added: the same
actor who can plant a symlink can write `.wal-mode` outright, and the marker is never a clearance input.

**RC2 (contract, P0) — the override skipped the marker, making `ModeMarkerConflict` unreachable.**
Measured at the public seam: an overridden legacy open returned `Ok` with `marker_exists = false`. The
blueprint conditions the marker on `Writability::Writable` only, and "failures are refusals subject to the
same override" presupposes a failure was produced. Fixed: the marker is attempted for **every writable**
open, refused while the guard is active, and **demoted to a warning** under the override. `Clearance
{ guarded: !override_on }` and `postflight` are untouched: the override skips the probe **both pre and
post** — the post-open one via `postflight`'s early return, the pre-open one via the `!override_on` gate
round 3 added to `preflight` (round 2 removed only the marker gate here and left the pre-probe firing).

**RC3 (diagnostics, P0) — marker failures escaped as `StoreError::Io`.** `create_dir_all` was fatal and
`read_marker`'s non-`NotFound` arm returned `Io`, which `connection.rs:216` launders into
`StoreError::Turso` — measured `Turso { message: "Is a directory (os error 21)" }` with
`is_open_guard_refusal() == false`, naming neither `.wal-mode` nor the override (and with encryption on,
the actively wrong "Check databaseEncryptionKey"). Fixed: one `marker_refusal()` for the marker's whole
surface, `create_dir_all` best-effort with a `tracing::warn!` (Step 7.1's "best-effort"), and
`OpenDirection::Unknown`'s Display now names `.wal-mode` and **both** override legs. No new variant; no
widening of `is_open_guard_refusal`.

**Five `.`-rooted test sites** in four files (`mem-bridge/tests/vault_feeder.rs` ×2,
`mem-vault/tests/{vault_store,vault_custody,vault_isolation}.rs`) no longer claim `.` as a storage root (`options.storage_root = None`),
so no test writes a marker into a package directory and `.gitignore` needs no entry for it. The three
`table_is_readable` copies now return `Result<bool, StoreError>` with the Step-9 three-arm classifier, so
`assert!(!table_is_readable(..))` can no longer pass vacuously on a guard refusal.

**45.9's proof now exists**: `test_purge_session_option_set_stays_multiprocess` (option classification
plus the behavioural half against a live engine-shaped handle). It was missing *evidence*, not a
regression test — it is green by construction.

**Residual added** (also in `open_guard.rs`'s module doc): *the reclaim path is last-writer-wins: a marker
installed inside the `read_marker` → `rename` window can be replaced. The marker is mode truth, never a
clearance input — the database-file probe is the enforcement.*

**45.12 cost correction:** the first-open marker cost is not `create_new` + `sync_all`; the shipped
protocol is staging `create_new` → write → `sync_all` → `rename`, i.e. one extra `rename` (same
directory) beyond what the record says. Unmeasured, still unmeasured here.
