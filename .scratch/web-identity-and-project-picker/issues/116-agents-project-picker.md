# 116 — /agents demands a pasted project id and has no project list to offer

Status: done — 2026-11-03 — landed in `deb8bf4` (the store's project-roster statement),
`2e8817f` (`GET /api/projects` + the surface pin) and `1204f83` (the screen). Evidence:
`crates/mem-episodic/tests/project_roster.rs` (5), `crates/mem-web/tests/api_projects.rs` (4),
`web-ui/tests/agents.rs`, `web-ui/tests/api_paths.rs`, `web-ui/tests/nav.rs`. Browser evidence is O2's.

Defect B of `.scratch/web-identity-and-project-picker/PRD.md`.

## Symptom (MEASURED, 2026-11-02)

`agent_sessions` held 94 rows across six project directories. `/agents` required the project id to be
pasted into a text box, and offered no way to discover one. After a correct paste the percent-encoded
flow worked (the `[]` milestone answer was a genuinely empty table — item B has one cause, not two).

## Fix

- **Store (D-6):** `sql::PROJECT_ROSTER` + `EpisodicStore::project_roster(limit)` — ONE statement over
  the UNION of live `agent_sessions` projects and `episodic_milestones` projects, LEFT JOINed to a
  live-session count, a milestone count and `last_activity_at = COALESCE(newest live session, newest
  milestone, 0)`, ordered `last_activity_at DESC, project_id ASC`, `LIMIT ?`. Consolidation hard-deletes
  its sources, so a sessions-only roster loses every consolidated project; milestones alone have no
  history for a never-consolidated one. The soft-delete filter sits in both session subqueries.
- **REST:** `GET /api/projects` (port-only) projects the four camelCase fields, bounded by the shared
  strict `?limit` parser extracted from the milestone route (`api::parse_limit`, default 200, max 500,
  fixed `400` outside, never a clamp), WARNing at the bound; `501` when the port is unwired.
- **Screen:** mount reads the roster, selects `?project=` when the roster carries it else the
  newest-activity row (`selected_project`: never `""`, never a fabricated id), then fetches that
  project's milestones. The paste box + Load pair becomes a `<select>` + Refresh. `NeedsProject` and
  `AGENTS_NEEDS_PROJECT_MESSAGE` are deleted; `AGENTS_EMPTY_MESSAGE` names the milestone projection and
  `AGENTS_EMPTY_DETAIL` explains the hourly consolidation pass (no number, no promise).
- **Surface pin (D-10):** both port-only rows land above the last `DELETE /api/user-profile` row in one
  round with every numeral (41 rows, 39 `/api/*`, 33 `.route(..)`, 38-row token walk, 40-row
  Host/Basic/reachability walks). Between the route's registration and the pin the surface is
  green-but-incomplete by construction — no test enumerates routes from the router side — and no gate is
  claimed inside that window.

## Residuals

- No in-process test for `AgentsPanel` (the `web-ui` suite is pure-function wasm tests). Closed by O2:
  navigate `http://127.0.0.1:3742/agents` through the chrome-devtools MCP and assert the project list
  with `N sessions · M milestones` plus the roster / empty+detail arms. A raw headless spawn is not
  evidence.
- The roster's `COALESCE` precedence means a project that still holds live sessions is dated by
  its newest LIVE session, while a fully consolidated one is dated by its newest milestone. That is the
  authored rule and is pinned by `test_project_roster_unions_live_sessions_and_milestones`.
- A project whose every session is soft-deleted AND which has no milestone is absent from the roster.
  Deliberate: the roster is over live source rows (D-6). Its data is not lost — the rows survive.
