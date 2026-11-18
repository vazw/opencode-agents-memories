# 141 — Render cleanup status in the Settings maintenance section

Status: done — 2026-11-18. Filed by owner decision so the status value issue 41 exposes is actually used.

*(Parent: issue 41.)*

## Parent

41 — capture-triggered retention cleanup. It ships `GET /api/cleanup/status` returning
`{enabled, intervalSeconds, lastRunMs|null, running}` and deliberately ships no UI.

## What to build

Show that status beside the existing cleanup action in the Settings maintenance section, so an operator can see
whether automatic cleanup is on, how often it runs, when it last ran, and whether a run is in flight.

The surface already exists and must be extended, not duplicated:

- `cleanup_spec()` (`web-ui/src/core_screens.rs:2481`) — the `POST /api/cleanup` request.
- `cleanup_outcome_view` and the `cleanup_next` reducer (`web-ui/src/confirm.rs:8`) — the maintenance action is
  never swapped out, only disabled while a run is in flight (`confirm.rs:330`).

A run that finishes must refresh the status (the `lastRunMs` and `running` fields both move), so the refresh path
is part of the deliverable rather than a follow-up.

## Acceptance criteria

- [ ] The Settings maintenance section displays enabled / interval / last run / running, read from
      `GET /api/cleanup/status`; a `null` `lastRunMs` reads as "never" rather than as an empty cell or a zero date.
- [ ] The view re-reads the status after a cleanup run completes, and while `running` is true the action is
      disabled rather than merely labelled.
- [ ] An unavailable status endpoint (a degraded composition answering `Unavailable`) renders as a stated
      "not available", never as `enabled: false` — a silent zero is a defect.
- [ ] The class census moves with the markup: `node web-ui/scripts/check-class-coverage.mjs --self-test
      --min-emitted=191` stays green with `dead=0 unresolved=0`, and the emitted floor moves in the same commit
      if the new markup really needs new classes.
- [ ] A CDP assertion in the house lane proves the four values render, and one assertion proves the never-ran case.
- [ ] No new dependency, no `Icon`, no `icondata` crate; the three native `<select>` testids are untouched.

## Blocked by

- 41 — supplies the endpoint. Also sequence against 140, which rewrites markup in `core_screens.rs`: whichever of
  the two lands first, the other rebases against it rather than editing a stale tree.

## Comments

**Closed 2026-11-18 at commit `1fc1f8a`.** Blueprint `blueprint-20261118-2207-issue-141-cleanup-status-row`
(`sha256=c642072032df`), SYS-APPROVED 2026-11-18T05:35Z. Evidence: `docs/evidence/issue-141/README.md`.
Three review rounds: round 1 rejected on four non-behavioural blockers, round 2 on five Step-7 deliverables, round 3
approved after proving the code byte-identical to the round-2 snapshot (`diff -r` empty except moved logs,
`git diff --numstat` still `+416/−23`, a fresh `trunk build` reproducing `web-ui-bc64730d57082f3d_bg.wasm`), so the
round-2 gate runs stand and the lane was not re-run.

Behaviour, from the reviewer's own runs: lane **RED 6/6** on the unwired bundle and **GREEN 24/24** after wiring with
the pre-existing 13+5 cases intact; wasm suite 21 binaries / 20 `test result: ok`; `fmt` exit 0; both clippy gates
exit 0; class coverage `emitted=192 dead=0 unresolved=0`. `EXIT=3` on the census is permitted by blueprint Step 7
(`violations=0 unclassified=0`, exit may stay 3 for ruling-side stale rows); the 13 decomposes as **1 probe staleness**
(`d58a94f ≠ 6c80902`) plus **12 pre-existing ruling-side findings** (6 stale kinds, 6 worklist overstatements),
identical at base `abf2bce` before a line of this issue's code existed. `stale` and `drift` are the same counter.

AC coverage: AC1 endpoint row with the pinned set 37→38; AC2 the `CleanupAutoStatus` classifier with `Unavailable`
and "said disabled" as arms that cannot collapse; AC3 `cleanup_last_run_label` rendering a null `lastRunMs` as "never" rather
than a zero date; AC4 the refetch proven by removing `refresh_status.run(())` at `core_screens.rs:2940` and watching the
case red; AC5 `cleanup_disabled(state, auto_running)` total and pure, with `running: true` disabling the action while
degradation leaves it enabled; AC6 mount, Reload and post-run fetches only — no timer, no interval, no polling.

Three findings recorded rather than fixed:

- `SETTINGS_PATHS` briefly gained `/api/cleanup/status` and was **reverted**, not pinned: the fetch builds its URL
  from `endpoint(EndpointId::CleanupStatus).template` (`core_screens.rs:2493`), so the set never carries it. A pin
  there would have been decoration.
- `wasm-bindgen-cli` was reinstalled to **0.2.129** to match `Cargo.lock`; the pinned 0.2.100 runner failed every wasm
  test binary on a schema mismatch. A machine-level repair, recorded so nobody reverts it.
- The Step-0 ledger path diverged from the blueprint's to the wave spawn's `LEDGER:` path.
