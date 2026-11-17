# 41 — W8.3 · Scheduled retention cleanup (capture-triggered tick)

Status: done — 2026-11-18 (commit `abf2bce`) — capture-triggered retention tick and `GET /api/cleanup/status` shipped; the Settings row that renders the status is issue 141.

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

- [x] `DEFAULTS.auto_cleanup_enabled` is `false`, the shipped `opencode-mem.example.jsonc` shows `autoCleanupEnabled: false`, and `crates/mem-core/tests/config_defaults.rs` asserts it. The divergence from upstream's `true` is recorded with its reason.
- [x] `autoCleanupInterval` accepts `"90s"`, `"30m"`, `"1h"`, `"2h30m"` and converts to the correct second count; any other string fails the load with `InvalidValue { key: "autoCleanupInterval" }`; a project config file cannot set it; the key appears in the shipped example and its mirror.
- [x] With cleanup enabled and the stamp due, a **successful `capture` request** leads to exactly one cleanup run; a second capture inside the interval leads to none. With cleanup disabled, no capture ever triggers a run — while `POST /api/cleanup` still runs.
- [x] Restart inside the interval produces no run; a restart after the interval produces exactly one. A missing or corrupt `cleanup-sweep.json` is treated as due.
- [x] A capture-triggered run colliding with a manual `POST /api/cleanup` results in exactly one execution and an observable skip for the other.
- [x] A failing tick emits exactly one `cleanup.tick_failed` line, never aborts the engine, and leaves the stamp retryable on the next due check.
- [x] **Capture latency is measured both ways and gated**: the blueprint and the evidence README carry before/after capture-response latency with cleanup due versus not due, produced by a named instrument (a scripted capture burst over the stdio protocol, not an estimate). A regression is a defect, not a note.
- [x] `GET /api/cleanup/status` returns `{enabled, intervalSeconds, lastRunMs|null, running}`, is token-gated like every other `/api` route, and has a contract test in the Rust suite. No UI change in this issue.
- [x] No new dependencies; production panic-free (no `unwrap`/`expect`); issue 39's suite stays green; targeted test runs only.

## Blocked by

Nothing. Issue 39 is done; 43 is closed. The Settings row that consumes the status endpoint is issue 141.

## Comments

- Split out of issue 39 on 2026-10-25 by owner decision: ship the manual endpoint first, so a timer would not be the first thing that deletes memories in the port. **Correction (2026-10-27):** the split rationale assumed the default had been flipped to `false`; it is `true` (`schema.rs:584`), so the hazard was unmitigated rather than avoided.

- **2026-10-27 — triaged: premise stale, nothing to schedule.** `POST /api/cleanup` was the 501 stub and `MaintenancePort::cleanup` returned `Unavailable`; issue 39 was paused behind 43. Kept behind 39.

- **2026-10-27 — re-verified at `8023dd8`.** The 501 was real (`maintenance.rs:235`), the port default `Err(PortError::Unavailable)`. This file's own config claim was falsified in three places: `auto_cleanup_enabled` defaults to `true`, not `false`.

- **2026-11-18 — grilled to closure (owner, one question at a time).** Ten decisions settled, listed above. Two facts reshaped the design: upstream's trigger is the session-idle event, which our capture path **already carries** (`plugin.js:1463` → `engine.rs:350`), so no scheduler or endpoint is needed; and `updated_at < cutoff` means the sweep deletes *idle* memories, so a boot-time trigger on an untouched config would delete without any user action. The default therefore flips, and the trigger becomes activity-bound. Owner additionally asked for the status value to be *used*, which is why the endpoint ships here and 141 renders it.

- **2026-11-18 — CLOSED at `abf2bce`.** Blueprint `20261117-1926-cleanup-tick-r6` (`sha256=2206bf07c276`), SYS-APPROVED 2026-11-17T19:30Z after six gate rounds. Evidence: `docs/evidence/issue-41-cleanup-tick/README.md`. Five review rounds and five reworks, all of them about the instrument rather than the behaviour: review 2 said the code was sound and rejected on evidence, and reviews 3–5 rejected gate integrity and prose while every rework left production code untouched.

  **The final instrument.** The sweep yields once per shard and once per delete chunk, and both are pinned by **poll counts**, not wall clock: `test_a_sweep_with_nothing_to_delete_still_yields_once_per_shard` (`polls_in_sweep` 36 → 0 on the no-delete path) and `test_a_sweep_that_deletes_polls_once_per_shard_and_chunk` (`polls_in_sweep=1 < 2` when the chunk yield goes). The two guards are independent — each reds only its own shape. `GAP_RATIO` and `GAP_CEILING_MS` survive as printed evidence only, labelled `evidence(not asserted)` in the test's own output, because a wall-clock bound here measures CPU work rather than scheduling: correct code reads `18–97 ms` per 36-shard body with `run_delay_ms=0.00` on tmpfs, so the old ratio gate red 4 of 47 correct runs while `polls_in_sweep` stayed 108/108. The shipped gate ran **0 false reds in 76 runs at loadavg 1.02–9.16**.

  A 100 ms ceiling was proposed by the debugger and **refused by the executor with a measurement** (`max_gap 185.97` at loadavg 8.67 with polls 108) — upheld independently by review 4 (`137.80 > 100` at loadavg 9.16, polls 41) and unreachable as a discriminator in any case, since no ceiling that clears correct code's `185.97` separates the release zero-yield mutant at `211.75`. It is the second time the executor disputed an unsound gate rather than following it, and both times the dispute was correct.

  **Gates on the reviewer's own runs**: gap suite 6/6 both profiles with `Compiling mem-store`, `retention_sweep` 11, `cleanup_tick` 8, `cleanup_status` 6, `retention` 12, `api_endpoint_contract` 20, `api_route_auth` 8, both clippy gates 0, `fmt --all --check` 0, `cargo audit` 0 with 2 allowed, security gate clean across 10 mandates. `grep -c 'yield_now().await'` = 2, `shard_manager.rs` sha `a55cd901…`.

  **Six gate-integrity failures were found and fixed across this wave**, each by measurement rather than argument: a `truncate(len-1)` that discarded the one sample spanning the sweep, letting a blocking 173 ms body pass at 0.06 ms; a pre-truncation number that withdrew a correct step; an idle-jitter threshold 0.9 ms wide that red correct code 6/6 and green the mutant; an evidence table asserting G2/G3 verdicts for runs that panicked before reaching them; three mutant attempts that measured stale binaries (`Finished in 0.15 s`, no `Compiling mem-store`) and reported false greens; and a release ratio gate that red 3 of 7 correct runs at loadavg 1.10 plus a comment number with no provenance. The stale-binary pattern is recorded as `bug-pattern` `mem_1794948258564_05df8013a`: a mutant row without a `Compiling <crate>` line is unrun.
