# Web identity + project picker — live evidence (2026-11-03)

Operator evidence for the two web-surface fixes landed by issues 115/116
(blueprint `1ec2b30cc56d`, commits `73a964f`…`a4afb39`). Everything below was
measured against the **live** engine and the **live** store, not a fixture.

## Deployment state measured

| Item | Value |
|---|---|
| Engine pid / exe | `17301`, sha256 `9e301c19…` (built from `a4afb39`) |
| Launch cwd | `/home/vaz` (readlink `/proc/17301/cwd`) |
| Web bind | `127.0.0.1:3742` |
| `episodic.db` `user_version` | **7** (issue 110's `not_before` applied here for the first time) |
| `capture_inbox` columns | `… attempts, last_attempt_at, status, not_before` |
| Backup taken before the first v7 start | `~/.opencode-mem/backups/20261102T2349Z/` — 32/32 DBs `integrity_check ok`, every table of the six root DBs matched live, `VERIFICATION.md` present |

## O1 — the identity is the inject path's resolver

Expected value derived from the engine's own launch directory, never a literal:

```
$ git -C "$(readlink /proc/17301/cwd)" config user.email
vaz@siamstr.com

$ curl -s -H "x-opencode-mem-token: …" http://127.0.0.1:3742/api/user-profile
{"success":true,"data":{"exists":true,"userId":"vaz@siamstr.com","displayName":"vazw",
 "version":170,"totalPromptsAnalyzed":1430,…}}
```

`exists: true` and `userId ==` the command's output. Before this wave the same
request answered `exists:false, userId:"unknown"` while a stored profile existed
for that identity: `SELECT length(profile_data), version, total_prompts_analyzed
FROM user_profiles WHERE user_id='vaz@siamstr.com'` (read-only) →
**525413 B, v170, 1430 prompts**.

## O1 — the roster endpoint

```
$ curl … /api/user-profiles
[{"userId":"vaz@siamstr.com","version":170,"totalPromptsAnalyzed":1430,"isDefault":true},
 {"userId":"jakkaphat.j@bangmod.co.th","displayName":"Jakkaphat Jaisen","version":47,
  "totalPromptsAnalyzed":330,"isDefault":false}]
```

`isDefault` follows the composed identity (`crates/mem-web/src/api/profile.rs`:
`is_default = profile.user_id == state.user_id`). This payload alone cannot show
that — the default is also row 0 here — so the by-identity rule is pinned by
`crates/mem-web/tests/api_profiles_roster.rs`, whose fixture composes a DIFFERENT
identity from `profiles.first()` and reds an `index == 0` implementation.

## O2 — the profile screen (chrome-devtools MCP)

`/user-profile` renders `identity: vaz@siamstr.com` and a native picker
(`select[data-testid="profile-identity-select"]`, testid `profile-identity-select`) with

```
— choose an identity —
vazw — 1430 prompts                     (selected)
Jakkaphat Jaisen — 330 prompts
```

Network on load: `GET /api/user-profiles` → 200, then
`GET /api/user-profile?userId=vaz%40siamstr.com` → 200.

Selecting the second identity drives a real switch:

```
before: vazw (version 170)   after: Jakkaphat Jaisen (version 47)
identity line: identity: jakkaphat.j@bangmod.co.th
GET /api/user-profile?userId=jakkaphat.j%40bangmod.co.th → 200
```

## O2 — the agents screen (chrome-devtools MCP)

`/agents` renders the project dropdown `select[data-testid="agents-project-select"]`
with one option per `GET /api/projects` row (11 rows when O2 ran), each carrying
its own counts, e.g.

```
opencode-agents-memory — 72 sessions · 0 milestones
leninguard — 6 sessions · 0 milestones
antwall — 7 sessions · 0 milestones
```

Those counts are the O2 run's; they move with live traffic. Re-derived at
2026-11-03T06:24:14Z the same endpoint answers **12 rows** (`/home/vaz/.config/nvim`
appeared after the O2 run: 1 session whose newest `createdAt` is
1793684242492), and the three quoted rows read **73 / 6 / 8** sessions. The label
shape (`"{label} — {n} sessions · {m} milestones"`) and the `0 milestones` on every
row are both unchanged.

Network: `GET /api/projects` → 200; the default selection then fetches
`GET /api/projects/%2Fhome%2Fvaz%2FCode%2FPersonal%2FRust%2Fopencode-agents-memory/milestones`
→ 200, i.e. percent-encoded project ids work end to end against the live router.

The `0 milestones` on every row is the consolidation age window, measured rather
than assumed. The run below carries its own clock — the label is the query's
`now`, not a note taken from a shell (read-only, `file:…?mode=ro`):

```
SELECT MIN(created_at), MAX(created_at), COUNT(*),
       CAST(strftime('%s','now') AS INTEGER)*1000 AS now_ms
  FROM agent_sessions WHERE deleted_at IS NULL;
→ 1793563141278 | 1793684858627 | 130 | 1793686601000      -- now_ms = 2026-11-03T06:16:41Z
cutoff (now_ms − 30 d) = 1791094601000                     → MIN(created_at) > cutoff
SELECT COUNT(*) FROM episodic_milestones;                  → 0
SELECT COUNT(*) FROM agent_sessions WHERE deleted_at IS NOT NULL; → 0
```

The oldest live session is 1.43 days old and the newest is minutes old, so none of
the 130 is eligible and the age window alone produces the empty roster — the
writer is not idle-by-flag. The pair drifts with traffic (127 / 1793662774787 in
review-4's run, whose own query clock read `1793683644415` = 2026-11-03T05:27:24Z),
so the two runs are identified by their clocks rather than by a shared label; the
**oldest** stamp is the number that matters and it is identical in both.

The cross-tier promotion gate is **not** a cause on this host:
`featureFlags.promotionPipeline` is unset, so the default `false`
(`crates/mem-core/src/config/schema.rs:632`) composes `NoopCrossTierTask`
(`crates/mem-bridge/src/cross_tier.rs:190-193`), whose `promotion_due` returns
`true` (`crates/mem-episodic/src/cross_tier.rs:96-101`) — a flag-off install can
never skip. Only the flag-on `FactsCrossTierTask` gates, on `updated_at`
(`crates/mem-bridge/src/cross_tier.rs:106-113`), and no session here reaches it.
Issue 117 records this run.

## Deployment note — the staged UI is a separate artifact

The first restart served the **previous** UI: `plugin/engine/linux-x64/web-ui/dist`
held a bundle built 2026-11-01T22:27:34Z (02 Nov 05:27 +07 — its wasm's own mtime)
whose wasm contained no `/api/user-profiles` call (0 occurrences), so the profile
page rendered but the picker never mounted and the browser never requested the
roster. Staging the engine binary alone is not a release.

The bundled UI was staged the way `plugin/scripts/stage-engine.sh` does it —
old bundle moved aside to `web-ui/dist.old-20261103T0440Z`, then
`cp -R ../../../web-ui/dist/. web-ui/dist/` — after which all four files
byte-matched `web-ui/dist` (`diff -rq` silent when that staging was made) and the
staged wasm carried `api/user-profiles`.

Re-measured 2026-11-03T05:17:33Z (the build's own file mtimes), after the
empty-state copy change (`8622731`) rebuilt `web-ui/dist`: the staged bundle was
then `web-ui-e16b064079341384…` and the fresh build `web-ui-52c1081f644725c6…`;
`diff -rq` reported `index.html` differing and both hashed asset pairs `Only in
…` — the same failure mode one wave later, and the reason the recipe below is
written down rather than remembered. Re-staged at 05:24Z (the
`dist.old-20261103T0524Z` stamp) and re-measured at 2026-11-03T06:17:22Z, the
clock printed in the same command: `diff -rq` between `web-ui/dist` and the
staged copy is **silent** (four files byte-identical, `web-ui-52c1081f644725c6…`),
and the live server serves that bundle — the recipe below is the one that was
used.

The repeatable recipe (run from `plugin/engine/linux-x64`, where `web-ui/dist` is
the staged bundle and `../../../web-ui/dist` is the fresh trunk build):

```sh
mv web-ui/dist "web-ui/dist.old-$(date -u +%Y%m%dT%H%MZ)"   # move aside; never delete
mkdir -p web-ui/dist
cp -R ../../../web-ui/dist/. web-ui/dist/
diff -rq ../../../web-ui/dist web-ui/dist                   # must print nothing
```

`diff -rq` is the check that matters: the four entries must be the same four
files **and** byte-identical — `index.html`, `styles-<hash>.css`,
`web-ui-<hash>.js` and `web-ui-<hash>_bg.wasm` — so a partial copy or an orphaned
hashed asset shows up as `Files … differ` / `Only in …` rather than as a UI that
silently keeps serving the old wasm.

`plugin/scripts/stage-engine.sh` **refuses** to overwrite an existing staged
bundle (`plugin/scripts/stage-engine.sh:77-79`: `'…' already holds a staged
bundle; bundles are replaced, never merged (trunk hashes asset names)`, then
`exit 7`). An engine-only re-stage and a UI re-stage are therefore two different
operations: the binary can be refreshed by the script, the UI must be moved aside
and re-copied by hand — skipping the second step ships yesterday's UI, which is
exactly what the first restart above did.

## Every figure, its scope, and the command that produced it

A figure in this file must come out of the command named beside it, over the scope
named beside it — the two failures this section exists to prevent are an instant
copied from prose and a value lifted from another scope. Re-derived read-only at
2026-11-03T06:24:14Z (`episodic.db` / `user-profiles.db` via `file:…?mode=ro`).

| figure | scope | command | reproduced |
|---|---|---|---|
| blueprint stamp `1ec2b30cc56d` | the blueprint file | `sed 1,/^---$/d .tmp/blueprints/…-r3.md \| sha256sum \| cut -c1-12` | `1ec2b30cc56d` |
| commit range `73a964f`…`a4afb39` | this wave's commits up to the evidence commit | `git log --oneline a4afb39 ^5956bcb` | **10** commits (6 feature + 3 rework + `a4afb39` docs), both endpoints present; `HEAD` extends the range by the four docs landings after it |
| engine pid `17301` | the one `opencode-agents` process | `pgrep -x opencode-agents` | `17301` |
| exe sha256 `9e301c19…` | that process's `exe` | `sha256sum "$(readlink /proc/17301/exe)"` | `9e301c194748…` |
| "built from `a4afb39`" | the staged engine binary | `stat -c %y plugin/engine/linux-x64/opencode-agents-memory` → `06:49:55 +07`, inside `[a4afb39 06:43:21 +07, 8622731 12:17:38 +07)`; `git diff --name-only a4afb39..HEAD -- '*.rs' ':!web-ui'` → **0** files | attested |
| launch cwd `/home/vaz` | that process | `readlink /proc/17301/cwd` | `/home/vaz` |
| bind `127.0.0.1:3742` | the API listener | `curl -s -o /dev/null -w '%{http_code}' 127.0.0.1:3742/api/health` | `200` |
| `user_version` **7** | `episodic.db` | `PRAGMA user_version` (`mode=ro`) | `7` |
| `capture_inbox` ends `… status, not_before` | that table | `SELECT group_concat(name, ', ') FROM pragma_table_info('capture_inbox')` | `…, attempts, last_attempt_at, status, not_before` |
| backup `20261102T2349Z`, **32** DBs, **6** root DBs | the backup tree | `find …/backups/20261102T2349Z/data -name '*.db' \| wc -l` → 32; `ls …/data/*.db \| wc -l` → 6 | 32 / 6 |
| "32/32 `integrity_check` ok", "every table matched live", pre-v7 | **the backup's own record**, not a run of mine | `grep -E 'Database files found\|integrity_check\|Row counts\|user_version' …/VERIFICATION.md` | the four lines, incl. `user_version` **6** at backup time |
| identity `vaz@siamstr.com`, `exists:true` | composed identity (no `?userId`) | `git -C /home/vaz config user.email` + `GET /api/user-profile` | `vaz@siamstr.com`, `exists:true` |
| **version 170**, **1430** prompts, `displayName vazw` | that one profile row | `GET /api/user-profile`; `SELECT version, total_prompts_analyzed FROM user_profiles WHERE user_id='vaz@siamstr.com'` | 170 / 1430 / `vazw` |
| **525413 B** of `profile_data` | the same row | `SELECT length(profile_data) FROM user_profiles WHERE user_id='vaz@siamstr.com'` | `525413` |
| roster **2** rows: v170/1430/`isDefault:true`; jakkaphat v**47**/330/`isDefault:false`, `"Jakkaphat Jaisen"` | `GET /api/user-profiles` (all active profiles) | `curl … /api/user-profiles`; `SELECT user_id, version, total_prompts_analyzed, is_active FROM user_profiles` | identical |
| `isDefault` is derived by identity | `profile.rs` + the roster fixture | `crates/mem-web/src/api/profile.rs` (`is_default = profile.user_id == state.user_id`); `api_profiles_roster`'s default-on-the-second-row fixture | by identity (this payload alone cannot distinguish it from position) |
| `/api/projects`: 11 rows at O2 → **12** now | all projects with live sessions or milestones | `curl … /api/projects` | 12 rows; **73 / 6 / 8** sessions on the three quoted rows |
| `0 milestones` on every row | same | same | 0 on all 12 |
| `.config/opencode`: **18** sessions, newest `1793618373080` | **that project only** | `GET /api/projects/%2Fhome%2Fvaz%2F.config%2Fopencode/sessions`; `SELECT MIN(created_at), MAX(created_at), COUNT(*) FROM agent_sessions WHERE project_id='/home/vaz/.config/opencode' AND deleted_at IS NULL` | 18 / `1793618373080` |
| served bundle `52c1081f644725c6…`, **4** files, byte-identical to `web-ui/dist` | `web-ui/dist` ↔ `plugin/engine/linux-x64/web-ui/dist` | `ls -1`, `diff -rq web-ui/dist plugin/engine/linux-x64/web-ui/dist` | 4 / 4, silent |
| the deployment note's three instants: `2026-11-01T22:27:34Z` (old bundle), `2026-11-03T05:17:33Z` (this build), `2026-11-03T06:17:22Z` (the diff run) | respectively: the old bundle's wasm, `web-ui/dist`'s wasm, and the diff command | `stat -c '%y' …/dist.old-20261103T0440Z/*_bg.wasm` → `2026-11-02 05:27:34.988 +0700`; `stat -c '%y' web-ui/dist/web-ui-52c1081f644725c6_bg.wasm` → `2026-11-03 12:17:33.878 +0700`; `date -u` printed with the `diff -rq` | all three derived, none copied |
| **0** occurrences of `api/user-profiles` in the pre-wave bundle | `dist.old-20261103T0440Z`'s wasm | `grep -ac 'api/user-profiles' …/dist.old-20261103T0440Z/*_bg.wasm` | 0 (the served wasm: 1) |
| the new copy is really served | the served wasm | `grep -ac 'hourly consolidation pass'` and `grep -ac 'configured age window (30 days by default)'` | 1 / 1 |
| `dist.old-20261103T0440Z`, `dist.old-20261103T0524Z` | the staged predecessors | `ls -d plugin/engine/linux-x64/web-ui/dist.old-*` | both present (+ `…1101T222753Z`) |
| `stage-engine.sh` refuses with **exit 7** | the script | `sed -n '77,79p' plugin/scripts/stage-engine.sh` | `if [ -f "$WEB_UI_DEST/index.html" ]` … `exit 7` |
| the measurement block (`MIN`/`MAX`/`COUNT`/`now_ms`/cutoff) | **all live sessions, all projects** | the `strftime('%s','now')` query above | `1793563141278 / 1793684858627 / 130 / 1793686601000 / 1791094601000` |

The one class deliberately not re-derived here is the O2 DOM and network
observation set (the `identity:` line, the picker's option text and its `selected`
state, the 170→47 switch, and the request order): those are chrome-devtools MCP
measurements of a browser session, not API reads. Their *values* are re-derived
above from the endpoints that feed them; the rendering itself is O2's record.

Screenshots: `.tmp/scratch/web-picker-profile-jakkaphat.png` (gitignored scratch,
reproducible by repeating the steps above).
