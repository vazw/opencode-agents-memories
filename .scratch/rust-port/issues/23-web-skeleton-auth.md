# 23 — W6.1 · Axum skeleton: health, CORS & auth (US 5.1)

Status: done — 2026-10-20 (reviewer APPROVED: 88/88 mem-web, 1162/0 workspace, security gate 10/10 PASS)

## What to build

`mem-web` bootstrap: `axum` server skeleton with `/health`, CORS, web auth and API auth, plus **static hosting of the `web-ui` SPA bundle (`tower-http` ServeDir over `web-ui/dist`, `index.html` fallback for client-routed paths) mirroring upstream `web-server.ts` static+fallback behavior** (§0b D5, W6.1). Reorderable: can be pulled forward right after Phase 1 (only needs W0.2) if parallelism is wanted. Wave detail: `.scratch/rust-port/roadmap.md` §W6.1.

## Acceptance criteria

- [ ] Behaviors ported: `web-server-health`, `web-cors`, `web-auth`, `web-api-auth`, `web-request-lifecycle`.
- [ ] Health endpoint shape matches upstream so existing tooling/monitoring works.
- [ ] Auth applies to the API surface (manual smoke test documented).
- [ ] Static hosting: `GET /` serves the `web-ui` shell and unknown client-routed paths fall back to `index.html`.

## Blocked by

- 02 · W0.2

## Comments

- **2026-10-20 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261020-1223-w61-axum-skeleton-auth-r2.md` (sha256 `3218f1c6089f`, SYS-APPROVED 2026-10-20T12:36Z): coder → reviewer `STATUS: APPROVED` with **no rework** (independent re-run: `cargo fmt --all --check` 0 diff · `cargo build --workspace` 0 warnings · `cargo clippy --workspace --all-targets -- -D warnings` exit 0 · `cargo test -p mem-web` 88/0 · `cargo test --workspace` 1162/0 across 132 binaries · `cargo audit --no-fetch` 0 vulnerabilities · 10-mandate security gate 10/10 PASS, no violations). Gate round 1 rejected r1 with three blocking defects plus one precision item (socket-seam runtime under `#[tokio::test]` current-thread, `clippy::map_flatten` on the Host-header expression, blank local-token file never regenerable, router helper's default `Host` colliding with the missing-Host test); r2 fixed all four and the gate re-verified them empirically. **D-8 (user choice: Host-header allowlist over documenting DNS-rebinding as accepted risk, `mem_1792497590541_zif48j3vx`) is implemented as the mitigation, not documentation** — gate check 0, before the Origin gate, OPTIONS, Basic auth and the static shell's token injection; the reviewer re-proved the ordering empirically (with the check disabled the same foreign-Host requests yield the CORS body, a Basic 401, a 204 preflight and an API 401 — never the D-8 envelope), re-proved NON-VACUITY in a standalone copy of the crate outside the workspace (7/7 D-8 probes go 200/401/204 without the check), and proved the refusal lands before token injection against a **real** `dist/` (allowed Host → 200 with `window.__OPENCODE_MEM_TOKEN__` injected; foreign Host → fixed 403 with neither token nor marker) and over real TCP. 35 adversarial Host values refused, 11 legitimate forms allowed; the allowlist is derived in `start()` from the host actually bound; `AppState` has no `Default`; a wildcard bind yields `is_enforced() == false` coherently with `assert_network_auth`. Wave ledger: `/tmp/opencode/progress/20261020-1223-w61-axum-skeleton-auth-r2.ledger.md` — 11/11 steps DONE. Deliverable: `mem-web` axum skeleton — 11 source modules (`error`, `settings`, `cors`, `web_auth`, `api_auth`, `host_allowlist`, `lifecycle`, `health`, `static_hosting`, `gate`, `server`), 9 integration-test binaries, one std-only shared test helper, `docs/web-skeleton-smoke.md` (D-1…D-8 divergences + manual smoke recipe), and 5 append-only rows in the `docs/agents/testing.md` registry. Three NON-BLOCKING findings left open for a later polish: (1) `host_allowlist.rs:120` trims the parsed Host value, so a whitespace-padded *allowed* name is accepted where the pinned parse rule would refuse it — no production impact (httparse strips trailing OWS before the app sees it) and no rebinding bypass, 1-line optional hardening; (2) `docs/agents/testing.md:141` states the disconnect-once coverage in prose without the blueprint's literal `test_request_lifecycle_disconnect_once_semantics_covered_by` label — substance present, label differs; (3) no test asserts the preflight `Access-Control-Allow-Headers: Content-Type` header (the code sets it; the AC did not require it).
