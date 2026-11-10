# 41 — W8.3 · Scheduled retention cleanup (daily tick)

Status: ready-for-human — 2026-11-10 (unblocked by issue 39 completion; awaits open decisions: trigger source, interval, in-memory state, auto_cleanup_enabled default true hazard)

## What to build

Wire the automatic cleanup that upstream runs daily, gated on `auto_cleanup_enabled`. Split out of
issue 39, which implements the **manual** endpoint only.

Upstream (`example/opencode-mem/src/services/cleanup-service.ts`):
`shouldRunCleanup()` returns false when `autoCleanupEnabled` is false, when a run is already in
flight, or when less than 24 h has elapsed since the last run; it is then called by the capture /
startup path rather than by a scheduler.

## Why this is its own wave

1. **It deletes user data on a timer.** A periodic retention job deserves its own review rather than
   riding along with the service implementation.
2. **`auto_cleanup_enabled` defaults to `true`** (`schema.rs:584`), **not `false`** — this item was
   written on the opposite belief and the default was never changed. A tick shipped while the default is
   `true` starts deleting on first boot for anyone who never touched config, which is exactly the hazard
   this issue was split out to avoid. The tick must either flip the default or gate its first run behind
   an explicit opt-in; that choice belongs to this lane, not to 39.
3. There is a house pattern to follow and it is not free: `ConsolidationScheduler` +
   `spawn_consolidation_lifecycle` (`crates/mem-bridge/src/engine.rs`) gives a startup tick plus a
   periodic tick with `structured_stderr` reporting — reuse it rather than inventing a second
   scheduler idiom.

## Measured surface (2026-10-25, HEAD `9f4853b`; verify before planning)

| piece | state |
|---|---|
| cleanup service | landed by issue 39: `MaintenancePort::cleanup()`, `expire_old_prompts`, 5-field `CleanupReport` |
| `auto_cleanup_enabled` | parsed; **default `true`** (`schema.rs:584`); currently read by nothing |
| `auto_cleanup_retention_days` | parsed, default `30.0`; used by the manual run |
| scheduler precedent | `ConsolidationScheduler::run_tick()`, `spawn_consolidation_lifecycle(&engine, scheduler)`, `engine.spawn_tracked(...)` |
| no clean-up state today | there is no `last_cleanup_time` equivalent; upstream keeps it in memory |

## Open decisions (grill before planning)

1. **Trigger source.** Startup tick + interval (the consolidation pattern), or hang it off the
   capture path as upstream does (`shouldRunCleanup` is consulted per capture)? The in-process
   scheduler is the obvious port, but it is a behaviour change nobody asked for.
2. **Interval.** Upstream's 24 h is hardcoded, not config. Add a config key or hardcode?
3. **In-memory last-run state.** Upstream keeps `lastCleanupTime` in the process. A restart resets
   it, so a frequently-restarting host cleans on every boot. Persist a timestamp, or accept it?
4. **Overlap with the manual endpoint.** The `AtomicBool` guard from issue 39 prevents concurrent
   runs; confirm the tick respects the same guard rather than duplicating it.
5. **Observability.** `structured_stderr` line for each tick (shape: counts, duration, skipped) so the
   operator can see a deletion job ran — and a `busy` path when the guard refuses.

## Acceptance criteria (draft — finalise after the grill)

- [ ] With `auto_cleanup_enabled = true`, one tick runs at startup and then no more often than the
      configured interval; with `false`, no tick ever runs.
- [ ] A tick that coincides with a manual `POST /api/cleanup` is skipped, not run concurrently, and
      the skip is observable.
- [ ] The last-run timestamp's restart behaviour is as decided and tested.
- [ ] A failing tick does not abort the engine and emits one structured line.
- [ ] No new dependencies; production panic-free.

## Comments

- Split out of issue 39 on 2026-10-25 by owner decision: ship the manual endpoint first, so a timer would
  not be the first thing that deletes memories in the port. **Correction (2026-10-27):** the split
  rationale assumed the default had been flipped to `false`; it is `true` (`schema.rs:584`), so the hazard
  is unmitigated rather than avoided.

- **2026-10-27 — triaged: premise stale, nothing to schedule.** `POST /api/cleanup` is still the documented
  501 stub and `MaintenancePort::cleanup` still returns `Unavailable`; issue 39 is paused and issue 43 blocks
  it. The config this issue describes is also wrong: `auto_cleanup_enabled` defaults to **true**, not false. A
  tick now would call a stub and read a key whose default contradicts the design. Keep behind 39.

- **2026-10-27 — re-verified at `8023dd8` (premise holds; rationale inverted).** The 501 is real
  (`maintenance.rs:235` → `envelope::not_implemented(CLEANUP_UNAVAILABLE)`), the port default is
  `Err(PortError::Unavailable)` (`ports.rs:574`), and issue 39 is still `PAUSED` behind 43 — there is
  nothing for a tick to call. But this file's own config claim is **falsified in three places** (`:19`,
  `:32`, `:63`): `auto_cleanup_enabled` defaults to **`true`** (`schema.rs:584`), not `false`. The split
  from 39 assumed the default had been changed; it never was. So the hazard the split was meant to avoid
  is still live: when a tick lands, a fresh install with an untouched config deletes on first boot. The
  scheduler precedent also still exists (`ConsolidationScheduler` `engine.rs:1255`, `run_tick` `:1311`,
  `spawn_consolidation_lifecycle` `:1381`). Recommendation: `paused` behind 39 → 43, with the default
  corrected in the body before any planner sees it. **State not changed at triage time.**
