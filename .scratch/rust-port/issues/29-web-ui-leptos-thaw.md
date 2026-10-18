# 29 — W6.5 · Leptos/Thaw web explorer UI (`web-ui`) (US 5.1)

Status: needs-triage

## What to build

The new `web-ui` package (outside the workspace graph per §0b D5 / ADR-0003): Trunk-based leptos 0.7/thaw 0.4 CSR app — ConfigProvider/Theme shell with light/dark, client routing reproducing the upstream Svelte app's screens (**full screen-set parity**: memories list/search, tags, profile, settings, export/import), the same-origin `fetch` REST client with a single Basic-forwarding `Authorization` seam for auth-configured servers (**credential-free localhost is the primary posture — no login/prompts ship**), and the **session-history/timeline tab** consuming W6.3. No Leptos server functions anywhere — the hand-written axum REST handlers (W6.2) are the only API surface. Wave detail: `.scratch/rust-port/roadmap.md` §W6.5.

## Acceptance criteria

- [ ] `trunk build` produces `web-ui/dist`, which W6.1's static hosting serves (`GET /` loads the shell; unknown client-routed paths fall back to `index.html`).
- [ ] Exact wave pins respected: `leptos = "0.7"` (floor `^0.7.7` via thaw), `thaw = "^0.4.8"` (stable line), `trunk` + `wasm32-unknown-unknown`, no leptos `nightly` features.
- [ ] `wasm-bindgen-test` for REST-client shape; manual smoke per screen against the Rust server.
- [ ] Full parity: every upstream Svelte app screen is reproduced — memories list/search, tags, profile, settings, export/import, and the session-history/timeline tab rendering W6.3 data.
- [ ] No login/prompt UI ships; the fetch seam supports Basic forwarding only when the server is auth-configured.

## Blocked by

- 23 · W6.1 (shell slice)
- 24 · W6.2 (per-screen lanes: memories, tags, profile, settings, export/import)
- 25 · W6.3 (session-history tab lane)

## Comments

- **2026-10-18:** created by the Rev-3 roadmap fold (web-stack ruling, ADR-0003). Tracer-bullet slices and wave text: `.scratch/rust-port/roadmap.md` §W6.5.
