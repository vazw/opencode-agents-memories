# 22 — W5.4 · Memory tools registration — existing tools + recall (US 1.4)

Status: needs-triage

## What to build

Register all agent-facing memory tools with the host: the ported memory tools plus the new `recall_project_sessions`, with per-tool scope rules and the portability tool path. Wave detail: `.scratch/rust-port/roadmap.md` §W5.4.

## Acceptance criteria

- [ ] Behaviors ported: `tool-scope`, `memory-timeline-orphan` (tool path), `memory-portability-tool`, `profile-tool-runtime`.
- [ ] `recall_project_sessions` registered and callable with the W4.2 schema.
- [ ] Per-tool scope rules enforced (tools cannot read outside their scope).

## Blocked by

- 16 · W4.2
- 19 · W5.1
