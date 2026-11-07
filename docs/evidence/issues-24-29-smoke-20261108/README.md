# Evidence — issues 24+29 acceptance wave (2026-11-08)

Blueprint: `blueprint-20261108-issues-24-29-smoke-r2` (post-fix stamp `ea779f64ee9f`, SYS-APPROVED).
Worktree `smoke/issues-24-29` at `868d3ee`. Work left uncommitted for the
integrator (commit+push deferred per orchestrator instruction).

## What was measured

- **41-row REST live smoke** (issue 24 AC2) by `scripts/smoke-rest-surface.sh`
  (tracked) against a fresh scratch engine: verdicts derived at run time from
  the pre-registered table; **41/41 ASSERTED, 0 FAIL, 0 MISSING, 0 seed
  failures** (awk recount: total=41 ASSERTED=41 MEASURED=0 BLOCKED=0). Auth
  walk 38/38 gated rows → 401 token-less. Binary under test sha256-12
  **fbe5443f40a9**; scratch root `.tmp/scratch/smoke-24-29/20261107T193807Z/`
  (worktree-local; disposable).
- **Live 3742 GET-only corroboration pass** (`live-readonly-pass.md/.log`):
  11 GET calls, zero non-GET (log-enforced self-check), populated arms —
  memories rows 20, `/api/search?q=probe` 200 with real hits, real project
  roster (`sessionCount=347`, live session rows). Live token read from its own
  root, logged as length + 4-char prefix only, never printed. Live binary
  currency re-verified (not inherited): pid 15645, staged binary c6646bbe2f63,
  staged 2026-11-07 23:54 after HEAD `868d3ee` (23:29+07).
- **Per-screen UI smoke** (issue 29 AC3/AC4) on the same dist
  (`web-ui-259e2ecdd2554e09`, wasm sha256 `38dedc58…`): census
  `STEADY-STATE CENSUS OK`; TAGS 5/5, PILL 11/11, DIALOG 14/14 (all
  `LANE_EXPECT=post` rc=0); sessions-arms 5 fixtures OK; 11 PNGs; wave probe
  (`ui-probes.md`): 0 planned notices, 0 console errors, 0 foreign-origin
  requests on all 10 routes on BOTH scratch and live 3742.
- **Gates:** wasm 238/0; fmt rc=0; both clippy gates rc=0; workspace tests
  2483/0 (no `.rs` changed).

## What would falsify each claim

- *Rows assert the documented contract:* any row answering outside its
  pre-registered status set or without its expected substring STOPs the run
  and prints the raw response (`rest-run.log`) — the harness cannot mark a
  failing row green without editing the expectation table itself.
- *The derivation logic is real:* `SMOKE_SELFTEST=1` includes a
  deliberately-wrong control (expect 418, actual 200) that must FAIL; a
  short-circuited verdict function exits non-zero. RED was recorded first
  (derivation absent → 9/9 cases fail).
- *The live store was never mutated:* the run log must contain only
  `LIVECALL GET` lines; the self-check re-greps it at exit.
- *The UI has no planned route, no foreign request, no console error:* the
  probe fails on any nonzero count on any route on either target.
- *Lane contracts hold:* any lane FAIL/STOP row (including the CONTROL rows)
  exits 1.

## Step-4 pre-run derivations (recorded before any live call)

**(a) skills POST→catalog roundtrip** — POST `/api/skills` with
`{"id":"ship-it","name":"Ship It","description":"Wave smoke ship-it
skill","content":"## Ship\n\nShip the wave."}` and `enabled` omitted stores
`enabled = !matches!(input.enabled, Some(false))` = **true**
(`crates/mem-skills/src/manager.rs:354`); `GET /skills/index.json` lists
enabled-only entries with `files:["ship-it.md"]` (`manager.rs:472-478`);
`GET /skills/ship-it/ship-it.md` → `catalog_markdown` answers `None` for an
unknown **or disabled** id (`manager.rs:487-494`), else `format_markdown` =
frontmatter (quoted name/description, `opencode/autoinvoke`) + one blank line
+ content verbatim (`crates/mem-skills/src/content.rs:53-73`). Consequence
pinned: rows 34/35's subs are derived from the POST literal (row 35 asserts
the `## Ship` content verbatim; the row-34 `skills` sub comes from the
document's `<skills>` framing, `skills.rs:4`), and any `enabled:false` in the
POST body would 404 row 35.

**(b) probe-b seed lifecycle** — POST `/api/memories` (content names probe-b;
canonical tag `opencode_user_c1c2c3d4e5f67890` from
`crates/mem-web/tests/shard_model_authority.rs:35`; tags
`["smoke-wave-24-29"]`) between rows 5 and 6; teardown DELETE after row 12.
Both are harness **seed actions, not table rows**; a failed seed STOPs the
run (the rows it gates go undispositioned, exiting non-zero anyway). Row 4's
POST is probe-a's seed (its response id feeds row 5).

**Embedder cache location (measured during the wave; corrects the
blueprint's `<root>/.cache` plan):** the engine resolves the embedder cache
at the resolved config's storage path `$HOME/.opencode-mem/data/.cache`
(`engine.rs:1548` → `composition.rs:419` → `config/mod.rs:342`); the CLI
`--storage-root` overrides the store's path only (`main.rs:188`). The
harness provisions the hardlink copy under the sandbox HOME's data dir —
exactly where a real deployment keeps it. Proven in-process: the mem-embed
golden test passes against both the host cache and the scratch copy, and an
embed probe with the engine's exact options answers `OK dims=768` on the
scratch copy. (The plan-gate r2 approved provisioning `<root>/.cache`; the
live run STOPped on row 4 and the diagnosis moved the copy — an expectation
was never edited.)

**D-11 wildcard (derivation correction, measured):** a second engine on
`--web-host 0.0.0.0` **refuses its own web launch** (`web.launch_failed`, no
socket): `assert_network_auth` (`crates/mem-web/src/api_auth.rs:30-46`)
fail-closes a non-loopback bind with no API token configured, and the root
launcher hardcodes `api_token: None` (`src/main.rs:176`; the
`webServerApiToken` config field has no consumer). The pre-registered
"second engine answers 401 token-less" is unreachable through the production
launcher; the refusal is the live wildcard-half evidence (an always-public
world would launch and serve), and the 401/200 contract is pinned at the
router seam (`crates/mem-web/tests/api_route_auth.rs:510-540`, green in the
workspace run).

## Artifacts in play

| file | what it is |
|---|---|
| `rows.tsv` | 41 rows, tab-separated: num, method, path, expected statuses, expected sub, **verdict (field 6)**, live code, body excerpt |
| `rest-run.log` | full harness run log (row-by-row verdicts, auth walk, D-11, awk recount, teardown) |
| `live-readonly-pass.md` / `.log` | the GET-only live pass report + raw call log (self-checked) |
| `ui-census.md` | census summary + the 11 route/fixture screenshots (PNG, this dir) |
| `lane-tags.md`, `lane-pill.md`, `lane-dialog.md` | LANE_TABLE outputs (post, 0 mismatches) |
| `ui-probes.md` | wave probe output for both targets |
| `README.md` | this file |

Scratch engine conventions: serial spawns, absolute `--storage-root`, sandbox
`HOME`, isolated `OPENCODE_MEM_LOG_FILE`, FIFO stdin guard, bounded SIGTERM
watchdog, kill-and-reap on every exit path, ephemeral ports, token from
`.auth-token` (never printed, never committed).
