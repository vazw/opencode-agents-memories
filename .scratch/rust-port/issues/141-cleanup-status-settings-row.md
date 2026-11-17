# 141 — Render cleanup status in the Settings maintenance section

Status: ready-for-agent — filed 2026-11-18 by owner decision, so the status value issue 41 exposes is actually used.

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
