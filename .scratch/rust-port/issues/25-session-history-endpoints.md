# 25 — W6.3 · Session-history endpoints — new (US 5.1)

Status: needs-triage

## What to build

New endpoints powering the Leptos/Thaw session-history tab (W6.5): `GET /api/projects/:project_id/sessions` and `DELETE /api/projects/:project_id/sessions/:session_id`, over the W4.2 query layer. This closes the Epic 1 UI side. Wave detail: `.scratch/rust-port/roadmap.md` §W6.3.

## Acceptance criteria

- [ ] Endpoint tests follow the same handler conventions as W6.2 parity handlers.
- [ ] Manual demo: session history renders in the `web-ui` Leptos UI.
- [ ] Delete is orphan-safe (timeline consistency per W4.2 guarantees).

## Blocked by

- 16 · W4.2
- 24 · W6.2
