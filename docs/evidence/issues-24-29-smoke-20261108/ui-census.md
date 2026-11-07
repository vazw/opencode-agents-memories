# UI census — issues 24+29 wave (2026-11-08)

Census lane (`web-ui/scripts/capture-evidence.cjs`) against the wave's scratch
engine (port 43853, dist `web-ui/dist`, bundle id `web-ui-259e2ecdd2554e09`):
`STEADY-STATE CENSUS OK`. Every count is taken only after the route has been
idle (no Thaw spinner, no `*-loading` testid) for 3 consecutive 100 ms
samples; `busyAtCapture` must be 0.

| route | screenshot | highlights (from the run's census rows) |
|---|---|---|
| `/` (home) | `home-r2.png` | shell landing panel |
| `/project-memories` | `project-memories-r2.png` | memories pager/list present |
| `/tags` | `tags-r2.png` | 3 message bars, 1 state block (empty-roster arm) |
| `/user-profile` | `user-profile-r2.png` | profile identity select present |
| `/settings` | `settings-r2.png` | settings controls |
| `/export-import` | `export-import-r2.png` | export/import panel |
| `/skill-library` | `skill-library-r2.png` | search input + 1 state block |
| `/sessions` | `sessions-r2.png` | picker + timeline layer |
| `/agents` | `agents-r2.png` | agents select (no-projects arm: 1) |
| `/stats` | `stats-r2.png` | 4 stat cards |
| skill-form (fixture) | `skill-form-r2.png` | form open at steady state (6 inputs, 1 card) |

Additional fixtures asserted by the same lane (not screenshots — hard JSON
assertions, each emitted as a census-log JSON row):
- `fixture:memories-field-label-association` — the search Field label
  association (`searchControlName=q`, uuid pair resolving:
  `labelForResolves=true`).
- `fixture:sessions-arms-<name>` — one JSON row per sessions arm fixture:
  `loading-to-rows` (loading→rows: `rows=2, loading=0`), `roster-unavailable`,
  `roster-error`, `timeline-error`, `timeline-empty`.

Raw census rows (one JSON line per route) are in the wave's run log; the
lane's own stdout is reproduced in `.tmp-census.log` of the wave scratch
(non-tracked) and summarized here. Screenshots live beside this file (11
PNGs, `*-r2.png` naming per the lane: the 10 routes plus the
`skill-form-r2` fixture shot; the fixture assertions above print JSON rows,
not images).
