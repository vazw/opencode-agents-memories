# 41 — W8.3 · Scheduled retention cleanup (daily tick)

Status: needs-triage — 2026-10-27 (premise stale: `POST /api/cleanup` is still a 501 and `auto_cleanup_enabled` defaults to **true**, not false — there is nothing for a tick to call)

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
2. **`auto_cleanup_enabled` now defaults to `false`** (owner decision, issue 39). A tick that shipped
   in the same wave would have started deleting on first boot for anyone who never touched config.
   The default change and the timer must be reviewed together, and the default is already landed.
3. There is a house pattern to follow and it is not free: `ConsolidationScheduler` +
   `spawn_consolidation_lifecycle` (`crates/mem-bridge/src/engine.rs`) gives a startup tick plus a
   periodic tick with `structured_stderr` reporting — reuse it rather than inventing a second
   scheduler idiom.

## Measured surface (2026-10-25, HEAD `9f4853b`; verify before planning)

| piece | state |
|---|---|
| cleanup service | landed by issue 39: `MaintenancePort::cleanup()`, `expire_old_prompts`, 5-field `CleanupReport` |
| `auto_cleanup_enabled` | parsed; **default `false`** after issue 39; currently read by nothing |
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

- Split out of issue 39 on 2026-10-25 by owner decision: ship the manual endpoint first, because
  `auto_cleanup_enabled` defaults to `false` and a timer would otherwise be the first thing that
  deletes memories in the port.

- **2026-10-27 — triaged: premise stale, nothing to schedule.** `POST /api/cleanup` is still the documented
  501 stub and `MaintenancePort::cleanup` still returns `Unavailable`; issue 39 is paused and issue 43 blocks
  it. The config this issue describes is also wrong: `auto_cleanup_enabled` defaults to **true**, not false. A
  tick now would call a stub and read a key whose default contradicts the design. Keep behind 39.
