# 35 — W6.8 · Composition root points `mem-web` at the built web UI

Status: done — 2026-10-21 (reviewer APPROVED on iteration 1: 20 web_dist_wiring oracles green/0 failed, 35/35 root suites, full `cargo test --workspace` 0 failed, fmt + clippy clean, 10/10 security mandates PASS with zero violations, live positive smoke served the shell with the token injected, live negative smoke emitted `web.dist_missing` + `web.launch_failed` with the port refusing connections)

**Scheduling:** its own wave (a small integration slice) — sequenced BEFORE W6.7's browser
smoke. Named here because two open issues (24, 29) require this wire and no wave owns it.
Goal-scoped in the vault project `rust-port-delivery`.

## What to build

The Leptos explorer builds to `web-ui/dist` (Trunk), but the engine's composition root never
points the web server at it. `src/main.rs:89` sets `dist_dir: storage_root.join("web-ui")`,
so the server serves `<storage_root>/web-ui` — a path the build never writes to. mem-web
then serves that directory (`crates/mem-web/src/static_hosting.rs:106`
`ServeDir::new(&state.dist_dir)`) and looks for the app shell at `<dist_dir>/index.html`
(`:154`, `:171`), which is absent. The built app is therefore unreachable from the live
server, and every live-server smoke that needs a rendered screen is blocked on it.

The fix is one derivation of "where is the built UI", resolved at the composition root and
handed to mem-web's `AppState.dist_dir` (`crates/mem-web/src/server.rs:34`). It must work
for a launched binary, not only for a checkout: the path may come from an explicit setting
(env var or CLI flag) with a documented default, and it must never be a hard-coded
repo-relative absolute path. A missing build must degrade loudly — the same structured
`web.launch_failed` path the rest of mem-web uses (`crates/mem-bridge/src/engine.rs:648`) —
not a silently empty page. `web-ui/dist` is gitignored build output and is never committed;
the wiring must not assume it exists.

This is a cross-lane change: `src/main.rs` is W5.x-owned (the Rust lane) and mem-web's
static hosting is W6.1's. It must be a NAMED wave, never a silent edit inside a UI wave
that does not own the composition root.

## Acceptance criteria

- [x] The composition root hands mem-web a `dist_dir` that resolves to the built Leptos app (`web-ui/dist`), so `GET /` returns the app shell and unknown client-routed paths fall back to `index.html`.
- [x] The location is resolved from an explicit, documented setting (env var or CLI flag) with a stated default; no hard-coded absolute or repo-relative path that breaks outside a checkout.
- [x] A missing build degrades loudly through the existing structured web-launch failure path (`web.launch_failed`), not a silent empty page or a panic.
- [x] `web-ui/dist` stays gitignored build output; the wiring never commits it and never requires it to exist at build time.
- [x] Verified live: with the UI built, the server answers `GET /` with the app shell; without it, the failure is loud and diagnosable from the structured log.

## Blocked by

- 29 · W6.5 (the Trunk build that produces `web-ui/dist`; its shell slice already lands)

## Comments

- **2026-10-21 — closed.** Blueprint `tmp/blueprints/blueprint-20261021-0359-w68-composition-root-dist-dir-r3.md` (canonical body sha256 `b3344088ed08`; `approval: SYS-APPROVED 2026-10-21T04:23:46Z`) after three gate rounds — r0 rejected with M1–M10, r2 rejected with R1–R5 (r2's non-UTF-8 oracle asserted a `Vec<String>` that cannot panic, so it could never fail), r3 APPROVED. Reviewer APPROVED on iteration 1, no rework.
- **Owned net regression (tracked by issue 36, deliberately not softened):** until issue 36 lands, a plugin-launched engine resolves no dist and refuses its web launch, so the ENTIRE axum surface — the REST API AND `/api/health`, not merely the UI — is unavailable for a plugin-launched engine; W6.7's browser smoke and W7.2's web-UI rehearsal cannot pass before it closes. Documented in `docs/bridge-schema.md` §9 (issue 36 by id) and the `docs/web-skeleton-smoke.md` D-7 note.
- **Carry-forward (non-blocking, not an AC):** the automated oracle uses `--web-dist=<path>`; the POSITIONAL form `--web-dist <path>` has no dedicated spawn oracle — the reviewer verified it works end-to-end by hand (200), so it is a coverage gap, not a behaviour bug.
- **Uncommitted:** the wave's six paths (`src/lib.rs`, `src/web_dist.rs`, `src/main.rs`, `tests/web_dist_wiring.rs`, `docs/bridge-schema.md`, `docs/web-skeleton-smoke.md`) exist in the working tree only; the wave made no git writes. Committed `HEAD` alone does not compile (issue 24's still-uncommitted `mem_web::api::ports`).
