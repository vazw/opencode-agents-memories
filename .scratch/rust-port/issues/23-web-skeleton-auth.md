# 23 — W6.1 · Axum skeleton: health, CORS & auth (US 5.1)

Status: needs-triage

## What to build

`mem-web` bootstrap: `axum` server skeleton with `/health`, CORS, web auth and API auth, plus **static hosting of the `web-ui` SPA bundle (`tower-http` ServeDir over `web-ui/dist`, `index.html` fallback for client-routed paths) mirroring upstream `web-server.ts` static+fallback behavior** (§0b D5, W6.1). Reorderable: can be pulled forward right after Phase 1 (only needs W0.2) if parallelism is wanted. Wave detail: `.scratch/rust-port/roadmap.md` §W6.1.

## Acceptance criteria

- [ ] Behaviors ported: `web-server-health`, `web-cors`, `web-auth`, `web-api-auth`, `web-request-lifecycle`.
- [ ] Health endpoint shape matches upstream so existing tooling/monitoring works.
- [ ] Auth applies to the API surface (manual smoke test documented).
- [ ] Static hosting: `GET /` serves the `web-ui` shell and unknown client-routed paths fall back to `index.html`.

## Blocked by

- 02 · W0.2
