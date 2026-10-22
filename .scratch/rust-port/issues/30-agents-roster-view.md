# 30 — W6.6 · Agent contribution view (US 5.1)

Status: needs-triage

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
