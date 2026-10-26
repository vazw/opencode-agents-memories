# 27 — W7.1 · Deferred behavior sweep — auto-update, timeouts, i18n, runtime-require

Status: needs-triage — 2026-10-27 (only request timeouts landed; auto-update, i18n and runtime-require need a port-or-N-A product ruling first)

## What to build

Port the deferred behaviors from earlier phases: auto-update service, request-timeout wiring, i18n translations, runtime-require/diagnostics parity. Various phases deferred these to keep waves focused; this wave clears the tail. Wave detail: `.scratch/rust-port/roadmap.md` §W7.1.

## Acceptance criteria

- [ ] Behaviors ported: `auto-update-check`, `auto-update`, `request-timeouts`, `i18n`, `runtime-require`.
- [ ] No behavior in the deferred list is left without a disposition (ported or documented N-A with rationale).

## Blocked by

- 19–26 (Phases 5–6 complete)

- **2026-10-27 — triaged: one of four legs landed.** Request timeouts are real and applied
  (`HTTP_IDLE_TIMEOUT` and the two profile-cleanup timeouts, with a lifecycle test asserting the ordering).
  Auto-update is absent (the config key parses and no code reads it), i18n is explicitly rejected
  (`web-ui/src/core_screens.rs` records "English-only — D8: no i18n layer"), and runtime-require has no shim.
  The blocking question is a product ruling, not implementation: for a Rust binary, is self-update ported or N-A,
  and does the D8 i18n rejection become the disposition of record?
