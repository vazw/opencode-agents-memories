# PRD — web profile identity + the /agents project picker

Blueprint: `.tmp/blueprints/blueprint-20261102-1726-web-profile-identity-and-agents-picker-r3.md`
(stamp `1ec2b30cc56d`, SYS-APPROVED 2026-11-02T17:33Z). Two web-surface defects, one wave.

## The two defects

**A. The profile page says "no profile" while two profiles exist.**
The composition root built the web `AppState` with a literal identity, so `GET /api/user-profile`
answered `{"exists":false,"userId":"unknown"}` on the live engine even though the store held two real
profiles (`vaz@siamstr.com`, 1430 analyzed prompts; `jakkaphat.j@bangmod.co.th`, 330). The inject path
resolved a real identity through `EnvResolver`; the web surface resolved nothing. Two identities, one
of them fabricated.

**B. `/agents` demanded a pasted project id and had no list to offer.**
`agent_sessions` held 94 rows across six project directories, but the screen required the user to
paste one and had no discovery path — and the milestone projection it then renders was empty for every
project (`episodic_milestones` had no rows at all).

## User stories

- **A1** As the owner, opening the profile page on a fresh engine shows my real identity and my real
  profile without editing a query string.
- **A2** As an operator whose engine was launched from more than one directory, the page shows which
  identity it is displaying and lets me switch to another stored identity — it never silently serves
  one identity's data under another's name.
- **A3** As a client of the REST surface, a missing profile for the identity I asked about says so and
  tells me whether other identities exist, without naming them.
- **B1** As the owner, opening `/agents` shows every project I have activity in, with how much of it
  (sessions and milestones), newest activity first.
- **B2** As the owner, choosing a project renders its agent roster, or — when it has no milestone yet —
  an explanation of what writes milestones, not a bare empty box.
- **B3** As the owner, a bookmarked `?project=<id>` still opens on that project.

## Non-goals

- No second identity resolver, and no identity inferred from the store (that is the defect).
- No milestone synthesis: the zero-milestone finding is diagnosed and filed as issue 117, not "fixed".
- No second `agent_sessions`-contributors endpoint (D-8): the roster is the union, in one statement.
- No schema change, no new config key, no new crate, no migration.
- `README.md` is untouched (its configuration section is a separate wave in flight).

## Acceptance criteria

Both new routes are **port-only** rows of the pinned `api::ROUTE_TABLE` (ADR-0003), appended above the
existing `DELETE /api/user-profile` row, which stays last, with every numeral re-pinned in one round
(41 rows, 39 `/api/*`, 33 `.route(..)` calls, a 38-row token walk, 40-row Host/Basic/reachability
walks) — and the auth/Host/Basic walks cover them, because an unlisted route is an un-gated route.

| # | criterion | proof |
| --- | --- | --- |
| AC-A1 | the web identity is the inject path's identity for the launch cwd | `crates/mem-web` handler default-path case + O1 (live) |
| AC-A2 | the identity resolver never panics and never invents an identity | `tests/web_identity.rs` (6) |
| AC-A3 | the roster lists every stored identity, marks the composed one by identity, is ordered, bounded, and 501s when unwired | `crates/mem-web/tests/api_profiles_roster.rs` (6) |
| AC-A4 | the picker appears exactly when the roster is not the single composed identity, and never auto-selects another | `web-ui/tests/core_screens.rs` (identity-control oracle) |
| AC-A5 | the missing-profile message names the requested identity and a count; zero profiles keeps upstream's literal byte-for-byte | `crates/mem-web/tests/api_profiles_roster.rs` |
| AC-B1 | the roster is the union of live sessions and milestones, counts live sessions only, orders deterministically, respects the limit | `crates/mem-episodic/tests/project_roster.rs` (4) |
| AC-B2 | the route projects all four fields, preserves a path-shaped id, refuses a bad `?limit` with 400, 501s when unwired | `crates/mem-web/tests/api_projects.rs` (4) |
| AC-B3 | the screen consumes both templates and selects the requested project or the newest-activity one | `web-ui/tests/api_paths.rs`, `web-ui/tests/nav.rs`, `web-ui/tests/agents.rs` |
| AC-B4 | the browser shows the identity line/picker and the project list | O2 (chrome-devtools MCP, owner/debugger) |

## Decisions worth keeping (full text in the blueprint)

- **D-1/D-2** identity is resolved where composition happens, through the inject path's own
  `ProjectScopeResolver`, over the **launch cwd** (`getTags(process.cwd())`), resolved once.
- **D-3** multiple identities get a picker; the page never substitutes one for another.
- **D-4** the missing-profile message becomes honest; upstream's literal survives the state upstream
  can actually reach (zero profiles).
- **D-6** the project roster's source of truth is the union of live `agent_sessions` and
  `episodic_milestones`, because consolidation hard-deletes its sources.
- **D-7** the zero-milestone finding is diagnosed (issue 117), not fixed here.
- **D-10** the surface pin is one step after both routes exist; no full-suite claim is made inside the
  registered-but-unpinned window.

## Issues

- `issues/115-web-profile-identity.md` — defect A.
- `issues/116-agents-project-picker.md` — defect B.
- `issues/117-zero-milestones-diagnosis.md` — the INFERRED cause of the empty milestone table.
