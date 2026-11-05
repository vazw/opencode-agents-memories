# 95 — Slice 4: profile confidence decay actually runs

Sequencing parent — owns no work. Children: 90.

Status: done — 2026-11-04 (SUPERSEDED — child 90 fixed 39dadf3)
no deliverable of its own. Agents should grab the child — issue 90. Do not open a wave against this file.

## What to build

The decay sweep exists, is tested, and is never invoked. Confidence scores therefore never decay, so a
preference observed once keeps full weight forever and nothing ages out. The profile tier renders on a
real path (119 `profile inject` events on 2026-11-01, a point-in-time count), which makes the absence
quiet rather than harmless.

Issue 90 owns the production caller, its clock (the sweep never reads one — the caller supplies `now`),
the reachable `ComposedRuntime.profile` handle, and the two real failure states (a poisoned
`last_decay_sweep`, a lost optimistic-version CAS). This file does not restate them.

## Acceptance criteria

- [ ] Close when its child closes (issue 90). No acceptance criterion is restated here.

## Blocked by

None — can start immediately. Independent of the capture waves.

## Evidence

Issue 90. Audit: `.tmp/progress/opsec-audit-20261101T090956Z.md` §3 (US 3.2) — `sweep_decay_daily` is
defined at `crates/mem-profile/src/profile_store.rs:1062`, referenced only by its own
`operation: "sweep_decay_daily"` strings (`:1070`, `:1118`) and a unit test (`:1595`), and called only from
`crates/mem-profile/tests/user_profile_decay.rs:253,317,323`; issue 14's R13 carry-over records it as
unowned.
