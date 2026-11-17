# 135 — the `/agents` project select has no accessible name

Status: done — 2026-11-17 — delivered in wave 5; verification under `## Outcome`.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

The project picker on `/agents` must be labelled like its siblings.

Sessions renders `<label for="sessions-project-select">Project</label>` and the profile card renders `Identity`; `/agents` renders a bare `<select>` with an id but no label and no `aria-label`, so the accessibility tree reports an empty name.

Add a visible label bound to the control's id, using the same shape as the two existing precedents. The control must stay a **native `<select>`** — two live testids depend on that, and Thaw's `Combobox` would break them silently because the wasm suite has no DOM.

## Acceptance criteria

- [ ] The `/agents` project select has a non-empty accessible name in the browser's accessibility tree.
- [ ] The control is still a native `<select>` with `data-testid="agents-project-select"` unchanged.
- [ ] The label's `for` resolves to the control's `id`, and both come from one constant so they cannot drift.
- [ ] A CDP lane asserts the accessible name; a wasm test guards the constant pairing and says so, rather than pretending to prove the DOM.
- [ ] RED on today's tree: the accessible name is empty.

## Blocked by

None — can start immediately.

## Outcome (MEASURED, closed 2026-11-17)

Delta landed in wave 5, commit `70fde7e`.

- `agents_select_has_accessible_name`: `tag: SELECT`, `labels: 1`, `label: "Project"`, AX `role: combobox`, AX `name: "Project"`.
- `data-testid="agents-project-select"` unchanged; `for`/`id` derive from `AGENTS_PROJECT_SELECT_LABEL`, guarded by a wasm const-drift test (G2) that does not pretend to prove the DOM.
- Evidence: `docs/evidence/issue-118-wave5/README.md` §1 F2, §6.
