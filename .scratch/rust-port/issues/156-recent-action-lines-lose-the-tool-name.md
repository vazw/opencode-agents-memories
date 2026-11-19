# 156 — live recent-action lines all read `unknown: <input>`, because the V2 host names a tool part `name`, not `tool`

Status: ready-for-human — 2026-11-19 (FIXED on main at `bd477b2`, merged from `d684da0`; blueprint `blueprint-20261119-1119-issue-156-and-150-observability-r4.md`, gate-approved at `cd6843f1c9f0`; handoff and review are in `.tmp/progress/`, counts there as measured output. Rows captured before the fix keep their `unknown:` lines — no migration. Live confirmation needs one capture after the host reloads the plugin, i.e. the next restart.)

*(Filed from the live post-restart verification of the staged build `5a4622b05f6ad62d…` on 2026-11-19 ~14:31 +07. Issue 149 shipped the section; the V1 harness cannot see this because it feeds the V1 part shape.)*

## What was measured

The engine is correct end to end. On a reflinked copy of the live store, the staged binary answers `inject` for a real
session (`ses_e0cdbe005ffeNSOQOQUSVpvlez`, project `/home/opencode/Code/Work/antwall`) with a **4083-byte block** whose
`<project_agent_session_history>` carries `## Recent actions` and **14 action lines** — 3 memories alongside them.

Every single line starts with the same word:

```
unknown: command: "cd /home/opencode/Code/Work/antwall && B=.tmp/scratch/review-fixwave9/game-drill.base.sh N…
unknown: command: "cd /home/opencode/Code/Work/antwall && grep -cF 'probes/progs-jumptable.log\" 2>&1 || true…
unknown: path: "/home/opencode/Code/Work/antwall/.tmp/progress/20261119-issue40-game-drill-fixwave9.ledger.md…
unknown: pattern: ".tmp/**/*.png", path: "/home/opencode/Code/Work/antministrator", hidden: true
```

`unknown` is the fallback, not a tool name. The section delivers `bash`/`read`/`glob` calls the child can no longer
identify — the value the feature exists to carry is gone at the first token.

The same `unknown:` values are already persisted on the live rows captured since the restart (14:42, 14:45, 15:37,
16:24, 16:59 +07), so this is not a rendering bug: the name is lost at capture time.

## Cause

`plugin/src/plugin.js:1779` reads the tool name from the V1 message shape:

```js
const name = part.tool || "unknown";
```

The V2 host's assistant tool part is built by `packages/core/src/session/message-updater.ts:316-321`:

```ts
SessionMessage.AssistantTool.make({
  type: "tool",
  id: event.data.id,
  name: event.data.name,          // ← the name lives here
  time: { created },
  state: ToolStateStreaming.make({ status: "streaming", input: "" }),
})
```

`part.tool` is undefined on every live part, so every capture records `name: "unknown"` and the engine faithfully
persists and renders it. The V1 path (`plugin/tests/harness/bridge.mjs`, `parseIdleTranscript`'s own unit fixtures) uses
`part.tool`, which is why 257/257 passes while the live output is unusable.

`part.state.input` is present and correct: it arrives as an object and is flattened to `key: value` pairs
(`plugin.js:1785-1792`), which is where `command: "…"`, `path: "…"` and `pattern: "…"` come from.

## The fix

One line, plus the assertion that keeps it:

```js
const name = part.tool || part.name || "unknown";
```

Both shapes then work: V1 fixtures keep their `tool`, the live host supplies `name`.

## What would settle it as measurement, not inference

- Capture a live child with at least one tool call after the fix, then inject for that session and read the section:
  the first token of each line must be `bash`/`read`/`grep`/`glob`, never `unknown`.
- The inverted world: if `part.name` is also absent in the host's `ctx.session.context()` content, the lines stay
  `unknown:` and the cause is one layer higher (the legacy client's `legacyMessage` spread, `legacy-client.js:76-82`),
  which would then need the name carried explicitly rather than spread.

## Why not fixed here

The one-line change is trivial; closing it as a silent edit is not. It alters what every captured action line says,
`plugin/dist/plugin.js` must stay byte-identical to `src`, and the harness needs a V2-shaped fixture or the fix ships
unwatched — the exact failure this issue exists to name.
