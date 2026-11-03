# 118 — the web UI draws 1 of 58 Thaw components: a hand-rolled design system sits beside the framework

Status: ready-for-agent — **wave 1 (theme + shell) is blueprinted and awaiting the Plan Gate**
(`.tmp/blueprints/` `…issue-103`-style file will be linked here on approval; the assessment that
produced this issue is `.tmp/progress/webui-thaw-assessment-20261103-0955.md`). Waves 2 and 3 are
scoped below but not yet planned.

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
