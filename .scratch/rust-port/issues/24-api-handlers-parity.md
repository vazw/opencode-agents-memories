# 24 — W6.2 · API handlers parity (US 5.1)

Status: needs-triage

## What to build

Full REST surface parity over `mem-store`/`mem-profile` behind the W6.1 auth/CORS layer, including container-tag traversal protection on all scoped endpoints. Endpoint-group slices can run in parallel. With §0b D5 the REST handlers are the *only* web API surface — no Leptos server functions may shim or bypass them. Wave detail: `.scratch/rust-port/roadmap.md` §W6.2.

## Acceptance criteria

- [ ] Behaviors ported: `web-server-api`, `web-server-stats`, `web-server-sessions` (episodic parts land with W6.3), `web-server-config`, `web-server-export`, `web-server-import`, `web-sessions-container-tag`.
- [ ] Manual smoke per endpoint group against the upstream dist web UI and/or the `web-ui` Leptos screens once W6.5 lands.
- [ ] Traversal attempts rejected at every scoped endpoint.

## Blocked by

- 14 · W3.3 (Phases 1–3 complete)
- 23 · W6.1
