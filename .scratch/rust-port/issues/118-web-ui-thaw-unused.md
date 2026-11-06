# 118 — the web UI draws 1 of 58 Thaw components: a hand-rolled design system sits beside the framework

Status: ready-for-human — **waves 1 (theme + shell), 2 (controls + states) and 3 (the three worst
screens) are built and live-verified; wave 3 merged to main at a21eb2d on 2026-11-06 (review 2 APPROVED
after review 1 caught a frozen Sessions timeline layer — untracked reads + a once-invoked closure — and
stale wave-2 evidence, both fixed with a permanent Sessions arms lane guard in `capture-evidence.cjs`).
`.tmp/blueprints/blueprint-20261106-issue-118-webui-wave3-worst-screens-r2.md`, stamp
`sha256=36ff0e12949c`, worktree `issue-118-w3` @ branch `issue-118/webui-wave3-worst-screens`) rebuilt
Memories, Settings and Sessions: the Memories card list moved to thaw `CardHeader`/`CardFooter` with
the pager inside the Ready arm, two new pure oracles (`page_after_shrink` with a div_ceil ceil-vs-floor
discriminator, `memories_empty_copy`), `PREVIEW_CHARS` 700→320, the search input wrapped in a thaw
`Field` with the explicit id DROPPED so the Field uuid is both the control's id and the label's `for`
(CDP-verified — an explicit id strands the label silently), and the action-error as an Error
`MessageBar`. Settings' ~900-char "By type" comma paragraph became a wrapped tag cloud (oracle
untouched) and the maintenance armed notice became a Warning `MessageBar` with a loading action
button. Sessions split into distinct roster arms (`sessions-roster-loading`/`-no-projects`/
`-unavailable`/`-error`) + timeline arms (`sessions-loading`/`-empty`/`session-error`) with a visible
`<label for>` picker label and the orphaned `Tag` swapped for `Badge appearance=Outline color=Warning`.
The page size is MEASURED, not preferred: the CDP `docHeight` at a 1279×800 CSS viewport read
3835 px at page size 10 and 3128 px at 8 (both > the ~2500 px bar), so `MEMORY_PAGE_SIZE = 8` stands
per the blueprint's else-branch. Class census `emitted=179 hooks=179 missing=0 dead=0` (pre-commit
floor stays 179); wasm suites green (60 core_screens + 20 screens); both web-ui and both root clippy
gates green; F4 finding recorded: thaw 0.4.8 `MessageBar`/`Badge` spread NO attrs, so both use the
blueprint's wrapper-div testid fallback. Evidence + the lane's absolute-storage-root requirement:
`docs/evidence/webui-wave3-worst-screens/README.md`. Wave 2 (blueprint `.tmp/blueprints/blueprint-20261105-1525-issue-118-webui-thaw-wave2-controls-r2.md`,
`blueprint_sha256: def66e3174bf`) replaced the raw controls with Thaw `Button`/`Input`/`Card`/`Tag`
and one `StateBlock`, and pruned the matching bespoke CSS in the same steps. Live on a disposable
engine 2026-11-06 (port 49384, fresh store): `raw <button>` **0** on all ten routes, class census
`emitted=179 hooks=179 missing=0 dead=0 unresolved=0`, token census `missing=0 dead=0`, wasm suite
green, both web-ui and both root clippy gates green. The two always-rendered native selects
(`profile-identity-select`, `sessions-project-select`) measure 1; `agents-project-select` renders only
with a populated project roster (empty roster shows `agents-no-projects`) and is unchanged at source.
Wave 2 also fixed a regression its own control swap introduced: Thaw buttons default to
`type=submit`, so `skill-save`/`skill-cancel` now carry `button_type=ButtonType::Button` and the form
prevents its implicit Enter submission — browser RED (`Save must not submit form — 1 !== 0`) → GREEN
(`PASS Save, Cancel, Enter`). Review-1 also caught that every `StateBlock` container rendered only the
caller's class (`class={let _ = STATE_BLOCK_CLASS; cls}` discarded the common class, so the `.state-block`
spacing never applied while the dead-class census still counted the token); fixed with
`class={[STATE_BLOCK_CLASS, cls.as_str()].join(" ")}` and pinned by
`web-ui/scripts/test-state-block-cdp.cjs` (RED `FAIL StateBlock container must carry .state-block` →
GREEN, classList `["state-block","state-empty"]`, 12 px margins, `/agents` loading arm included). The
route census is now readiness-gated (`web-ui/scripts/capture-evidence.cjs`), which is why the `/stats`
row reads 4 cards rather than a mid-load 0. Evidence: `docs/evidence/webui-wave2-thaw-controls/README.md`.

## Observation (MEASURED 2026-11-03, chrome-devtools MCP against the live engine at 127.0.0.1:3742)

```
Thaw components imported by web-ui/src:        1 of 58   → `use thaw::ConfigProvider;` (shell.rs)
Thaw widget elements rendered, all 10 routes:  1         → document.querySelectorAll('[class*=thaw-]').length
Replacing them:  24 raw <button> · 8 <input> · 3 <select> · 2 <textarea> · 2 <table>
                 166 distinct bespoke class strings · web-ui/styles.css = 1,565 lines
theme.rs:        configures Thaw tokens; nothing reads them
Live at once:    two palettes (--dark-* / --light-*) and two font stacks
```

Every control on screen is a browser default with custom spacing around it. That is the whole
explanation for "the UI looks plain": the app has a design system, and it is not Thaw.

Screenshots: `.tmp/scratch/webui-audit-20261103-0950/` (10 PNGs + `SHA256SUMS`, gitignored).

Also measured, and not a matter of taste: **a white band under the shell on tall pages** (Memories
renders a 6,624 px page).

## Why this is worth a wave rather than a restyle

- **`theme.rs` is the lever.** Building the Thaw theme from the palette the screens were already tuned
  against gives exactly one token source and turns every `Button`/`Input`/`Card`/`Tag` into a real Thaw
  widget — no component-by-component restyling.
- **The plainness is structural, not cosmetic.** Swapping classes for Thaw components without
  reconciling the theme would leave two palettes and two font stacks, which is the state that produced
  the complaint.
- **Three screens read badly enough to be user-facing defects:** Memories (6,624 px, card-per-memory
  and pagination wanted), Settings ("By type" is one ~900-character comma-joined paragraph), Sessions
  (asks the user to type a project id by hand — the one item that also touches data flow).

## Waves

- **Wave 1 (being planned) — one token source and a Thaw shell.** Reconcile the theme; rebuild the
  header/nav with `Layout` / `LayoutSider` / `NavDrawer`; remove the white band. Prerequisite for
  everything else, and the wave that changes every screen at once.
- **Wave 2 — controls and states.** `Button`, `Input`, `Field`, `Card`, `Tag`, `Select` for the three
  native dropdowns; a shared `StateBlock` over `Skeleton` / `Spinner` / `MessageBar` replacing five
  ad-hoc loading/empty/error forms.
- **Wave 3 — the three worst screens.** Memories, Settings, Sessions, in that order.

## Constraints (they decide the design, so they belong in the issue)

- **Stack frozen by ADR-0003:** leptos 0.7.x + thaw **0.4.8**, CSR, wasm-only, `web-ui/` excluded from
  the cargo workspace. No bump, no new dependency — and without `icondata`, Thaw's `Icon` is out.
- **Thaw 0.4.8 has no `DataTable` and no `Dropdown`.** `Table` is primitives only.
- **`Select` renders a native `<select>`; `Combobox` does not.** Two live testids depend on that:
  `select[data-testid="profile-identity-select"]` and `select[data-testid="agents-project-select"]`.
  `Combobox` would break both silently — the wasm suite has no DOM, so nothing would catch it.
- **`web-ui/scripts/check-class-coverage.mjs` fails whenever class sites change**, so a removed class
  must lose its CSS rule in the same step.
- **Tests are wasm-only under the Node runner, no DOM** — oracles stay pure functions. Rendering
  evidence is the **chrome-devtools MCP**, never a raw headless spawn.

## Acceptance criteria (falsifiable, not aesthetic)

Wave 1:
- One palette and one font stack in `web-ui/` — the count of `--dark-*`/`--light-*` definitions and the
  count of distinct `font-family` stacks are stated before and after, and the "after" is 1 each.
- Thaw widget elements on a rendered route exceed 1, with the number and the route named.
- The white band is absent at a stated page height, shown by screenshot at the same route that exposes
  it today.
- `check-class-coverage.mjs` passes at its stated count; the wasm suite is green; `trunk build` clean.

All waves:
- The two `data-testid` contracts still resolve to native `<select>` elements.
- Every behavioural oracle stays a pure function the Node runner can execute without a DOM.
- A bad wave is undone by staging the previous UI bundle back — no `rm -rf`, and the served bundle at
  `plugin/engine/linux-x64/web-ui/dist` is never replaced in place.

## Related

- `docs/adr/0003-web-explorer-ui-stack.md` — the frozen stack and why.
- `.tmp/progress/webui-thaw-assessment-20261103-0955.md` — the full per-screen critique and the
  16-row effort/risk table this issue compresses.

## Wave 1 outcome (MEASURED 2026-11-03, `web-ui/dist` served at 127.0.0.1:8123 through the chrome-devtools MCP, 1280×800, isolated context)

| Claim | Before (`:3742`, bundle `52c1081f…`) | After (preview, own build) |
|---|---|---|
| `[class*=thaw-]` elements, 10 routes | 1 | **39** on every route |
| Thaw components in `shell.rs` | 1 | 7 (`ConfigProvider`, `Layout`, `LayoutSider`, `NavDrawer`, `NavDrawerHeader`, `NavItem`, `Button`) |
| literal colours / `thaw` in `styles.css` | 20 / 0 | 0 / 0 |
| `html` background | `rgba(0, 0, 0, 0)` | dark `rgb(20, 23, 28)`, light `rgb(247, 248, 250)` |
| `.thaw-config-provider` bg / type | `rgb(41, 41, 41)` (stock) / 14px/20px | `rgb(28, 32, 39)` dark, `rgb(255, 255, 255)` light / **14px/20px** |
| `nav[aria-label="Primary"]` | **0**; nav attribute list `["class","attr:aria-label"]` | **1**, `["aria-label"]`, `colonAttrs []` |
| nav item selected / `aria-current` | 0 / 1 | 1 / 1 |
| scrollbar container `clientHeight === scrollHeight` | n/a | equal on all 10 routes; 5000 px injected block: `slack 4488`, sider and shell bottoms both 5288.1 |
| wasm suite / class census | 185 / `emitted=185` | **196** / `emitted=187 hooks=187 missing=0 dead=0 unresolved=0` (184 after the stylesheet step) |

The labelling defect the wave fixes (R-E): `shell.rs:59` shipped `<nav class="app-nav" attr:aria-label="Primary">`,
which this build renders as an attribute *named* `attr:aria-label`, so the nav never had its landmark.

Not judged here (needs the live data, after the operator stages the bundle): the ±2 % `docH` pair against
`before.json`, `slack > 0` on the four tall routes, and the two `data-testid` selects (R11) — the preview has
no API. Note `.githooks/pre-commit` still passes `--min-emitted=185`; raising it is outside this wave's footprint.

