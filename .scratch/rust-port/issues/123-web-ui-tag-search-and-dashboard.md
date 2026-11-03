# 123 — the memories screen cannot search by tag (400), and the shell still reads like a prototype

Status: needs-triage — filed from the owner's pass over the live UI (2026-11-03, `http://127.0.0.1:3742`,
bundle `web-ui-321d4859fd9a772f`, engine `0ba4e0f1`). **Part A–E are measured defects and named jobs;
Part F–G are a design pass** that needs an `architect` A/B draft before any blueprint, because two of its
items change the API surface. Nothing here is `ready-for-agent` until that split is decided.

**Owner ruling 2026-11-03: A1.** The fix is a free-text `tags=` filter, server-side, matching the
memory `tags` column; `tag=` keeps its scope meaning. A2 (container-tag picker) is rejected — it filters
projects, not tags, and leaves the clickable chips decorative. The wave is in design; the other two open
questions (does `/tags` gain a free-form tag group; is user scope dead on this install) are still open
and the architect's draft proposes an answer for each.

Owner's notes, verbatim, are the source of this file: *"the web still need more care on /project-memories
page … Search memories with tag return server error 400: Invalid container tag … restyle rendered memories
list to card and can click on the tag to search this selected tag … on /tags page show only project and
user but user is zero: User (0) … on /sessions still have to type project id … I don't like the fixed text
length, it should be reactive to screen size … also find an icon for Switch theme button and all menu
button … the key is to modernize the web page like a serious web dashboard."*

Every claim below is labelled **MEASURED** (command + output reproduced in this file) or **READ** (file:line).

---

## A. `?tag=` is a *container* tag, so every human tag is a 400 — MEASURED

Reproduced against the live engine with the page's own injected token
(`window.__OPENCODE_MEM_TOKEN__` from `GET /`; `crates/mem-web/src/static_hosting.rs:23`):

| request | status | body |
|---|---|---|
| `GET /api/memories?page=1&pageSize=5` | **200** | items… |
| `GET /api/memories?…&tag=ops-drill` | **400** | `{"success":false,"error":"Invalid container tag"}` |
| `GET /api/memories?…&tag=deployment` | **400** | same |
| `GET /api/memories?…&tag=opencode_project_03b7931d97d0837c` | **200** | items… |
| `GET /api/search?q=deploy&page=1&pageSize=5&tag=deployment` | **400** | same |
| `GET /api/search?q=deploy&page=1&pageSize=5` | **200** | items… |

**The chain (READ):**

1. `web-ui/src/core_screens.rs:397-402` — the `#memories-tag-filter` input (`:472-481`, placeholder
   `"tag,tag"`) is split on commas, trimmed, empties dropped. Free text is exactly what it accepts.
2. `:405-409` — those strings become repeated `tag=` pairs on `/api/memories` (`list_memories_spec`,
   `:166-182`) or `/api/search` (`search_spec`, `:185-194`).
3. `crates/mem-web/src/api/memories.rs:392-396` — every `tag` value goes through `resolve_scoped_request`,
   and a failure rejects the **whole** request (`D-10`: never fall back to the unfiltered listing).
4. `crates/mem-web/src/api/guard.rs:55-70` → `mem_core::scope::split_container_tag` — the value must be
   `{prefix}_{user|project}_{16 lowercase hex}`. `ops-drill` cannot pass. /api/search has the same wiring.

So the input promises a capability the API has never had: `tag=` is a **scope narrowing** parameter
(D-10/D-16 in `docs/web-skeleton-smoke.md`), and the screen quietly re-purposed it as free-text search.
A correct input value exists — the 26 container tags `/api/tags` lists — but no user would guess one.

**Why the suite is green anyway:** the contract tests only ever send valid container tags
(`crates/mem-web/tests/api_handlers_container_tag_traversal.rs`, `skills_api_handlers.rs`,
`api_endpoint_contract.rs:260`), and the wasm tests never cross the boundary to assert that the request the
**UI** builds is satisfiable by the **server**. That missing cross-boundary test is the defect behind the
defect; whichever option wins should land one.

**Two options (A/B, decision needed):**

- **A1 — add a free-text filter.** A new parameter (e.g. `tags=`, plural) matching the memory `tags` column,
  substring/exact per tag, case-insensitive; `tag=` keeps its scope meaning. Needs a store query that does
  not exist today — `get_distinct_tags` (`crates/mem-store/src/vector.rs:181`) is over `container_tag`, and
  no statement filters the `tags` column. **This is the option that satisfies "click a tag to search this
  tag"**, and it makes the Tags page meaningful (item C).
- **A2 — make the input a container-tag picker** over `/api/tags`. Smallest change, no server work, honest
  UX; but it filters *projects*, not tags, so B's clickable chips stay decorative.

Recommendation: **A1**, keeping a text input as the escape hatch and adding the picker on top.

---

## B. The memories list is a flat row; tags are one joined string — READ

`web-ui/src/core_screens.rs:338-345`: `<li class="memory-row">` carrying `kind`, `content`,
`tags` (joined with `", "`, `:331-335`), `created`. CSS is a four-column grid
(`web-ui/public/styles.css:768-772`) with the content spanning all columns on row 2 (`:790-795`).

Ask: render each memory as a **card** — type badge, timestamp, content, per-tag **chips**, pin/delete in a
row footer — and make each chip a click that filters by that tag (fires A1) and is visible in the filter
bar as a removable pill.

---

## C. /tags lists only the two scopes, and `User (0)` is structural — MEASURED

`GET /api/tags` → `{"project": 26 entries, "user": 0 entries}`.

READ: `crates/mem-web/src/api/tags.rs:29-39` collects from user-scope **shards** and keeps rows whose
`container_tag` contains `_user_`; shard paths are `<storage>/<scope>s/…` (`crates/mem-store/src/shard_manager.rs:348`).
MEASURED:

```sh
find /home/vaz/.opencode-mem/data -maxdepth 1 -type d
# data, data/.cache, data/projects, data/recovery        ← no `users/` directory
ls /home/vaz/.opencode-mem/data/projects | grep -c shard_0.db   # 26
```

**No user-scope shard has ever been created on this install**, so the group cannot be non-empty here. The
`User (0)` row is truthful — but it advertises a scope with nothing behind it, and the page says nothing
about the tags memories actually carry (memory types like `ops-drill` / `deployment`, and free-form tags).

Ask: keep the scope roster (projects; the user group only when non-empty) **and** add what a tag page is
for — the distinct free-form tags with counts, each one a link into item B's filtered list.

---

## D. /sessions still demands a typed project id — MEASURED

`web-ui/src/screens.rs:360-367` renders `#session-project-id`, `name="projectId"`, placeholder
`"project id"`, bound to a plain text signal. Meanwhile `GET /api/projects` landed with issue **121** and
`/agents` already renders that roster (`web-ui/src/agents.rs:617` formats
`"{label} — {n} sessions · {m} milestones"`).

Live: `GET /api/projects/opencode_project_03b7931d97d0837c/sessions` → `200 {"success":true,"data":[]}`.

Ask: the same picker `/agents` uses, with the session list keyed off the selection; deep-link
`/sessions?project=<tag>` so an agent row can jump here.

---

## E. Fixed measure, one breakpoint — MEASURED

```sh
grep -n 'max-width' web-ui/public/styles.css
# 159: 52rem · 230: 64rem · 371: @media (max-width: 640px) · 609/689/841/956/1239: 64rem · 1004: 72rem · 1123: 14rem
grep -n 'memory-content' -A5 web-ui/public/styles.css
# 793:  max-width: 72ch;
```

Ten panel caps at **64rem = 1024 px** and a single **72ch** measure on memory text, with exactly **one**
breakpoint (`max-width: 640px`) for a ten-route dashboard. On the 1280-wide gate window the app is a
1024 px column with dead margins and text that never reflows for the screen.

Ask: fluid widths (`clamp()` / `min()` against the viewport, a large-screen density that actually uses
1600+), 2–3 real breakpoints (mobile / tablet / desktop / wide), and a measured table in the blueprint at
640 / 1280 / 1600 / 1920 rather than a prose promise.

---

## F. Icons — READ, with one measured gap

Nothing in the shell renders an icon today: the theme toggle is the text "Switch to light"/"Switch to
dark" and the nav is text-only.

- `thaw::Icon` exists — `thaw-0.4.8/src/icon/mod.rs:12`, takes `#[prop(into)] icon: Signal<icondata_core::Icon>`
  with `width`/`height` defaulting to `"1em"`.
- `icondata_ai 0.0.10` is already in the dependency graph via `thaw` (789 icons: `AiMenuOutlined`,
  `AiMenuFoldOutlined`, `AiHomeOutlined`, `AiTagOutlined`, `AiProfileOutlined`, `AiSettingOutlined`,
  `AiDashboardOutlined`, `AiClockCircleOutlined`, `AiSearchOutlined`, `AiReloadOutlined`).
- **Gap — MEASURED:** Ant Design has no sun/moon. `grep -oE 'Ai(Sun|Moon)[A-Za-z]*' icondata_ai-0.0.10/src/lib.rs`
  → empty. So the theme toggle needs a second icon data crate (`icondata_mdi`: `MdiWeatherNight`,
  `MdiWhiteBalanceSunny`; or `icondata_lu`) — data only, no runtime weight — or the `AiBulbOutlined`
  fallback. `web-ui/Cargo.toml` currently names only `thaw`, so whichever crate is used must be declared
  there (an undeclared transitive import is not a dependency).

Constraint: an icon-only control still needs an accessible name — the current text label is what the
wave-1 gate's R14 pair relies on for the nav; do not trade a visible label for an unlabelled glyph.

---

## G. "A serious web dashboard" — the umbrella, needs design

Beyond A–F: one accent colour and a neutral scale instead of ad-hoc greys; a page header (title, count,
primary action) per route; consistent control heights and focus rings; per-panel loading/empty/error states
(the wave-1 `.panel-loading` marker exists — reuse it rather than inventing a second mechanism); pagination
that says "1–20 of 214" rather than bare Previous/Next; a sticky filter bar on the memories screen.

---

## Acceptance criteria

1. **No value the UI can send produces a 400** from `/api/memories` or `/api/search` — enforced by a test
   that runs the UI's own request spec (`list_memories_spec` / `search_spec`) against the server or against
   the guard, not by care.
2. A chip click filters, the total reflects the filter, and the filter is visible and removable.
3. `/tags` renders tags with counts; an empty group is not rendered; `User (0)` appears as nothing (or is
   explained) rather than as a zero row.
4. `/sessions` has no free-text project id; the picker lists the same roster `/agents` shows.
5. One measurement table at 640 / 1280 / 1600 / 1920 showing widths, `docH` and no horizontal overflow.
6. Every icon-only control has an `aria-label`; the nav landmark and `aria-current` from wave 1 are intact.

## Open questions (owner)

- **A1 or A2?** (free-text tag filter vs container-tag picker) — A1 is the only one that delivers B.
- Should `/tags` show free-form tags as a third group, or stay a scope roster?
- Is user scope dead code on this install? No `users/` shard has ever been created, so either nothing ever
  captures user-scope memories or the path is unexercised. Worth answering before designing a UI around it.

## Not in scope

Wave 1's shell/theme work (issue 118, landed and live); wave 3's three worst screens unless the architect
folds them in; any change to the auth model (`?mode=ro` reads only while diagnosing).
