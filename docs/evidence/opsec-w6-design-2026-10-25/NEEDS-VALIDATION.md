# Needs validation — W6 erasure design

Three hypotheses are source-grounded but blocked by a fact that source cannot settle. None carries a
severity (by contract). Each has an owner-run, **read-only or disposable-root** plan; the agent ran
nothing.

The paste-pack rule applies: these are probes **the operator runs**, on a throwaway copy of the
storage root. Nothing here is a live-data operation, and nothing here was executed by the audit.

---

## 1. `w6/erase/sweep-liveness` — can a "successful sweep" ever happen on a busy engine?

**Why it matters.** Decision 6's promise is conditioned on "the next successful sweep". If sweeps are
starved by the engine's own concurrency, `PendingErase` is permanent and the promise is a
permanently-pending state dressed as a schedule.

**What source establishes.** `VACUUM` returns `Busy` rather than waiting when another connection
holds a write transaction (pinned at `crates/mem-store/tests/turso_capabilities.rs:4499`, `:4546`).
A same-process second open of the same path inherits the first handle's option set, so the sweep
needs the primary handle **closed** (`turso_capabilities.rs:3423`), and the close-then-reopen probe is
hermetic — no concurrent writer exists during it (`:3470`, `:3510`).

**Blockers.**
- Starvation rate depends on runtime concurrency (capture drain, consolidation, web reads).
- No measurement of the close/reopen window under a concurrent writer.
- The design never says whether the sweep joins the existing `db_lane` serialisation point.

**Plan (local, disposable root).** Start the engine against a throwaway root. Drive steady capture
load (or a loop of web reads plus one write every 50 ms). Request two prompt deletes. Record
wall-clock time from request to `Erased` per request, and count `SweepFailed`/`Busy` residuals.
Control: the same with an idle engine. Instrument whether the post-window reopen carries the vacuum
option (the arm-4 technique at `:3256` shows how to detect an inherited option set). Produce: a
distribution of time-to-Erased and the fraction of requests never reaching it within 24 h.

**What would close it.** Either a measured bound (e.g. p99 under N minutes at realistic load) stated
in ADR-0019, or an explicit "sweeps are opportunistic and unbounded; `PendingErase` may persist
indefinitely" plus the manual lever.

---

## 2. `w6/guard/tshm-probe-portability` — does a `mem-store` probe reproduce turso's lock flavour?

**Why it matters.** The guard's oracle is a private dependency internal. If the tool-side probe does
not conflict with the holder's lock, the guard silently reports "free" and every open proceeds. This
is the failure mode that no negative test can catch.

**What source establishes.** The holder takes a 1-byte POSIX record lock at offset 0, shared, for the
mapping's lifetime, with the primitive chosen at compile time (`F_OFD_SETLK` on Linux, `F_SETLK`
elsewhere). `l_pid` is not populated under OFD. A read-only open takes neither the coordination lock
nor the `.db` lock.

**Blockers.**
- No tool-side probe exists or is measured in the repository.
- OFD and process-scoped `fcntl` have different self-conflict semantics; `flock` is a third domain.
- The selectable-WAL self-conflict case has no pinned shape.

**Plan (local, temp dirs).** Two child processes. Child A opens a temp database with
`experimental_multiprocess_wal(true)` and holds it. Child B attempts three probes on byte 0 of the
`-tshm`: (a) `F_OFD_SETLK` write lock, (b) `F_SETLK` write lock, (c) `flock(LOCK_EX|LOCK_NB)`. Record
which conflict. Repeat on macOS if available. Expected matrix to pin: Linux — (a) and (b) conflict,
(c) does not; macOS — (b) conflicts, (a) unavailable. Then add the self-conflict arm: one process
holds a flag-on handle and probes its own file through a fresh descriptor. The test must fail if the
matrix changes after a turso or libc update.

**What would close it.** A pinned positive control in the ADR's own test suite, plus a note that the
oracle is a dependency internal with a version pin and a canary test.

---

## 3. `w6/guard/vacuum-under-legacy-holder` — what does a sweep do to a live legacy peer?

**Why it matters.** The guard's justification inherits the measured ordinary mixed-open result (the
opener's committed row is silently gone). The sweep is a full file rewrite, which is strictly more
invasive. Whether the outcome is peer corruption, row loss, a clean `Busy` refusal, or a clean
rewrite decides whether the fail-open cases are graded "data loss" or "corruption of a live peer" —
and therefore how much ceremony ADR-0025 needs.

**What source establishes.** Only the same-mode refusal is measured. The consultant calls a concurrent
`VACUUM` "the worst outcome in this design" as a judgement, not a measurement
(`.tmp/drafts/20261025-w6-consult-response.md:142`).

**Blockers.**
- No probe runs `VACUUM` while a flag-on child holds the same database.
- The outcome is a runtime property of the engine's split WAL bookkeeping.

**Plan (local, temp dir, disposable).** Parent creates a temp database; child opens it with
`experimental_multiprocess_wal(true)`, commits a seed row, and opens an uncommitted write
transaction. Parent (holding no handle) opens the same file **flag-off with**
`experimental_vacuum(true)` — the fail-open path the guard is meant to make unreachable — and
attempts `VACUUM` under a bounded timer. Record: the `VACUUM` result, the child's read of its own
rows afterwards, the child's ability to commit, the final row set through a fresh handle, and
`integrity_check` on both sides. **Non-vacuity control:** the same sequence with the child flag-off
must have the parent's open refused, so the arm is attributable to the mode difference and not to a
broken fixture.

**What would close it.** One pinned test in `turso_capabilities.rs` (same style as Probe B), and its
result written into ADR-0025's hazard statement instead of the inherited ordinary-open result.
