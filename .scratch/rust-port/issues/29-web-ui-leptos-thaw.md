# 29 — W6.5 · Leptos/Thaw web explorer UI (`web-ui`) (US 5.1)

Status: ready-for-agent — 2026-10-27 (verified at HEAD: all ten client routes are `Live` — every screen incl. session-history exists and is wired; residual is AC3's live smoke and the parity proof it carries — brief under `## Agent Brief`)

## What to build

The new `web-ui` package (outside the workspace graph per §0b D5 / ADR-0003): Trunk-based leptos 0.7/thaw 0.4 CSR app — ConfigProvider/Theme shell with light/dark, client routing reproducing the upstream Svelte app's screens (**full screen-set parity**: memories list/search, tags, profile, settings, export/import), the same-origin `fetch` REST client with a single Basic-forwarding `Authorization` seam for auth-configured servers (**credential-free localhost is the primary posture — no login/prompts ship**), and the **session-history/timeline tab** consuming W6.3. No Leptos server functions anywhere — the hand-written axum REST handlers (W6.2) are the only API surface. Wave detail: `.scratch/rust-port/roadmap.md` §W6.5.

## Acceptance criteria

- [x] `trunk build` produces `web-ui/dist`, which W6.1's static hosting serves (`GET /` loads the shell; unknown client-routed paths fall back to `index.html`).
- [x] Exact wave pins respected: `leptos = "0.7"` (floor `^0.7.7` via thaw), `thaw = "^0.4.8"` (stable line), `trunk` + `wasm32-unknown-unknown`, no leptos `nightly` features.
- [ ] `wasm-bindgen-test` for REST-client shape; manual smoke per screen against the Rust server.
- [ ] Full parity: every upstream Svelte app screen is reproduced — memories list/search, tags, profile, settings, export/import, and the session-history/timeline tab rendering W6.3 data.
- [x] No login/prompt UI ships; the fetch seam supports Basic forwarding only when the server is auth-configured.

## Blocked by

- 23 · W6.1 (shell slice)
- 24 · W6.2 (per-screen lanes: memories, tags, profile, settings, export/import)
- 25 · W6.3 (session-history tab lane)
- 31 · W6.7 (the four core screens — AC3/AC4 await them)
- 26 · W6.4 (the ExportImport/SkillLibrary screens — named in AC4's parity list)
- 35 · W6.8 (the composition-root wire the live-server smoke needs)

## Comments

- **2026-10-18:** created by the Rev-3 roadmap fold (web-stack ruling, ADR-0003). Tracer-bullet slices and wave text: `.scratch/rust-port/roadmap.md` §W6.5.
- **2026-10-20 (W6.5 shell slice landed, issue stays open):** the standalone `web-ui/` package (workspace-excluded via the root `exclude = ["web-ui"]` line) now builds with Trunk 0.22 to a Leptos 0.7.8 CSR + Thaw 0.4.8 WASM bundle in `web-ui/dist/` — shell layout, nav model, the eight-entry client route table, the baseline light/dark theme (`opencode-mem-theme`), the single typed same-origin `fetch` seam (`build_url` / `prepare` / `finish` / `map_transport` / `request<T>`) with its auth seam (server-injected `__OPENCODE_MEM_TOKEN__` header + optional Basic, never Bearer), and the pinned 34-entry REST path table. Every screen route except the shell landing renders a "not yet implemented" notice naming its owning wave and the endpoints it will consume; nothing pretends to have data. Gates run: G1–G5 and G7–G11 green (`cargo test --target wasm32-unknown-unknown` = 50 passed across the eight test files; `cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings` = 0; `trunk build` = 0 with all three hashed artifacts; `cargo audit --deny warnings --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2026-0173` = 0; the three browser assertions at `127.0.0.1:9222` passed: `/` → home-panel, `/project-memories` → planned-notice naming W6.2, `/nope` → unknown-route; plus the manual smoke: client-side nav without reload, theme persists across reload, zero console errors, zero external-origin requests). G6 (`cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`) — clippy and the full workspace test run are green (`tests/workspace_deps.rs`, `tests/context_docs.rs`, `tests/workspace_smoke.rs` included); `cargo fmt --all --check` reports diffs only in W6.2's in-flight `crates/mem-store/src/vector.rs` and `crates/mem-web/**` files, which this wave's confinement forbids touching. **Still open:** the per-screen parity AC (memories, tags, profile, settings, export/import → W6.2 / #24; skills screen → W6.4), the session-history tab (W6.3 / #25), and the server-integration smoke + live token check, which need the W5.x composition root to point `mem-web`'s `dist_dir` at `<repo>/web-ui/dist`.

- **2026-10-20 (W6.5 close-out — reviewer APPROVED):** the code-reviewer re-ran the wave's gates independently and returned `STATUS: APPROVED` — G1-G11 green, the 50-test wasm-bindgen battery green, the Trunk bundle's three hashed artifacts present with `data-trunk` count 0 and no token marker in `dist/index.html`, the security source gates green, the ledger closed at 12/12 rows, and the W6.5 <-> W6.2 coupling verified (the 34-entry pinned REST table is a correct subset of W6.2's ROUTE_TABLE, and the route owners match the blueprint's table: ProjectMemories/Tags/Profile/Settings -> W6.2, ExportImport/SkillLibrary -> W6_4, SessionHistory -> W6.3). The G6 fmt block on four W6.2-owned files is RESOLVED — `cargo fmt --all --check` now exits 0 workspace-wide, so the cross-wave finding closed. **AC state:** AC1 (trunk build -> dist, shell loads, unknown client-routed paths fall back to index.html), AC2 (exact wave pins: leptos 0.7.8, thaw 0.4.8, trunk 0.22, wasm32-unknown-unknown, csr only — no nightly/ssr/hydrate) and AC5 (no login/prompt UI; the fetch seam forwards Basic only when the server is auth-configured) are MET and ticked. **AC3** (wasm-bindgen-test for the REST-client shape AND manual smoke per screen against the Rust server) and **AC4** (full upstream Svelte screen parity) are NOT met: there is no composition root yet, so the live credential check (extract the injected token from a running mem-web, prove it is absent from dist/) stays deferred, and every screen route except `/` still renders a planned-notice naming its owning wave. **This issue stays OPEN** — the per-screen parity work lands in the per-screen lanes (memories/tags/profile/settings/export-import -> W6.2 / #24, skills screen -> W6.4 / #26, session-history tab -> W6.3 / #25). The reviewer's non-blocking notes carried forward: the shell's `read_marker_token` browser adapter is not yet wired into an AuthContext (the screen lanes own that wiring), and the `Status:` line change in this diff is the orchestrator's pre-wave triage flip, not a wave edit — the wave itself touched `## Comments` only.

## Agent Brief

**Category:** enhancement
**Summary:** the screens are all built and wired; prove them against the live server.

**Current behavior (verified at HEAD `206cf9f`):**
The shell slice landed and was reviewed. Every client route now resolves to a **live** screen — the route
module's own header states that the nav model answers live for all of them and that no route renders the
placeholder any more. The screen set is complete: memories/list-search, tags, profile, settings, export/import,
skills library, session history, agents and stats. The session-history tab exists and is wired (its owning lane
closed `done`). The `Planned` state and its notice still exist in the code, which is correct — it is the
mechanism, no longer the state of any route.

**Desired behavior:**
Each screen renders real server data against the running Rust engine, with zero console errors and every
request same-origin. The REST client's shape is covered by its wasm tests; this brief adds the part tests
cannot reach — a live browser smoke per screen. The session-history screen is the one that may still answer a
stubbed port, because the sessions wiring at the composition root is issue 37's AC6; if it does, that is
recorded as the residual with its owner, not worked around.

**Key interfaces:**
- The same-origin fetch seam and its single auth seam — Basic forwarding only when the server is
  auth-configured; the injected token header, never Bearer.
- The client route table and the nav model's state function — the source of truth for which screens are live.
- The live server's custom token header (`x-opencode-mem-token`); a `Bearer` header will 401.

**Acceptance criteria:**
- [ ] A live smoke per screen: each route loads against the running engine, renders server data (not a
      notice), makes same-origin requests only, and logs zero console errors.
- [ ] The session-history screen is either proven against live data, or recorded as blocked by issue 37 · AC6
      with the notice it renders.
- [ ] The wasm test battery still passes, and a test pins the REST-client shape.
- [ ] No screen falls back to the planned notice for a route the nav model calls live.

**Out of scope:**
- Building screens — they exist; this is verification.
- The sessions port wiring (issue 37 · AC6).
- The desktop/mobile visual polish and any new screen.
