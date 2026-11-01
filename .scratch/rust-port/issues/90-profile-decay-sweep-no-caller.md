# 90 — `sweep_decay_daily` has no production caller: profile decay never runs

Status: ready-for-agent — 2026-11-01 (premise MEASURED by the `opsec` audit §3 (3.2) and re-derived.
`sweep_decay_daily` is defined at `crates/mem-profile/src/profile_store.rs:1062`; its only other
occurrences are the two `operation: "sweep_decay_daily"` strings inside that function (`:1070`, `:1118`),
a unit test at `:1595`, and three call sites in `crates/mem-profile/tests/user_profile_decay.rs:253,317,323`
— no caller outside tests. Work will add a lifecycle call site beside the consolidation scheduler in
`crates/mem-bridge/src/composition.rs` and prove it with a new repo-root test
`tests/engine_startup_drain.rs`-style row. Filed because issue 14 records this as a carry-over with no
owning issue.)

## What is missing

US 3.2 promises confidence decay and AI cleanup. The sweep exists and is tested; nothing invokes it.

- `sweep_decay_daily` — the daily decay sweep — has no caller outside its own definition and tests.
- `ProfileCleanupHook` is never constructed, and that is **by design, not a defect**: issue 47 is
  `wontfix` on exactly this point ("the learner is deliberately unwired"). Treat the hook as gated on
  issue 47 and do **not** list it as missing work here. Only the sweep is this issue's deliverable.

Practical effect: confidence scores never decay, so stale preferences keep the weight they had when
first observed, and nothing ever ages out. The profile tier renders (119 `profile inject` events on
2026-11-01 as of 11:00Z — a point-in-time count), which makes the absence quieter, not harmless.

## What lands

- A production caller for `sweep_decay_daily` on the engine's lifecycle — the consolidation scheduler is
  the existing precedent for "startup tick + periodic tick" and already carries a documented lifetime
  (`engine.rs`'s `spawn_consolidation_lifecycle`), so a sibling call is the cheapest shape.
- **The handle.** The reachable one is `ComposedRuntime.profile`
  (`crates/mem-bridge/src/composition.rs:51`, `pub profile: Arc<ProfileStore>`). `EngineRuntime`
  (`engine.rs:235-247`) carries `capture`, `inject`, `recall`, `skills` and `tools` and **no profile
  handle** — a call site written against `EngineRuntime` cannot reach the sweep without a new field.
- **The clock.** `sweep_decay_daily(now: i64)` never reads a wall clock (`profile_store.rs:1019-1021`
  states the property, `:1060` restates it) — a sweep is fully reproducible from its inputs. The caller
  therefore owns the clock: the lifecycle must read the wall clock once per tick (the same source the
  capture drain uses for `now_millis()`) and pass it in, and the test must pass a fixed `now` rather than
  sleeping.
- **The failure states that are real** (an earlier revision demanded an "operation lock" the profile
  store does not have — `grep -rn operation_lock crates/mem-profile/src` is empty):
  - a **poisoned** `last_decay_sweep` mutex: the read (`:1064-1071`) and the write-back (`:1113-1119`)
    both map poisoning to `ProfileError::LockPoisoned { lock: "last_decay_sweep", operation:
    "sweep_decay_daily" }`, so the tick must log a named event and leave the engine alive;
  - a **lost optimistic-version CAS**: `update_profile` returns `false` when the row's `version` moved
    under it, and the sweep already counts that instead of retrying inline (`:1094-1110`, the debug line
    at `:1105-1108`). The lifecycle keeps that behaviour — no inline retry, no forced write.

## Acceptance criteria

- [ ] `sweep_decay_daily` is reachable from a production call site (proved by a test that asserts the
      scheduled task invokes it, in the style of the repo-root `tests/engine_startup_drain.rs`).
- [ ] The call cannot delay `hello` — it runs behind the handshake, like the capture replay.
- [ ] A poisoned `last_decay_sweep` on either the read or the write-back path produces a named event and
      leaves the engine running (tested by poisoning the mutex, not by reading the code).
- [ ] A lost optimistic-version CAS is counted and logged, and is not retried inline (tested by moving a
      row's version between the scan and the write).
- [ ] A seeded profile's confidence measurably drops after a tick with a supplied `now`.
- [ ] Issue 14's R13 carry-over line is updated to point here.

## Parent

Slice issue 95 (`95-slice4-profile-decay-runs.md`).

## Comments

**Why it is easy to lose:** issue 14 is marked done and the symbol is implemented and unit-tested, so
nothing looks unfinished. Only a production-caller grep shows the gap — which is precisely the check the
2026-11-01 audit added and which found four other instances of the same shape (issues 63, 83, 84, 86).

**Scope correction of an earlier revision:** it listed `ProfileCleanupHook` next to the sweep as missing
construction, and required a busy operation lock to skip a tick. Neither belongs here: the hook is gated
on issue 47's `wontfix`, and the profile store has no operation lock to contend for.
