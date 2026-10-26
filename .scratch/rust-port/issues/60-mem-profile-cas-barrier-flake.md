# 60 — `mem-profile` · `test_exactly_one_competitor_cas_reclaims_dead_owner` times out intermittently, in isolation

Status: ready-for-agent — 2026-10-27 (diagnosis first; the test is pre-existing and its failure is intermittent at ~1/6 in isolation; brief under `## Agent Brief`)
`barrier hs.seen.b never appeared` after ~30 s, including when run alone.** It surfaced twice in one day
(a workspace run and a reviewer's isolation run) and passes otherwise, so it is a flake — but an
unexplained one, in a test that exists to prove a lock property.

## PREMISE — measured, not inherited

- **Workspace run, issue-56 wave:** `cargo test --workspace` (no `CARGO_TARGET_DIR`) → this test failed after
  **30.08 s** with `barrier hs.seen.b never appeared`. The same wave's absolute-target run
  (`CARGO_TARGET_DIR=$PWD/target`, 231 `test result: ok`) passed.
- **Isolation runs, reviewer, same day:** six consecutive
  `cargo test -p mem-profile --test profile_learning_lock test_exactly_one_competitor_cas_reclaims_dead_owner -- --exact`
  runs → **run 1 failed after 30.01 s** with the same message; **runs 2–6 passed in 0.04–0.05 s**.
  So 1/6 in isolation — the first pass's "passes in isolation" claim was one lucky observation.
- **Whole file in isolation** passes (`15 passed, 0.21 s`) when the barrier does not stall.
- **Not this wave's:** `git diff --stat 774ed0f..HEAD -- crates/mem-profile` is empty; the test was added
  by `4548562` (2026-10-20), the only commit that ever touched
  `crates/mem-profile/tests/profile_learning_lock.rs` (verified with `git log --all`).
- The barrier timeout constant is `BARRIER_TIMEOUT = 30 s` (`profile_learning_lock.rs:179`), and the failure
  is the *second* participant never arriving at `hs.seen.b`.

## WHY IT MATTERS

The test exists to prove that exactly one competitor CAS-reclaims a dead owner's lease — i.e. that the
double-reclaim race is closed. A 30 s barrier timeout means one of the two threads never reached the
barrier: either the test's own choreography loses a wake-up, or the code under test blocks somewhere it
should not. Today the test reports neither, just a timeout.

Two consequences:
1. **`cargo test --workspace` is not reliably green** on this tree — a reviewer or CI sees a red suite with
   no defect behind it, which is the same "conditionally green" problem as issues 56/59.
2. **The property may be under-tested**: if the second thread sometimes never starts, the CAS assertion may
   be passing vacuously on the runs where it does.

## WHAT A WAVE SHOULD PRODUCE

1. The actual reason the second participant misses the barrier — thread start failure, a lock held across
   the barrier, a lost notify, or a scheduler-dependent ordering. Evidence, not a guess.
2. A test that cannot silently degrade: if the barrier is a coordination device, its timeout must say which
   side failed and why, not just `hs.seen.b never appeared`.
3. **Do not raise `BARRIER_TIMEOUT` to make it green.** A 30 s budget that is exceeded intermittently in
   isolation is a signal, and widening it hides the signal. If the diagnosis shows the timeout is genuinely
   load-sensitive and the property is still proven, that must be measured across many runs before the
   constant moves.

## ACCEPTANCE

- A recorded diagnosis of which participant fails to arrive and at which point.
- `cargo test -p mem-profile --test profile_learning_lock` green across **at least 20 consecutive isolation
  runs** and in a workspace battery, with the count recorded.
- The CAS property still asserted — a fix that makes the test pass by weakening what it proves is a
  rejection.

## Comments

- **2026-10-27** — filed from the issue-56 review after the failure appeared twice. Evidence:
  the coder's workspace log (30.08 s) and the reviewer's six isolation runs (1 fail at 30.01 s, five passes
  at 0.04–0.05 s). No code in that wave touches `crates/mem-profile`.

## Agent Brief

**Category:** bug
**Summary:** a lock-correctness test times out intermittently — including when run alone — and nobody knows which
participant fails to arrive.

**Current behavior (verified):**
`test_exactly_one_competitor_cas_reclaims_dead_owner` in the profile learning-lock suite fails with
`barrier hs.seen.b never appeared` after roughly 30 seconds. It has failed twice in one day: once under a
concurrent workspace run, and once in a six-run isolation series (run 1 of 6, 30.01 s; runs 2–6 pass in
0.04–0.05 s). The whole file passes in isolation when the barrier does not stall. The test is pre-existing —
the only commit that ever touched its file is the one that added it — and the barrier timeout is a 30-second
constant.

**Desired behavior:**
A recorded diagnosis of **which participant fails to arrive at the barrier and where it blocks** — thread start
failure, a lock held across the barrier, a lost notify, or a scheduler-dependent ordering. Then a test that
cannot silently degrade: if the barrier is a coordination device, its timeout must say which side failed and
why, not just that the second participant never appeared.

**Key interfaces:**
- The barrier rendezvous in the test — the thing that stalls.
- The lock state type the test exercises (owner, timestamp, operation) and the CAS reclaim path.
- `BARRIER_TIMEOUT` — a signal, not a knob to widen.

**Acceptance criteria:**
- [ ] A recorded diagnosis naming the failing participant and the point at which it blocks.
- [ ] The suite is green across **at least 20 consecutive isolation runs** and in a workspace battery, with the
      count recorded.
- [ ] The CAS property is still asserted: exactly one competitor reclaims the dead owner's lease.
- [ ] The failure message names the side that did not arrive.

**Out of scope:**
- Raising the barrier timeout to make the suite green — a 30-second budget exceeded intermittently in
  isolation is the signal, and widening it hides it.
- Weakening the assertion, or turning the race test into a sequential one.
- Any other flaky or failing test in the workspace.
