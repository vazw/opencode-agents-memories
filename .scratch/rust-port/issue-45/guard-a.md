# Issue 45 — Design **A** (re-derived under O1–O6): probe B behind `ConnectFactory::connect`, own-process clearance by a process-local registry of file identities — **pre-registered, not shipped**

Status: architect draft, competing with design B. **Authority:** `.scratch/rust-port/issues/45-mixed-version-open-guard.md`
at `c18d0b1` — the 2026-10-25 AMENDMENT A1–A5 **and the 2026-10-25 OWNER DECISIONS O1–O6**, where the
decisions explicitly win over the older text. Measured evidence:
`.tmp/drafts/20261025-2304-issue-45-measurement.md`; raw logs `.tmp/scratch/issue-45-ab/`.
Labels: `MEASURED` / `READ` / `INFERRED`.

> This draft was first written against the pre-amendment text (`probe A applies to every open`). O2/O3
> supersede that rule, and the change is not cosmetic: it removes probe A, and with it the reachable need
> for any own-process clearance. §3 states the proof, §4 states what survives, and §11 states the
> conditional design that the first legacy opener must inherit.

---

## 1. Interface

One module, one seam, two typed refusals, one override. `-tshm`, byte offsets, fcntl and the marker stay
inside `crates/mem-store/src/open_guard.rs`.

```rust
pub(crate) enum WalMode { Legacy, Multiprocess }        // from ConnectOptions::experimental
pub(crate) enum Writability { Writable, ReadOnly }      // from ConnectOptions::read_only (new field)

pub(crate) fn preflight(db_path: &Path, mode: WalMode, writability: Writability)
    -> Result<Clearance, StoreError>;                   // O2/O3 decision table + the O4 override
pub(crate) fn postflight(clearance: &Clearance) -> Result<(), StoreError>;  // probe B, post-open
```

Factory (`db.rs:320-338`, the only production implementation of the seam `READ`):

```rust
let clearance = open_guard::preflight(&path, mode, writability)?;   // may refuse; never mutates the file
let database = builder.build().await?;                               // existing body, unchanged
if let Err(refusal) = open_guard::postflight(&clearance) {
    drop(database);                                                  // 45.5: no owner left behind
    return Err(refusal);
}
let connection = database.connect()?;
Ok(Arc::new(TursoDb { database, connection, _open: clearance.into_token() }))
```

`ConnectOptions` gains `pub read_only: bool` (default `false`): O2 keeps a read-only exemption in the
direction matrix, and the seam receives nothing else that could express it. `READ`: no read-only open
exists in the tree today (`turso::Builder::read_only` is never called; `grep -rn read_only crates src`
matches shard-metadata semantics only). Nothing sets it in this issue; the token exists so the first
read-only opener inherits the contract.

Deletion test: delete the module and the `-tshm`/fcntl/marker knowledge must reappear at every call site —
five today (`ConnectionManager`, four `encryption_migrator` sites, `mem-profile`'s coordination DB), and the
next open path misses it. The module is deep because that knowledge never leaves it.

## 2. Decision core (O2/O3 — shared with design B verbatim; the A/B is *not* here)

```rust
fn decide(mode: WalMode, writability: Writability, b: Option<Probe>) -> Decision {
    match mode {
        // O3: this tree cannot produce a legacy opener (for_config never consults config for the
        // mode), so a legacy open is refused by construction rather than silently unguarded.
        WalMode::Legacy => Decision::RefuseUnsupportedMode,
        WalMode::Multiprocess => match writability {
            // O2 mirrors turso's own exemption (0.8.3 database.rs:991-993): a read-only
            // multiprocess opener never runs the legacy-holder probe. Divergence recorded.
            Writability::ReadOnly => Decision::Clear { divergence: ReadOnlySkipsLegacyProbe },
            Writability::Writable => match b {
                Some(Probe::Held)          => Decision::Refuse(Direction::LegacyAuthority),
                Some(Probe::Unknown(e))    => Decision::Refuse(Direction::Unknown(e)),
                Some(Probe::Free) | Some(Probe::Missing) => Decision::Clear { divergence: None },
                None => Decision::Refuse(Direction::Unknown(-1)), // cannot happen: Writable ⇒ Some
            },
        },
    }
}
```

- **Probe A does not exist** (O3). The `.tshm` probe's mechanics are measured and workable, but they are
  unreachable on any supported platform (no legacy opener ⇒ nothing to protect from a multiprocess
  authority), and implementing them would create the self-conflict §3 measures. `45.3` is superseded; the
  measurement is kept as the reason.
- The `mode × writability → outcome` table is still implemented and tested (O3's last clause), so the first
  legacy opener inherits a refusal rather than silence. The table is a **pure function of
  (mode, writability, probe)** — 45.13's decision-table test needs no filesystem race.
- **O4 override** is applied at the refusal site, not inside `decide` (the function stays pure):
  `preflight` maps `Decision::Refuse*` through `override_active(env OPENCODE_MEM_ALLOW_UNGUARDED_OPEN, config
  key, env wins)`. When active, it emits one structured log line naming the path, the bypassed outcome and
  the fact that the guard is disabled, and returns `Clear { overridden: true }`; `health` reports the same.
  O5's reader test is an acceptance item, not an option.
- `Probe` is `Free | Held | Missing | Unknown(errno)`. `Missing` is a *known* answer ("the file does not
  exist ⇒ no holder can"), never `Unknown`; this is what keeps a fresh root from bricking.

### Refusals (45.7)

```rust
#[non_exhaustive]
OpenBlocked { db_path: PathBuf, direction: OpenDirection }   // LegacyAuthority | Unknown(i32)
UnsupportedOpenMode { db_path: PathBuf, mode: &'static str }
```

- Names the path and the detected direction; `Unknown` is its own value; **no pid** (an OFD lock has no
  `l_pid`; `READ`: `turso_core-0.8.3/io/unix.rs:125-180` sets `l_pid: 0`).
- Size: `MEASURED` `size_of::<StoreError>() = 112`, `Option<StoreError> = 112`, `(PathBuf, u8) = 32`. Two
  new variants with ≤32-byte payloads leave the enum at 112; 45.7's ≤128 holds with margin. (The issue's
  `112` reproduces exactly — it was not inherited.)
- `MigrationLocked` (`error.rs:80`) is the naming precedent; it carries a pid because its file body does.
  This one must not.

## 3. Self-conflict under O2/O3 — **the requirement is vacuous, and here is the proof**

The amendment says: *"with probe A gone, the self-conflict problem is entirely a probe-B concern on Linux —
an exclusive lock on the database file conflicts between open file descriptions, including two in the same
process, and our own cached handle is exactly such a description."* **The measurement does not reproduce
that.** It is the one amendment claim this wave refutes, and the correction matters because it removes the
need for the mechanism both designs were asked to provide.

Chain (`MEASURED` unless marked):

1. Probe B runs only for a **multiprocess** open (O2).
2. A multiprocess open takes **no** lock on the database file — turso sets `OpenFlags::NoLock`
   (`READ`: `turso_sdk_kit-0.8.3/src/rsapi.rs:1002-1003`; `turso_core-0.8.3/database.rs:861-892`, the
   `flags | OpenFlags::NoLock` return at `:888`) and probe B measured against a live flag-on holder
   returns **`Free`** (t1: `t1 probes_live_flag_on_holder probe_a=Held probe_b=Free`).
3. Therefore a probe-B target can only be locked by *us* if this process holds a **legacy** handle on it.
4. A legacy handle cannot be created at the seam: `for_config` pushes `MultiprocessWal` unconditionally on
   non-Windows (`READ`: `db.rs:253-269`, config not consulted) and O3 refuses any legacy `ConnectOptions`
   with `UnsupportedOpenMode`.
5. Therefore **`preflight` can never probe a description this process owns** — not because a registry clears
   it, but because the mode that would take the lock is unrepresentable at the seam.

Corollaries, all measured on this host:

- `45.6`'s test ("open the same fixture twice without closing the first handle") **passes today with no
  clearance at all**: `t2 S1 own_flag_on probe_a=Held second_factory_open=ok`; put through the amended
  decision core the second open runs probe B, which returns `Free` (`t1`, same handle shape).
- The pre-amendment self-conflict was **real, but it belonged to probe A**, not probe B: `.tshm` byte 0 is an
  OFD lock and `F_OFD_SETLK` conflicts between descriptions including two in the same process
  (`t2 S1 probe_a=Held`). O3 deletes probe A and the conflict with it. Nothing about probe B conflicts.
- What probe B *does* do is destructive, and this is the real constraint the amendment was reaching for:
  **any open+close of the database file in a process holding a POSIX record lock on it releases that lock**
  (`t2 S4/S6/S7`: after the probe, a foreign legacy opener gets past the db-file lock and is refused only by
  the `-wal` lock; true for `F_SETLK` whole-file, `F_SETLK` one byte and `F_OFD_SETLK` one byte — the release
  comes from `close()`, not from the fcntl). Under O3 this can only bite a hand-built legacy handle
  (`turso::Builder`, the W0 matrix), which is outside the guard's contract; the condition that makes it
  reachable through the seam is reintroducing a legacy opener, and that is exactly the day the registry
  (§11) must ship *first*.
- Probe B must still be one byte at offset 0 with `F_OFD_SETLK` on Linux, so a *foreign* holder's **POSIX**
  lock is reported `Held` rather than replaced-and-released (`t2 S7`: `Held`). This is not clearance; it is
  the probe not being destructive to other people's locks.

## 4. Probe B mechanics (the only remaining probe)

- File: the database file itself. `open(O_RDWR)` **without `O_CREAT`**; absent ⇒ `Missing`.
- `fcntl(F_OFD_SETLK)` on Linux / `F_SETLK` elsewhere; `F_WRLCK`, `l_start = 0`, `l_len = 1`, `l_pid = 0`.
- `EAGAIN`/`EACCES` ⇒ `Held`; success ⇒ `F_UNLCK`, `Free`; other ⇒ `Unknown(errno)`. `EACCES` from `open`
  ⇒ `Unknown` ⇒ refuse (writable), overridable per O4.
- Pre-open **and** post-open. Post-open is the only place the residual window narrows, and only one
  direction can still bite: after our own multiprocess open we hold no db-file lock, so a **legacy** holder
  that arrived during the open is observable — `Held` ⇒ drop the database, refuse (45.5).
- Refusal after open drops the handle before returning; a flag-on open takes no OS lock, so the file is left
  exactly as the pre-open snapshot found it. The test asserts `integrity_check` and row equality against the
  snapshot, not just "no crash".

**G4 residual sentence (one):**
> What remains uncovered is a holder that arrives after the post-open re-probe and before our first write;
> the window is narrowed twice and never closed, because holding the lock ourselves would make us the holder
> the probe looks for.

## 5. The A5 marker

Written by every guarded **root owner**, read by nobody in design A's shipped path (A5's mode truth is for
*other* builds and for the first legacy opener). One record per root, fixed name by ADR-0025, body
`{pid, timestamp, start_time, mode}`:
- `create_new` + `sync_all`; `AlreadyExists` ⇒ read it.
  - live ours ⇒ reuse (mode is fixed for the process);
  - live foreign, same mode ⇒ leave it; we are not the owner and do not pretend to be;
  - live foreign, different mode ⇒ `OpenBlocked { Unknown }` for a writable open (two legacy builds share no
    `-tshm`; the byte probe cannot see this collision at all);
  - dead/corrupt/zero-length ⇒ unlink best-effort, retry `create_new` once, else `Unknown` ⇒ refuse a
    writable open (overridable).
- Location: never `.tshm` (A5: magic/version at `shared_wal_coordination.rs:39-43`, version-check at
  `:438-446`, resize via `shared_wal_set_len` `:944`), never the DB header or WAL. `MEASURED`:
  `.tshm` is 86 016 bytes at rest and **byte-identical** across a probe (`t2 S5 identical=true`) — 45.14's
  hash assertion is mechanically available.
- `READ`: the marker's liveness reuses `LockState`/`read_live_lock`'s start-time token semantics
  (`operation_lock.rs:42-136`) as a *format*, not as a clearance input. Design A needs no `ProcessProbe` in
  the open path (it only writes; the read path is reached only on `AlreadyExists`, and then a foreign live
  record is the interesting case, where liveness must be decided — so one `read_live_lock` call, not one per
  open).

## 6. Open-path table (45.1)

| # | path | today's lock | disposition |
|---|---|---|---|
| 1 | engine (`mem-bridge` `run_engine`): shards, `metadata.db`, `user-prompts.db`, `user-profiles.db`, `episodic.db`, `facts.db`, vault | `.engine.lock` (D-39) | covered (seam) |
| 2 | `purge-session` (`src/main.rs:328`): `mem_episodic::gate::try_acquire` + `acquire_turso_operation_lock`, **no** engine lock | operation lock | covered (opens via `EpisodicStore`/`FactsStore` → `ConnectionManager` → factory, `READ`); it is a *legitimate second multiprocess opener*, which is precisely why O2 removed probe A |
| 3 | ready steps (`ready.rs:150-185`, three steps; two write — `ready.rs:226-246`) | engine lock (same process) | covered |
| 4 | `encryption_migrator` (4 sites, incl. a staged `*.encrypt-*.tmp`) | engine lock; staged path is a different file | covered; probe B sees a distinct path and a distinct absence of locks |
| 5 | `mem-profile` `LearningLock::open_coordination` (`learning_lock.rs:790`) — per operation, encryption off, multiprocess kept | none | covered; a same-process multi-handle opener, so 45.6 has a real subject even though it needs no clearance |
| 6 | tests through `TursoConnectFactory` | none | covered |
| 7 | tests through a raw `turso::Builder` (`turso_capabilities` W0 matrix) | none | **not covered** — they bypass the seam by design and keep measuring the engine |
| 8 | future read-only opener (backup/export) | none | covered by contract (`Writability`), behaviour untested until it exists |

**New finding for 45.1's "each test harness class" row:** two existing tests open *through the factory* with
a legacy option set — `turso_capabilities.rs:2550` (`open_single_process_db`) and `:2840` — and O3's
`UnsupportedOpenMode` refuses exactly that. The implementation needs a deliberate answer (a `#[doc(hidden)]`
test-only constructor on `ConnectOptions`, or moving those two probes back onto the raw builder and losing
the seam fidelity they were written for). **Not decided here** — it is a 45.1 disposition question the
syseng gate must settle, and it is cheap now and expensive after the tests go red.

## 7. Platform matrix (45.10)

| platform | probe B | status |
|---|---|---|
| Linux | `F_OFD_SETLK`, byte 0, 1 byte | **MEASURED** both directions (refusal of a live legacy holder; `Free` with no holder; self-conflict absent) |
| other unix | `F_SETLK` byte 0 | **untested** — named as untested; note the measured POSIX close-release makes this path's failure mode strictly worse for a hand-built legacy handle in the same process |
| Windows | vacuous by construction: no multiprocess WAL (`db.rs:258-262`), so a legacy open ⇒ `UnsupportedOpenMode` and no probe exists | recorded rationale, not `Unknown`-refusal |

## 8. Cost (45.12) — MEASURED (debug profile, n=2 000, `t4.log`)

| item | mean |
|---|---|
| probe B (`open`+`F_OFD_SETLK`+`F_UNLCK`+`close`) | **5.70 µs** (p95 5.64, max 25.5) |
| the fcntl pair alone | **2.29 µs** |
| bare `open`+`close` baseline | 3.25 µs |
| registry lookup, precomputed key | 226 ns |
| pre-open `metadata` (had the registry shipped) | 1.74 µs |
| marker read+parse (design B's clearance; A reads only on `AlreadyExists`) | 7.12 µs |

**"Two fcntl calls" was the pre-amendment figure; under O3 it is one probe ⇒ two fcntl calls (lock +
unlock) in ~2.3 µs, and the probe as a whole ~5.7 µs per writable multiprocess open.** Read-only opens:
zero. Legacy opens: refused before any syscall. Debug profile: the syscall components dominate; re-measure
in release before ADR-0025 quotes absolutes.

## 9. Pros

1. **Only pays for what can fire.** Probe A is gone with its reason recorded; the marker is written once per
   root; no per-open read; no liveness machinery in the open path.
2. **The proof in §3 is a contract, not an optimisation**: "probe B never inspects a description of ours"
   follows from (probe B ⇐ multiprocess) ∧ (multiprocess takes no db lock) ∧ (legacy refused). That is a
   rule the next concurrent opener cannot violate without changing the mode matrix.
3. **Cheapest correct refusal**: 5.7 µs per writable open, 0 for the common own-handle path, versus B's
   7.1 µs on *every* open.
4. **No new on-disk authority**: one write-only marker (A5) + one in-memory table that is *not shipped until
   needed* (§11). Nothing new to corrupt, brick, or migrate.
5. The refusals are typed, sized, pid-free, and carry the path (45.7).

## 10. Cons

1. **45.6 is satisfied by proof and a passing test, not by a mechanism** — which is honest but means the
   guard has no defence the day a legacy opener returns. Mitigation is a written precondition in ADR-0025
   (§11), not code. A reviewer who wants machinery for every stated risk will read this as under-building;
   the measurement is the answer.
2. **The §3 proof is conditional on the seam's completeness.** A raw `turso::Builder` legacy handle in the
   same process is outside the contract and would be damaged by probe B if the guard ever probed it. `READ`:
   no production `Builder` call exists today. ADR-0025 must record the dependency.
3. `ConnectOptions::read_only` is a public-field addition with no setter today (O2 forces it into the
   signature).
4. Post-open probing doubles the syscall cost of an open in the refusal-region (5.7 → 11.4 µs) and its
   direction table is `INFERRED` design reasoning, not yet a test (the refuting observation: invert the
   post-probe direction and see whether the other direction still produces a typed refusal).
5. The mode at the seam comes from `ExperimentalFeature::MultiprocessWal`'s *presence*; a future feature
   list that implies a mode without that token would silently classify as legacy and refuse — the classifier
   needs a test of its own.

## 11. The clearance mechanism: pre-registered, and the trigger that must instantiate it

Design A's registry remains the correct mechanism **for the day a legacy opener returns** (or for any future
open path that takes a POSIX lock on the database file and does not pass through the mode matrix). Its shape:

```rust
Key  = (dev, ino) read by fs::metadata  // not a path: aliases, symlinks, `./`
Map  = OnceLock<Mutex<HashMap<Key, { Opening, Open(n) }>>>   // ADR-0013: poison ⇒ LockPoisoned refusal
Token= RAII in TursoDb; last drop removes the entry
Gate = one tokio::sync::Mutex<()> per key, held across preflight→build→register
```

Why identity, not pid, not path: measured `T3` (below) shows a *root-scoped* answer loses a row; a path key
would treat `/root/a.db` and `/root/./a.db` as two files and probe destructively into our own handle.

**The trigger, written so it cannot be forgotten:** *the first commit that adds a code path able to produce
`WalMode::Legacy` (or a read-only open that takes an OS lock on the database file) must land the registry in
the same change, with a test that opens the fixture twice without closing the first handle and a test that
runs probe B against our own legacy handle and observes `Held`.* Until then, `UnsupportedOpenMode` is the
enforcement.

## 12. Predicted outcome on the shared measurement, and what refutes it

- **Prediction (registered before the run, pre-amendment):** G1 holds; raw self-conflict reproduces;
  registry clears; on the clearance-evidence arm A refuses and B clears; A cheaper.
- **Refuted by any of:** the unguarded mixed factory open already refused; probe A `Free` against our own
  handle (no self-conflict to solve); A's identity key false-clearing; B *not* clearing; probe B
  non-destructive in-process.
- **MEASURED outcome:** G1 holds; probe A self-conflicts (`Held`); probe B does **not** self-conflict (`Free`)
  and *is* destructive on the own-file case; B clears a foreign sibling holder and loses a row; B is 7 µs
  slower on the common path. **A wins the axis; neither clearance mechanism needs to ship under O3.**
- **Not predicted, discovered:** the POSIX close-releases-the-process's-locks rule (T2 S4/S6/S7), and the
  O3/O2 consequence that probe B is the only probe and needs no clearance at all.

## 13. Acceptance mapping (amended)

| criterion | A |
|---|---|
| 45.1 | §6, including rows 4, 5, 7 the issue omits and the two seam-based legacy tests O3 will refuse |
| 45.2 | guard inside `TursoConnectFactory::connect`; one new test opens through it, no call-site change |
| 45.3 | **superseded by O3** — not implemented; the measurement that justifies deleting it is retained |
| 45.4 | probe B against a real child process: `t2 S3` (refused, engine text names the db file), `T3` (`Held`) |
| 45.5 | post-open probe B, drop-before-return, snapshot equality, G4 sentence |
| 45.6 | passes with no mechanism; the test must exist to pin the vacuity (§3), plus the §11 trigger |
| 45.7 | `OpenBlocked`, `UnsupportedOpenMode`, enum stays 112 (re-measured), no pid |
| 45.8 | **O6**: one integration direction (live legacy holder ⇒ refuse) + the decision-table multiprocess row + the inversion arm (disable probe B ⇒ that control goes red) |
| 45.9 | covered via the seam (§6 row 2); decision 8's queue is out of scope |
| 45.10 | §7; non-Linux/Windows named |
| 45.11 | `libc = { workspace = true }` in `mem-store` only |
| 45.12 | §8 (one probe, measured) |
| 45.13 | §2 table (mode × writability) + recorded read-only divergence |
| 45.14 | marker in the root, `.tshm` untouched and byte-identical (measured), no turso format version read |
| 45.15 | ADR-0025 Grounds (A4) — §14 |

## 14. What ADR-0025 must carry (not written here; issue 43's opsec §9 items 4–5 gate its text)

1. **O2's direction matrix verbatim** and the fact that A3's older "probe A applies to every open" rule was
   superseded *because it would refuse the legitimate second multiprocess opener* — with the measured
   evidence (`t1 probe_a=Held` with a live flag-on holder; cell 4's coherent two-opener result).
2. **O3's construction refusal** and its dependency: `for_config` never consults config for the mode, so the
   day that changes, the registry (§11) ships with it.
3. **A5's marker location/format** + the measured reasons `.tshm` cannot host it.
4. **A4's doc-vs-code contradiction** (docs `multiprocess-access.mdx:139-145`, `:78`, `:148` doc-sourced;
   code `database.rs` line pairs).
5. **The POSIX close-releases-locks finding** and the two rules it forces (probe B one byte via
   `F_OFD_SETLK`; clearance-before-probe whenever a legacy opener exists).
6. **The §3 vacuity proof and its precondition** (seam completeness; no production `turso::Builder`).
7. **O4/O5's override**: name, precedence, the log line, `health`, and the reader test.
8. **The two seam-based legacy tests** O3 will refuse, and their disposition (test-only escape) — 45.1.
