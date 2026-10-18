# ADR-0003: Web explorer UI stack — Leptos CSR + Thaw over the axum REST API

- **Status:** Accepted (user sign-off on the web-stack amendment draft, 2026-10-18)
- **Date:** 2026-10-18
- **Context source:** roadmap Rev-3 §Phase 6 + §0b D5; searcher web-research digest (2026-10-18); local working examples `~/Code/Personal/Rust/new-siamstr` (CSR) and `~/Code/Personal/Rust/siamstr` (full-stack); upstream `example/opencode-mem/src/services/web-server.ts` + `web/src` (READ-ONLY).

## Context

The port must ship a web explorer (Epic 5.1). Two binding constraints: (a) the upstream REST contract served by `api-handlers.ts`/`web-server.ts` is the port's acceptability oracle — the 110-test suite's `web-server-*`/`api-handlers-*` tests demand stable, hand-written endpoints with exact response shapes and Basic web-auth semantics; (b) the product is an authenticated, localhost-oriented memory explorer — no SEO, no first-paint requirement, single user, private data. The upstream UI is itself a Vite/Svelte-5 CSR SPA statically hosted by the same server process with an `index.html` fallback. Candidate stacks: Leptos full-stack (SSR + hydration + server functions via `leptos_axum`) vs Leptos front-end-only (CSR/WASM consuming the REST API). Dependency reality (2026-10): thaw 0.4.8 (latest stable) pins leptos `^0.7.7`; the leptos 0.8 line pairs only with thaw 0.5.0-beta (prerelease, published 2025-05, in-repo pinned to 0.8.5, unconfirmed against 0.8.22); thaw maintenance is thin; leptos 0.9.0 stable is imminent with no thaw 0.9 plan visible.

## Decision

**Leptos front-end-only (CSR) + Thaw, built with Trunk.** The UI is a standalone `web-ui/` package **excluded from the cargo workspace** (workspace `exclude = ["web-ui"]`; ADR-0001 crates-exactness unchanged — `web-ui` is not a `mem-*` crate) producing `web-ui/dist` that the `mem-web` axum server statically hosts with SPA fallback, replicating upstream's static + `index.html`-fallback pattern. The API surface is exactly the hand-written axum REST handlers; **no Leptos server functions exist in this codebase.** UI→server coupling is same-origin `fetch` only: **credential-free localhost is the primary posture — no login/prompt UI ships**; Basic forwarding stays available at the single `Authorization` fetch seam for auth-configured servers. CORS serves external tooling, not our UI. The UI is not a dependency of anything: no `mem-*` crate may depend on it, and it consumes no `mem-*` code — its contract is HTTP. Version pairing frozen: `leptos = "0.7"` (caret, floor `^0.7.7`) + `thaw = "^0.4.8"` (stable line) + `trunk` + `wasm32-unknown-unknown`, no leptos `nightly` features, stable channel. Do not mix `ssr`/`csr` features (mutually exclusive leptos build graphs).

## Rejected: Leptos full-stack (SSR + hydration + `leptos_axum`)

1. Server functions cannot carry the upstream REST contract (hashed default endpoints since leptos 0.7/0.8; POST+serde_qs semantics ≠ upstream GET/JSON + Basic auth) → plain axum handlers are mandatory either way → full-stack yields a duplicative second API surface and deletes zero work.
2. SSR/hydration buys nothing for a no-SEO, single-user local tool, while adding the documented hydration-bug class (double execution, HTML-validity panics, browser-only-crate gating) and the workspace feature-unification hazard (leptos #4586 — `ssr` leaking into the wasm graph silently breaks client event handlers; upstream's behavioral fix unmerged).
3. Version-wise it forces the thaw 0.5.0-beta prerelease (the leptos-0.8 line has no stable thaw); front-end-only has no axum coupling and can run the all-stable `leptos 0.7 + thaw 0.4.8` table, already proven in a local working project.
4. cargo-leptos workspace metadata (`[[workspace.metadata.leptos]]` bin/lib package split), auto-downloaded dart-sass/Binaryen toolchains, and either a ninth workspace member (ADR-0001 exactness break) or a dual-target `mem-web` crate churn the frozen shape for no product gain.

## Consequences

- `mem-web` stays a pure axum library crate (ADR-0001 row verbatim); its headless reqwest test lane (waves 23–25) is untouched by UI concerns.
- Deployment is still one server artifact: `mem-web` serves `web-ui/dist`; CI/Docker builds `web-ui` first (trunk) then the workspace — the `new-siamstr` Dockerfile two-stage shape is the reference.
- A second build tool (`trunk`) and the `wasm32` target join the toolchain; wasm crates never enter `cargo build --workspace` (they cannot compile on the host target).
- UI leptos/thaw upgrades are UI-local and gate on a *stable* thaw pairing for the target leptos line (see roadmap R11); the upstream Svelte build remains a drop-in UI over our REST API, preserving OVERVIEW's compat promise at the API layer.
- Upstream parity story for UI tests: the 110-test suite has no Svelte-render tests — the UI's own acceptance is `wasm-bindgen-test` + per-screen manual smoke (W6.5), not a port of upstream lint.
