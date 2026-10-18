# 21 — W5.3 · Session capture & context injection end-to-end (US 1.1, 1.2, 1.3)

Status: needs-triage

## What to build

The Epic 1 headline capability: capture structured session summaries at turn/session end (via the host events from W5.1) and inject `<project_agent_session_history>` context at session start, within token budgets, blocker-first ordering. Child-session/agent attribution per W4.1 scoping. Wave detail: `.scratch/rust-port/roadmap.md` §W5.3.

## Acceptance criteria

- [ ] Host-driven end-to-end test: capture → persist → inject on next session start.
- [ ] Behaviors ported: `auto-capture`, `auto-capture-concurrency`, `memory-context` (token-budget trimming order matches upstream).
- [ ] Injection respects the token budget with blocker-first priority; memory-count accounting verified.
- [ ] Child-session/agent attribution flows through capture (integrates W4.1 + W4.2).

## Blocked by

- 13 · W3.2
- 15 · W4.1
- 16 · W4.2
- 19 · W5.1
