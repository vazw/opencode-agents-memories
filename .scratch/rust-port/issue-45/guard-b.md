# Issue 45 — Design **B** (re-derived under O1–O6): the same seam and decision core, with own-process clearance taken from the A5 storage-root marker

Status: architect draft competing with design A. **Authority:** issue 45 at `c18d0b1` — AMENDMENT A1–A5 and
**owner decisions O1–O6** (the decisions win over the older text). Measured evidence:
`.tmp/drafts/20261025-2304-issue-45-measurement.md`; raw logs `.tmp/scratch/issue-45-ab/`.
Labels: `MEASURED` / `READ` / `INFERRED`.

Design B shares design A's interface, its O2/O3 decision core, its probe-B mechanics, its refusals, its
override, its post-open probe and its marker *write* protocol **verbatim**. Only one thing differs:

> *What evidence establishes that the holder a probe would see is this process's own open file description?*
>
> A: an in-memory registry of file identities. **B: the marker file A5 already mandates** — read
> `{pid, start_time, mode}`, and when the live record is ours, clear the probe for this open.

---

## 1. Where B sits under O2/O3

Under the amended matrix the only probe that exists is probe B, and probe B runs only for a **multiprocess
writable** open. A multiprocess handle takes no lock on the database file (`MEASURED`: probe B against a live
flag-on holder returns `Free`), and a legacy handle cannot be produced at the seam (O3). So B's clearance, like
A's, is not needed to satisfy `45.6` today: the requirement is vacuous (§3 of design A, with the proof).

B is therefore evaluated on two questions:

1. **Is it the better mechanism for the day a legacy opener returns?**
2. **Is it cheaper or safer to carry meanwhile?**

Both answers are measured below, and both favour A.

## 2. The marker (B's mechanism)

### 2.1 Record

```
<storage_root>/.wal-mode            (final name fixed by ADR-0025; not decided here)
{"pid":24733,"timestamp":"2026-10-25T23:04:00.000Z","start_time":28848369,"mode":"multiprocess"}
```

- `{pid, start_time}` is exactly `LockRecord`'s evidence (`operation_lock.rs:71-90`), parsed by a
  `ModeRecord { #[serde(flatten)] state: LockState, start_time, mode }` through the **shipped**
  `read_live_lock`-shaped path: start-time token, conservative off-Linux verdict, corrupt/dead reclamation
  (blocker 1, `aa0dcc4`/`c2e128c`, 16 tests). No new liveness code — B's best structural property.
- `mode` is the A5 addition the owner's selectable-WAL intent requires (43:69-82), so the file is not a new
  artifact: it is the mandated one, made load-bearing.

### 2.2 Write protocol

Identical to design A §5: `create_new` + `sync_all`; `AlreadyExists` ⇒ read; live ours ⇒ reuse; live foreign
same mode ⇒ leave; live foreign **different** mode ⇒ `OpenBlocked{Unknown}` for a writable open; dead/corrupt ⇒
unlink best-effort + retry once, else `Unknown` ⇒ refuse (overridable per O4).

### 2.3 Clearance

```rust
fn own_process(record: &ModeRecord) -> Own {
    if live(record) && record.pid == std::process::id()
       && record.start_time == process_start_time(std::process::id()) { Own::Ours } else { Own::NotOurs }
}
```

When `Own::Ours`, `preflight` returns `Clear` without running probe B. Root-granular by construction: the
record names the process that owns the **root**, not the files it holds.

### 2.4 Release

No refcount, no RAII token: pid+start_time make a leftover from an exited process read dead, so the next
opener reclaims it. The engine's clean shutdown removes it best-effort (the `EngineLock::release` shape).

## 3. Cost — MEASURED (debug profile, n=2 000, `t4.log`)

| item | mean |
|---|---|
| marker read + `serde_json` parse | **7.12 µs** (p95 7.22, max 33.5) |
| probe B as a whole | 5.70 µs |
| **B's own-process open** | **≈ 7.1 µs** (read; probe skipped) |
| **B's probing open** | **≈ 12.8 µs** |
| design A, own-process open | 0 µs |
| design A, probing open | ≈ 5.7 µs |

Every open pays the read; design A pays only on the probing path. The marker write is one `create_new` +
`sync_all` per process per root, not measured here (`INFERRED` from `engine_lock::try_create`'s fsync cost).

## 4. Pros (steelmanned)

1. **One artifact, not two.** A5 mandates the marker anyway; B makes it earn its keep instead of shipping a
   write-only file plus an in-memory table. One thing for a reviewer to reason about; one thing to test.
2. **No new liveness code** — the start-time token and the dead/corrupt reclamation are already shipped and
   inversion-tested.
3. **No global mutable state** in the open path: no `static`, no `OnceLock`, no per-key gate.
4. **Carries the live foreign owner's mode.** This is the only mechanism that could ever preserve the
   supported same-mode multiprocess configuration across the seam (§7, variant B2) and detect two legacy
   builds colliding in a root (no `-tshm` exists, so no byte probe can see it). A cannot do either.
5. **No per-file bookkeeping** for the same-process multi-handle paths (`LearningLock` per operation, the
   `encryption_migrator` source+staged pair).

## 5. Cons

1. **Root-granular clearance is unsound for a sibling database — MEASURED, and it loses a row.** With this
   process holding `one.db` (marker ours) and a *foreign* legacy process holding `two.db` in the same root,
   B clears `two.db`, skips probe B, opens flag-on, and the opener's committed row is silently gone while
   `integrity_check` answers `ok` (`t3.log`:
   `[m45] t3 DECISION A=Refuse("legacy-authority") B=Clear differ=true`,
   `[m45] t3 B_path_result before=[1, 2] after=[1, 2] final=[1, 3] integrity=ok lost_row_2=true`).
   This is precisely the failure class decision 7 exists to close, reintroduced one level up.
2. **Clearance correctness depends on a durable write succeeding.** If the first open's marker write fails
   (ENOSPC, ro root, permissions) while the database open succeeds, every later open sees "not ours" and
   probes — including probe B against a legacy handle of ours, which is destructive (measured, design A §3).
   The fallback is an in-memory bit: A's registry with extra steps.
3. **Inherits the `hidepid=2` fail-open** (issue 43's open A3): a live foreign marker can read dead, be
   unlinked, and be replaced with ours ⇒ a *false clearance* for a foreign holder. Accepted for a lock you
   must not steal; for clearance it is data loss. A's registry has no such cell.
4. **Brick risk A does not have**: a corrupt 100-byte marker must refuse writable opens (unknown ⇒ refuse),
   so damaging that file disables every writable open of the root until it is removed. The O4 override is the
   only way out.
5. **The shared artifact gets two jobs.** The marker is the A5 mode truth *and* B's safety input; a bug in
   clearance (or a stray delete) corrupts the signal other builds use to decide legacy-vs-multiprocess
   (43:69-73). A keeps the mode truth monotone (write-only).
6. **Slower on the common path** — every open pays 7.1 µs, including the engine's per-operation
   `LearningLock` opens; A pays 0 there.
7. **Weaker off Linux**: no `/proc` token ⇒ a stale marker with a recycled pid reads live-ours and clears.
   A never asks about a pid.

## 6. Open-path table (45.1) — B's differences from A §6

| # | path | disposition under B |
|---|---|---|
| 4 | `encryption_migrator` source + staged `*.encrypt-*.tmp` in the same root | covered, but B clears **both**: the marker says the root is ours, so no probe runs for the staged file even though nothing of ours holds it. Harmless for a fresh temp path — the same over-broad clearance that fails on row 5's sibling case |
| 5 | `mem-profile` `LearningLock::open_coordination` — per operation, same process | covered by the marker (ours); 45.6 handled without a registry |
| 7 | raw `turso::Builder` tests | not covered (same as A) |

The two seam-based legacy tests O3 will refuse (`turso_capabilities.rs:2550`, `:2840`) are B's problem too —
identical for both designs.

## 7. The variant that would make B interesting — and why it is out of scope

**B2**: when the marker is live and *foreign* but its `mode` equals the requested mode, let the open through
without a probe, because the matrix pins that configuration as coherent and lossless
(`turso_capabilities.rs:4303`: both handles see each other's writes; `final_ids=Ok([1, 3, 100])`,
`integrity=Ok("ok")` — re-measured this wave).

B2 is **not** design B: it contradicts 45.4/45.8's remaining direction as written ("a live legacy holder ⇒
refuse"; B2's relaxation is about the *same-mode* case, but a root marker is a weaker proof than a per-file
one, and it would let a same-mode foreign holder of a *sibling* file through for exactly the reason §5.1
fails). It is recorded as the losing design's best idea: **the only place a mode-carrying marker pays for
itself is a future concurrent-engine design, and that requires an issue amendment, not an architect choice.**

## 8. Predicted outcome on the shared measurement, and what refutes it

- **Prediction (pre-run):** clears the same-process case; G1 holds; probes behave as in A; on the
  clearance-evidence arm B clears a foreign sibling holder that A refuses ⇒ B loses; ~7 µs worse per
  own-process open.
- **Refuted if:** the pid+start_time check turns out to be per-file after all (T3 ties and B's simplicity
  wins); or a design is judged on same-mode preservation (B2 wins, issue needs amending); or B's write
  protocol turns out cheaper than A's lookup+stat in a release build.
- **MEASURED outcome:** the refutation did not occur — B clears, the row is lost, A refuses. **B loses.**

## 9. Acceptance mapping (rows that differ)

| criterion | B |
|---|---|
| 45.6 | passes: marker names the process; no registry — but the requirement is vacuous under O3 either way |
| 45.14 | passes, and more strongly than A: the marker is B's clearance source, so its location is exercised on every open rather than merely written |
| 45.7 / 45.10 / 45.11 / 45.12 / 45.13 | as A |
| 45.1 | §6 — rows 4/5 show the over-broad clearance |
| 45.3 / 45.8 | as A (O3/O6) |

## 10. ADR-0025 obligations B adds

1. If B were taken: clearance is **root-scoped**, and that is only sound while D-39 holds *and* no foreign
   process may hold a sibling database in the root — the second condition is exactly what the mixed-version
   window violates. Record it, do not bury it.
2. Decide whether one 100-byte file should carry both the A5 mode truth and a safety input, or whether
   clearance stays memory-only (A).
3. The `hidepid=2` inheritance, citing issue 43's open A3.
