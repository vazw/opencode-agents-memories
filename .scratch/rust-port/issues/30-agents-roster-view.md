# 30 — W6.6 · Agent contribution view (US 5.1)

Status: ready-for-agent — 2026-10-27 (web-ui slice landed `66e78e7`; residual creates a `RouteSpec`
milestone row in `mem-web`'s `ROUTE_TABLE`, a port seam over `EpisodicStore::list_milestones`, the
panel's single fetch at `web_ui::agents::initial_outcome()`, and the `api_endpoint_contract`
re-baseline — agent brief under `## Comments`). This line read `needs-triage` while the view had
shipped six days earlier: that is `surface-built-ahead-of-wiring`, not an unstarted issue.

## What to build

An `Agents` view in the `web-ui` Leptos/Thaw explorer: **who contributed to this project's durable memory**.

Under the thesis adopted on 2026-10-21 — *"the episodic tier is staging, not a permanent tier"* — raw session rows are transient material that consolidation ages into **milestones** and promotes into **durable facts**; agents survive as **attribution**, not as a growing registry. So this view is built on the **milestone contributors**, not on the raw session listing: `EpisodicStore::list_milestones(project_id, limit)` returns `Milestone`s carrying a decoded `contributors` list (`crates/mem-episodic/src/milestone.rs`), and that list is the only place in the system where durable agent attribution survives.

The screen lists the distinct contributing agents of the current project scope with their milestone count and last contribution, and drills into the milestones (time window, source count, promoted facts) they contributed to. **Promoted facts are agent-anonymous by construction** — `PermanentMemorySink::promote(scope_hash, record)` derives the fact id from `sha256(project_id + "\x1f" + key)`, with no agent component — so the agent→fact link is only reachable through the milestone that produced it.

This closes a **product** gap, not a parity gap: upstream opencode-mem has no agents screen and no agents endpoint (its SPA has exactly three client routes; `agent` is only a per-message attribute in the v2 adapter).

**Backend requirement (new):** there is no REST route exposing milestones — W6.2's `ROUTE_TABLE` (37 rows) has no milestone row — so this wave needs a new endpoint over the milestone read path plus the matching port seam. That grows the route table, whose row count is pinned by W6.2's approved blueprint and by its contract test; the growth must be **re-baselined explicitly in this wave**, never silently.

Wave detail: `.scratch/rust-port/roadmap.md` §W6.6.

## Acceptance criteria

- [ ] The client route table carries an `Agents` entry (path, label, owner) and the nav model renders it as a first-class entry next to Sessions; the route table stays the single source of truth for client routes and the `Owner` enum gains this wave's owner.
- [ ] A milestone read path exists at the REST surface (new route + port seam over `EpisodicStore::list_milestones`), and the route table's row count is re-baselined in the same wave with W6.2's contract test updated to match — no silent contract drift.
- [ ] The screen lists the distinct contributing agents of the current project scope with `agent_id`, display name, milestone count and last contribution, derived from the milestones' `contributors`; an empty scope renders an empty state, not an error.
- [ ] Selecting an agent shows the milestones they contributed to (with their time window and source count) and the facts those milestones promoted.
- [ ] The screen reuses the existing same-origin `fetch` seam (single Basic-forwarding `Authorization`); no new credential surface and no schema change — in particular no `agent_id` column is added to `memories`.
- [ ] `web-ui/src/routes.rs` carries the new `Agents` route with this wave's owner, **and** re-homes the four stale `Owner::W6_2` labels (`ProjectMemories`, `Tags`, `Profile`, `Settings`) to the W6.7 lane's owner so the explorer's planned-notices stop naming a closed wave — exactly once, whether here or in W6.7.
- [ ] Manual smoke: the Agents view renders against the Rust server for a project that has consolidated milestones.

## Blocked by

- 25 · W6.3 (the session endpoints and the screen seam this view sits beside)
- 29 · W6.5 (the shell, route table, nav model and fetch seam it renders in)
- 17 · W4.3 (the consolidation that produces milestones and their contributors) — closed

## Out of scope

- Raw-session browsing as the Agents view: raw rows are staging under the adopted thesis; the session-history screen (W6.3) is the transient-activity view.
- Agent attribution on tier-1 memories: promoted facts are agent-anonymous by construction; adding it is a schema decision (ADR candidate) and a separate lane.
- A cross-project / global Agents view.

## Comments

- **2026-10-27 — triage: the web-ui slice is landed, the backend is not. Re-scoped to the residual; agent
  brief below.** Verified against the tree at HEAD `1ef7fe6`, by symbol rather than line number so it
  survives refactors:

  | AC | state | witness |
  |---|---|---|
  | 1 route + nav | **landed** | `RouteId::Agents` in the client route table (`path: "/agents"`, label `Agents`, `Owner::W6_6`), rendered by the nav model |
  | 3 roster | **landed** | `web_ui::agents::roster(&[MilestoneDto]) -> Vec<AgentSummary>` — distinct milestone count, latest source stamp, display name from the latest source |
  | 4 drill-down | **landed** | `web_ui::agents::agent_detail(..)` — milestones newest first with server-reported source count and the facts they promoted, deduped by fact id |
  | 6 re-home four `Owner::W6_2` labels | **landed** | those four rows now carry `Owner::W6_7` (wave 31 did the re-homing) |
  | 2 REST milestone read path | **absent** | `mem-web` has no milestone route: a case-insensitive search for `mile`/`contribut` across `crates/mem-web/src` returns nothing, and `EpisodicStore::list_milestones` has callers in `mem-episodic`, `mem-portability` and `mem-bridge` tests but **none in `mem-web`** |
  | 5 same-origin fetch seam | **not wired** | the panel issues no request by design; `initial_outcome()` returns `AgentsOutcome::Unavailable` |
  | 7 manual smoke | **blocked on 2** | cannot render against the server without the route |

  **One correction the wave must carry.** The panel's own module doc says `ROUTE_TABLE` is **37** rows and
  "the deferred wave adds the 38th". The table is **38** today — the `POST /api/deduplicate` row landed
  after the slice (issue 54) — and `api_endpoint_contract` pins it with
  `assert_eq!(ROUTE_TABLE.len(), 38, …)`. So the milestone row is the **39th**, and the contract moves
  38 → 39. Re-derive the number; do not trust the doc comment, which is one route behind.

## Agent Brief

**Category:** enhancement
**Summary:** add the milestone read path to the REST surface and wire the already-landed Agents panel to
it — the view is built, the wire is not.

**Current behavior:**
The Agents view renders a documented *unavailable* state because no milestone route exists at the REST
surface. Its pure functions (`roster`, `agent_detail`, `format_window`, `agents_view`) are implemented and
covered by `web-ui`'s wasm test suite; `initial_outcome()` is the single place the build hard-codes
`AgentsOutcome::Unavailable`, and its doc comment names that function as the wiring point.

**Desired behavior:**
A project-scoped milestone projection is reachable over the same-origin REST surface, and the panel
fetches it once: `Loading` → `Loaded` with the roster rendered, or `Failed(message)` on a transport or
decode error. An empty project scope renders the empty state — not an error, and not the old unavailable
state. Drilling into an agent shows its milestones and the facts they promoted, in the shape the slice
already defines.

**Key interfaces:**
- `EpisodicStore::list_milestones(project_id: &str, limit: usize) -> Result<Vec<Milestone>, EpisodicError>`
  — exists and fails soft on undecodable contributor rows; wrap it, do not reimplement it.
- `Milestone` (contributors decoded from JSON, source sessions, time window) plus the promoted-facts
  accessor the client DTO already mirrors — the projection must carry exactly what `MilestoneDto` decodes,
  because the view functions consume that shape unchanged.
- `mem-web`'s `RouteSpec` / `ROUTE_TABLE` and the `api_endpoint_contract` row-count assertion — one new
  row, and the count moves with it.
- `web-ui`'s `EndpointSpec` / `ENDPOINTS` / `EndpointId` — its own ordinal assertion pins the catalog, so
  the client side must grow in step.
- `web_ui::agents::initial_outcome()` — the single wiring point; the view functions must not change.
- The existing same-origin fetch seam and its single Basic-forwarding `Authorization` header — reuse it;
  add no credential surface.

**Acceptance criteria:**
- [ ] A milestone route exists at the REST surface, project-scoped, returning the projection the client
      DTO already decodes. A request scoped to one project never returns another's data.
- [ ] `api_endpoint_contract`'s pinned row count is re-baselined in the same commit as the new row
      (re-derive the current value; the module doc's 37 is stale at 38). No silent contract drift.
- [ ] The client `ENDPOINTS` / `EndpointId` catalog grows in step and its ordinal assertion still holds.
- [ ] On mount the panel issues exactly one request and renders `Loading` then the roster; a transport or
      decode failure renders `Failed` with a message — no panic, no error page.
- [ ] An empty scope renders the empty state, distinct from both `Failed` and the old unavailable state.
- [ ] `AgentsOutcome::Unavailable` no longer describes the production path. Either it is removed with its
      render state, or a build mode that genuinely lacks the route can still reach it — **a variant
      reachable only from a test is the vacuous-guard shape**, so the blueprint must state which was
      chosen and why.
- [ ] The existing view functions (`roster`, `agent_detail`, `format_window`, `agents_view`) and their
      tests are untouched; the wiring is additive.
- [ ] Manual smoke: roster rows and a drill-down render against the running Rust server for a project with
      consolidated milestones, zero console errors, every request same-origin.

**Out of scope:**
- Agent attribution on tier-1 memories — promoted facts are agent-anonymous by construction; that is a
  schema decision and a separate lane.
- A cross-project or global Agents view.
- Reworking milestone consolidation, or adding an `agent_id` column to `memories`.
- Any other screen's route, owner labels or catalog rows.
