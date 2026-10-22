# 25 — W6.3 · Session-history endpoints — new (US 5.1)

Status: done — 2026-10-20 (reviewer APPROVED: 9/9 endpoint tests green, 60/60 web-ui wasm tests, clippy/fmt clean, 10 security mandates PASS, demo harness smoke verified end-to-end; the empty-segment 404 is produced in the adapter, not the router — divergence D-25)

## What to build

New endpoints powering the Leptos/Thaw session-history tab (W6.5): `GET /api/projects/:project_id/sessions` and `DELETE /api/projects/:project_id/sessions/:session_id`, over the W4.2 query layer. This closes the Epic 1 UI side. Wave detail: `.scratch/rust-port/roadmap.md` §W6.3.

## Acceptance criteria

- [x] Endpoint tests follow the same handler conventions as W6.2 parity handlers.
- [x] Manual demo: session history renders in the `web-ui` Leptos UI.
- [x] Delete is orphan-safe (timeline consistency per W4.2 guarantees).

## Blocked by

- 16 · W4.2
- 24 · W6.2

## Comments

### 2026-10-21 — W6.3 implementation note (coder handoff)

Landed the session-history slice behind the W6.2 seam. The two routes and the
`SessionPort` trait are unchanged; this wave supplies the adapter, the tests, the
screen and the demo.

**Endpoints (contract now executable).**
- `GET /api/projects/{project_id}/sessions` → `200 {"success":true,"data":[SessionSummary…]}`
  over `EpisodicStore::timeline(project_id, &TimelineFilter::default())`
  (project-scoped, newest first, soft-deleted rows invisible).
- `DELETE /api/projects/{project_id}/sessions/{session_id}` → `200 {"success":true}`
  over `EpisodicStore::soft_delete_session`; unknown / wrong-project /
  already-deleted all answer the one fixed
  `404 {"success":false,"error":"Session not found"}` (no existence leak, no id
  echo). Orphan-safety is the store's single `BatchMode::Write` batch
  (`MARK_CHILDREN_ORPHANED` + `SOFT_DELETE_SESSION`): the child stays visible in
  the timeline with `orphaned:true`.

**What shipped.** `EpisodicSessionPort` + `episodic_session_port(store)` in
`crates/mem-web/src/api/store_adapter.rs` (with the `EpisodicError` → `PortError`
mapping); `crates/mem-web/tests/api_sessions.rs` (9 cases over a real
`episodic.db`, hermetic `StoreConfig`, fixed-vector `Embedder`, injected clock +
id factory); `crates/mem-web/examples/session_history_demo.rs` (real store +
real adapter + the built `web-ui/dist`, bound `127.0.0.1:0`, demo-only local
token); `web-ui/src/nav.rs` (`ScreenState::Live`, `SessionHistory -> Live`) and
`web-ui/src/screens.rs` (fetch + render + per-row delete, both paths built from
the pinned templates with `paths::encode_component`); `web-ui/tests/screens.rs`;
`docs/web-skeleton-smoke.md` §F (real timeline/delete/orphan smoke) + D-13
amended + new divergence **D-25**.

**Recorded divergence (D-25).** `matchit` 0.8.4 matches an *empty* segment as a
`{project_id}` value, so `/api/projects//sessions` reaches the handler with `""`
instead of a router 404. It is pinned to `404` (not `400`) in the adapter; a
non-empty malformed id still answers the store's `400`.

**Deliberate posture.** `ApiServices::production()` keeps `UnavailablePort` for
sessions until the W5.x composition root calls
`.with_sessions(episodic_session_port(store))`; production therefore still
answers the documented `501` by design. The demo's `demo-token` is demo-only and
loopback-only — never a production credential.

**Evidence.** `cargo test -p mem-web --test api_sessions` 9/9 green;
`cargo clippy -p mem-web --all-targets -- -D warnings` clean;
`cargo fmt -p mem-web --check` clean; `web-ui`: fmt/clippy(wasm)/test(wasm) green
(60 wasm tests); `trunk build` 0; `session_history_demo` printed
`http://127.0.0.1:39911/sessions`, project `demo-project`, parent/child ids, and
the live smoke showed list → 200, delete parent → 200, child `orphaned:true`,
second delete → 404, traversal GET `data:[]` / DELETE 404, whitespace id → 400,
empty segment → 404, no token → 401, foreign `Host` → 403; the built `dist/`
rendered in Chrome (both rows, then the parent-delete re-listed the child with
the `orphaned` badge and no console error).
