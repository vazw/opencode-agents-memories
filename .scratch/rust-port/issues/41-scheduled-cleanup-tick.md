# 41 — W8.3 · Scheduled retention cleanup (capture-triggered tick)

Status: ready-for-agent — 2026-11-18. All open decisions settled in the grill of 2026-11-18 (below); endpoint `GET /api/cleanup/status` ships here, the Settings row that renders it is issue 141.

## What to build

Wire the automatic cleanup that upstream runs, gated on `auto_cleanup_enabled`. Split out of issue 39, which implements the **manual** endpoint only.

Upstream (`example/opencode-mem/src/services/cleanup-service.ts`) consults `shouldRunCleanup()` from the session-idle timer (`index.ts:1313`) and runs `runCleanup()` inline; `handleRunCleanup` calls `runCleanup()` directly, so **`autoCleanupEnabled` gates only the automatic path** — our manual endpoint already matches this and must keep doing so.

## Settled decisions (grill, 2026-11-18)

1. **`auto_cleanup_enabled` default flips to `false`.** Upstream defaults `true`; ours would fire at engine boot on an unattended host, which is not upstream's context (upstream only ever runs inside a live OpenCode host on session idle). Explicit opt-in is required. This deviates from upstream's default and the deviation is recorded.
2. **Trigger is the capture path.** The wrapper already turns `session.idle` into a `capture` stdio request (`plugin/src/plugin.js:1463` → `:1104`; dispatched at `crates/mem-bridge/src/engine.rs:350`), so the engine receives an idle signal per turn and needs no scheduler, no timer, and no plugin change. No `isServerOwner` equivalent is needed: `engine.rs:1521`'s exclusive lifetime lock makes a single engine per storage root structural.
3. **Interval is a new config key**, `autoCleanupInterval`, a **unit string**: `"30m"`, `"1h"`, `"2h30m"`, `"90s"`. Default `"24h"`. Converted to `u64` seconds in the normalizer. Upstream hardcodes 24 h; this is a deliberate divergence.
4. **Key shape** mirrors the retention sibling: `Option<String>` in `FileConfig`, resolved through a new normalizer that rejects anything not matching the unit grammar with `ConfigError::InvalidValue { key: "autoCleanupInterval" }`. Added to `PROJECT_DROPPED_FIELDS` (cleanup is storage-wide; a project file must not narrow it), to the shipped example, and to the `config_example.rs` mirror.
5. **Last-run stamp is persisted**, in a `cleanup-sweep.json` sidecar beside the storage root, reusing `mem_profile::decay::sweep_is_due` (`crates/mem-profile/src/decay.rs:73`) as the pure gate and `save_stamp`'s best-effort posture (`crates/mem-profile/src/profile_store.rs:1129-1140`): a failed write logs and the run still counts. Precedent: issue 102's `decay-sweep.json`.
6. **Seam is a notify channel.** `mem-web` and `mem-bridge` do not depend on each other, so the capture path cannot reach `MaintenancePort`. The composition root (`src/main.rs`) keeps an `Arc` clone of the maintenance port it already builds, spawns one task awaiting a `tokio::sync::Notify`, and the capture handler calls `notify_one()` after a successful capture. Coalescing is harmless.
7. **The sweep is unbounded but measured first.** The blueprint must record one real sweep's duration and counts on this store before approval; a per-run shard bound with a cursor is only added against a measured number.
8. **Collision: first to acquire the guard wins.** The issue-39 `AtomicBool` decides; the loser is observably skipped.
9. **Logs: silent when not due.** Exactly one `cleanup.tick` line per completed run with counts and `duration_ms`; `cleanup.tick_failed` with a closed-set kind; `cleanup.busy` on a guard skip. A disabled config and a not-due check emit nothing — the shape `run_decay_tick` already uses (`engine.rs:2093`).
10. **`GET /api/cleanup/status` ships here**; the Settings row that renders it is **issue 141**.

## Measured surface (2026-11-10, re-verified 2026-11-18 at HEAD `8eb9b5a`)

| piece | state |
|---|---|
| cleanup service | landed by issue 39 (`Status: done — 2026-11-10`): `MaintenancePort::cleanup()`, `expire_old_prompts`, 5-field `CleanupReport` |
| delete predicate | `RETENTION_CANDIDATES_SELECT` (`crates/mem-store/src/shard_retention_sql.rs:12`): `WHERE updated_at < ?1` — **idle 30 days**, pinned skipped. Not "created 30 days ago" |
| `auto_cleanup_enabled` | parsed and resolved, **default `true`** (`schema.rs:702`); read by nothing |
| `auto_cleanup_retention_days` | parsed, default `30.0`, injected into the port at `src/main.rs:237` |
| guard | `cleanup_running: Arc<AtomicBool>` + `compare_exchange` (`mem-web/src/api/deduplication/mod.rs:426,525`) |
| scheduler precedents | `ConsolidationScheduler` (`engine.rs:1907`); `spawn_profile_decay_lifecycle` (`engine.rs:2128`) with its persisted 24 h gate |
| capture seam | `engine.rs:350` `"capture" => self.call_capture(request)` |
| no cleanup state today | nothing persists or tracks a last cleanup run |

## Acceptance criteria

- [ ] `DEFAULTS.auto_cleanup_enabled` is `false`, the shipped `opencode-mem.example.jsonc` shows `autoCleanupEnabled: false`, and `crates/mem-core/tests/config_defaults.rs` asserts it. The divergence from upstream's `true` is recorded with its reason.
- [ ] `autoCleanupInterval` accepts `"90s"`, `"30m"`, `"1h"`, `"2h30m"` and converts to the correct second count; any other string fails the load with `InvalidValue { key: "autoCleanupInterval" }`; a project config file cannot set it; the key appears in the shipped example and its mirror.
- [ ] With cleanup enabled and the stamp due, a **successful `capture` request** leads to exactly one cleanup run; a second capture inside the interval leads to none. With cleanup disabled, no capture ever triggers a run — while `POST /api/cleanup` still runs.
- [ ] Restart inside the interval produces no run; a restart after the interval produces exactly one. A missing or corrupt `cleanup-sweep.json` is treated as due.
- [ ] A capture-triggered run colliding with a manual `POST /api/cleanup` results in exactly one execution and an observable skip for the other.
- [ ] A failing tick emits exactly one `cleanup.tick_failed` line, never aborts the engine, and leaves the stamp retryable on the next due check.
- [ ] **Capture latency is measured both ways and gated**: the blueprint and the evidence README carry before/after capture-response latency with cleanup due versus not due, produced by a named instrument (a scripted capture burst over the stdio protocol, not an estimate). A regression is a defect, not a note.
- [ ] `GET /api/cleanup/status` returns `{enabled, intervalSeconds, lastRunMs|null, running}`, is token-gated like every other `/api` route, and has a contract test in the Rust suite. No UI change in this issue.
- [ ] No new dependencies; production panic-free (no `unwrap`/`expect`); issue 39's suite stays green; targeted test runs only.

## Blocked by

Nothing. Issue 39 is done; 43 is closed. The Settings row that consumes the status endpoint is issue 141.

## Comments

- Split out of issue 39 on 2026-10-25 by owner decision: ship the manual endpoint first, so a timer would not be the first thing that deletes memories in the port. **Correction (2026-10-27):** the split rationale assumed the default had been flipped to `false`; it is `true` (`schema.rs:584`), so the hazard was unmitigated rather than avoided.

- **2026-10-27 — triaged: premise stale, nothing to schedule.** `POST /api/cleanup` was the 501 stub and `MaintenancePort::cleanup` returned `Unavailable`; issue 39 was paused behind 43. Kept behind 39.

- **2026-10-27 — re-verified at `8023dd8`.** The 501 was real (`maintenance.rs:235`), the port default `Err(PortError::Unavailable)`. This file's own config claim was falsified in three places: `auto_cleanup_enabled` defaults to `true`, not `false`.

- **2026-11-18 — grilled to closure (owner, one question at a time).** Ten decisions settled, listed above. Two facts reshaped the design: upstream's trigger is the session-idle event, which our capture path **already carries** (`plugin.js:1463` → `engine.rs:350`), so no scheduler or endpoint is needed; and `updated_at < cutoff` means the sweep deletes *idle* memories, so a boot-time trigger on an untouched config would delete without any user action. The default therefore flips, and the trigger becomes activity-bound. Owner additionally asked for the status value to be *used*, which is why the endpoint ships here and 141 renders it.
