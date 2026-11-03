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
request answered `exists:false, userId:"unknown"` while a 525 KB / 1430-prompt
profile existed.

## O1 — the roster endpoint

```
$ curl … /api/user-profiles
[{"userId":"vaz@siamstr.com","version":170,"totalPromptsAnalyzed":1430,"isDefault":true},
 {"userId":"jakkaphat.j@bangmod.co.th","displayName":"Jakkaphat Jaisen","version":47,
  "totalPromptsAnalyzed":330,"isDefault":false}]
```

`isDefault` follows the composed identity, not position.

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
with 11 options, each carrying its own counts, e.g.

```
opencode-agents-memory — 72 sessions · 0 milestones
leninguard — 6 sessions · 0 milestones
antwall — 7 sessions · 0 milestones
```

Network: `GET /api/projects` → 200; the default selection then fetches
`GET /api/projects/%2Fhome%2Fvaz%2FCode%2FPersonal%2FRust%2Fopencode-agents-memory/milestones`
→ 200, i.e. percent-encoded project ids work end to end against the live router.

The `0 milestones` on every row is the 30-day consolidation age gate plus the
promotion due gate (issue 117 carries the read-only measurement that settles it),
not an empty pipeline.

## Deployment note — the staged UI is a separate artifact

The first restart served the **previous** UI: `plugin/engine/linux-x64/web-ui/dist`
held a 2026-11-02 bundle whose wasm contained no `/api/user-profiles` call (0
occurrences), so the profile page rendered but the picker never mounted and the
browser never requested the roster. Staging the engine binary alone is not a
release.

The bundled UI was staged the way `plugin/scripts/stage-engine.sh` does it —
old bundle moved aside to `web-ui/dist.old-20261103T0440Z`, then
`cp -R ../../../web-ui/dist/. web-ui/dist/` — after which all four files
byte-matched `web-ui/dist` (`diff -rq` silent at 04:40Z) and the staged wasm
carried `api/user-profiles`.

Re-measured 2026-11-03T05:17Z, after the empty-state copy change (`8622731`)
rebuilt `web-ui/dist`: the staged bundle is `web-ui-e16b064079341384…` and the
fresh build is `web-ui-52c1081f644725c6…`; `diff -rq` reports `index.html`
differing and both hashed asset pairs `Only in …`. The staged UI is therefore
stale again — the same failure mode one wave later, and the reason the recipe
below is written down rather than remembered.

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

Screenshots: `.tmp/scratch/web-picker-profile-jakkaphat.png` (gitignored scratch,
reproducible by repeating the steps above).
