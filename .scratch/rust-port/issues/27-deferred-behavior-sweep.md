# 27 — W7.1 · Deferred behavior sweep — auto-update, timeouts, i18n, runtime-require

Status: needs-triage

## What to build

Port the deferred behaviors from earlier phases: auto-update service, request-timeout wiring, i18n translations, runtime-require/diagnostics parity. Various phases deferred these to keep waves focused; this wave clears the tail. Wave detail: `.scratch/rust-port/roadmap.md` §W7.1.

## Acceptance criteria

- [ ] Behaviors ported: `auto-update-check`, `auto-update`, `request-timeouts`, `i18n`, `runtime-require`.
- [ ] No behavior in the deferred list is left without a disposition (ported or documented N-A with rationale).

## Blocked by

- 19–26 (Phases 5–6 complete)
