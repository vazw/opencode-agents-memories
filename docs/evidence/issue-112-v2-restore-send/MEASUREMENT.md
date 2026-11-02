# Issue 112 — the V2 post-compaction restore is a write: measurement record

- **Wave:** `blueprint-20261102-0426-v2-compaction-restore-send` (SYS-APPROVED 2026-11-02T04:58Z).
- **Wrapper HEAD before the wave:** `1483813` (local; nothing pushed).
- **Host of record:** `opencode v2.2.5`, checkout `/home/vaz/Code/Personal/JS/opencode` @
  `9af7aa2da8` (`git log --oneline -1` → `9af7aa2da8 fix(tui): capture the scroll anchor before the
  navigation state clears`). Every host-source line below was read at that commit.
- **Read-only posture:** every database read is `file:$HOME/.local/share/opencode/opencode.db?mode=ro`;
  nothing in this record wrote to the host store, `~/.config/opencode/**` or the engine.
- **Labels:** `MEASURED` = a command in this file printed it; `READ` = source line inspected; an
  unlabelled figure is a claim the command above it printed.

The live `/compact` run was **owner-run after this wave shipped**, and it failed: no row, no toast.
§8 records the observation, the cause, and the fix (issue 114). The code-side re-derivation below is
unchanged by that failure — the send path was correct; the agent gate in front of it could not resolve.

## 1. Agent coverage — the premise of D-112-4 (`MEASURED`)

Run at `2026-11-02T05:11:58Z`. Each query is ONE statement with both counts as subqueries, so both
halves are read in one transaction; two separate `count(*)` calls drift against a live session.

```sh
DB="file:$HOME/.local/share/opencode/opencode.db?mode=ro"; S=ses_e9d5dd698ffe5lUV7W7s2KDpbh
Q1="select (select count(*) from session_message where type='assistant' and json_extract(data,'$.agent') is not null)||'/'||(select count(*) from session_message where type='assistant');"
Q2="select (select count(*) from session_message where session_id='$S' and type='assistant' and json_extract(data,'$.agent') is not null)||'/'||(select count(*) from session_message where session_id='$S' and type='assistant');"
Q3="select (select count(*) from session_message where session_id='$S' and type='user' and json_extract(data,'$.agent') is not null)||'/'||(select count(*) from session_message where session_id='$S' and type='user');"
sqlite3 "$DB" "$Q1"; sqlite3 "$DB" "$Q2"; sqlite3 "$DB" "$Q3"; date -u +%FT%TZ
```

Output (verbatim, in order):

```
146680/146680
5932/5932
0/429
2026-11-02T05:11:58Z
```

Interpretation: every visible assistant row carries `$.agent` (all sessions **146680/146680**; the live
session **5932/5932**), while user rows carry it **0/429**. `resolveSessionAgent`
(`plugin/src/compaction.js:56`) therefore walks assistant rows; the user branch is kept for parity but
can never be the source under this host. The denominators grow while the session is used — the ratio is
the fact, the count is only this observation.

**Retraction carried forward:** do not re-derive the user figure with `data LIKE '%"agent"%'`; that
pattern matches the `agents` array and `metadata`, not the field. The r2 figure `25/429` was that
artifact and is withdrawn.

## 2. The lane before the fix (`MEASURED` 2026-11-02T05:08:01Z, read-only)

- `session_message` for the live session, `type='synthetic'` → **1133** rows (the store grows as the
  session is used; this numeral is an observation, not a constant).
- Rows carrying `"description":"memory context"` → **318** across all sessions, **34** for this session.
  Newest: `2026-10-30 15:49:04`, text prefix `## Restored Session Memory`; none dated 2026-10-31 or
  later. So the restore demonstrably landed through 2026-10-30 under an earlier configuration; what the
  wave fixes is the V2 lane producing nothing on 2026-11-02 while its ingest leg ran.
- `part` rows with `id LIKE 'prt-compaction-%'` → **7**, newest `2026-10-11 16:14:11` (the V1 lane's
  send ids; not touched by this wave).
- Session row: `agent = main`, `model = {"id":"deepseek-v4.1-flash","providerID":"opencode-go",
  "variant":"high"}`, `parent_id` empty.
- Engine log `~/.opencode-mem/opencode-mem.log`: `Compaction memory injected` → **4**; the last two are
  `2026-11-02T11:00:11.804+07:00` and `2026-11-02T11:00:32.856+07:00`, both for the live session with
  `memory_count:10`. `compaction.restore_skipped` → **0**, `compaction.restore_failed` → **0**,
  `toast.not_delivered` → **0**.
- Host log `~/.local/share/opencode/log/opencode.log`: records with
  `component=opencode-mem-plugin` → **0** (also 0 for the JSON spelling). The wrapper's `logWarn` writes
  JSON to **stderr** (`plugin/src/plugin.js:214-221`) and the host log never captures it; the live
  oracle is the DB row plus the toast, never a log line (D-112-5).

## 3. Host source re-derivation (`READ` @ `9af7aa2da8`, v2.2.5)

The send lands on `ctx.session.synthetic`; the three facts that make the mapping exact:

- `packages/core/src/plugin/host.ts:611` — `get: (input) => sessions.get(input.sessionID),`; the same
  object has `prompt: sessions.prompt` at `:615` and `synthetic: sessions.synthetic` at `:628`.
- `packages/protocol/src/groups/session.ts:406` — `resume: Schema.Boolean.pipe(Schema.optional),` in the
  `session.synthetic` payload, described at `:414` as "Durably admit synthetic session input and
  schedule execution **unless resume is false**".
- `packages/plugin/src/promise/adapter.ts:637` —
  `synthetic: adaptApiMethod(SessionEndpoints["session.synthetic"], host.session.synthetic),`; the
  sibling `prompt` adaptor is at `:633`.
- `packages/core/src/session/session.ts:323` —
  `if (input.resume !== false && !(yield* get(sessionID)).revert) yield* execution.wake(sessionID)`.

Input shapes: `SessionPromptInput` (`packages/client/src/effect/api/api.ts:210-220`) and
`SessionSyntheticInput` (`:236-244`) carry `delivery` and `resume` but **no** `agent`/`model`/`variant`.
The only identity levers are `switchAgent`/`switchModel` (`packages/core/src/plugin/host.ts:613-614`),
and `switchAgent` publishes `SessionEvent.AgentSelected` (`packages/core/src/session/session.ts:99-105`)
which renders as a visible "agent changed" notice
(`packages/session-ui/src/timeline/session-timeline-row.tsx:509-519`, the `previous → agent` line at
`:513`). That is the cost that makes `session.model` not purchasable without a visible side effect
(D-112-4 reason 3).

## 4. Why the description string is load-bearing (D-112-2, `READ`)

Two independent renderers drop a description-less synthetic; the owner reads the terminal one:

- `packages/session-ui/src/timeline/projection.ts:1179-1185` — `isNotice` returns
  `!!message.description?.trim() || timelineNoticeRequired(message)`.
- `packages/tui/src/routes/session/rows.ts:392` — `if (message.type === "synthetic" &&
  !message.description?.trim()) return rows`; the live-append gate is at `:259-275` and tests the same
  field on `session.inbox.enqueued` / `session.synthetic`.
- Label rule: `description ?? text` (`packages/session-ui/src/timeline/session-timeline-row.tsx:574`)
  and, in the TUI, `description ?? ""` for a synthetic
  (`packages/tui/src/routes/session/index.tsx:2195`). So `description: "memory context"` is the entire
  visible label in the terminal, not a caption.

Admission vs. promotion (D-112-3): `packages/core/src/session/session.ts:298-326` admits the row to the
inbox unconditionally (`delivery` defaults `"steer"`, `:309`) and wakes the runner only on
`resume !== false`. The row reaches the visible transcript at admission
(`packages/client/src/solid/data.ts:441-471`, merged at
`packages/app/src/session/timeline/controller-projection.ts:71-118`) and its `text` enters the **model**
request only on promotion inside the runner (`packages/core/src/session/runner/llm.ts:190`,
`SessionInbox.promote`). So `resume: false` delays the model's view by one step; it does not delay the
user's.

## 5. Deployment path and the dead `./v2` export (D-112-8, `READ`)

- `packages/plugin/src/host.ts:57` — `server: entry(["server", ""])`; `:58` — `tui: entry(["tui"])`.
  `./v2` is not in that resolution list at any version. The V2 adapter is reached only by the in-process
  import at `plugin/src/plugin.js:1260` (`const mod = await import("./v2/plugin.js")`) inside the
  `setup` the host calls via `server()`.
- `packages/core/src/plugin/module.ts:147-152` advertises `features.tui` for a package whose
  `exports["./tui"]` resolves; the **TUI process** then loads it
  (`packages/tui/src/plugin/context.tsx:95-104` the `serverTuiPlugins` filter, `:292-306` the merge into
  `mergePluginTargets`, `:670` `Host.resolve(target).tui`). The server never imports it.
- The `./v2` export stays because `plugin/tests/v2-loader-contract.test.mjs:30` pins it; the finding is
  tracked separately as issue 113. This wave only corrects the misleading header comment in
  `plugin/src/v2/plugin.js`.

## 6. The toast bridge is unchanged and already in shape (D-112-9, `READ`)

`plugin/src/tui.js` is **byte-unchanged** by this wave. Its registration spelling
(`context.client.rpc(Descriptor).events.on(name, cb)`) is the live API, used in-tree by the host's own
TUI feature plugin at `packages/tui/src/feature-plugins/prompt/goal.tsx:14` and `:29`; the context field
is `readonly client: OpenCodeClient` (`packages/plugin/src/tui/context.ts:549`); `ToastOptions` is
`{title?, message, variant?, duration?, sessionID?}` (`packages/plugin/src/tui/context.ts:269-282`) and
the implementation defaults `variant` to `"info"` (`packages/tui/src/plugin/api.tsx:108-135`). Our body
`{title, message, variant: "success", duration: 3000}` survives `toastPayload`
(`plugin/src/v2/toast-rpc.js:55`). Step 7 therefore verifies the **deployment**, not the bridge.

## 7. The ratio gate (`MEASURED`, re-run after this file was written)

```sh
DB="file:$HOME/.local/share/opencode/opencode.db?mode=ro"; S=ses_e9d5dd698ffe5lUV7W7s2KDpbh
Q1="select (select count(*) from session_message where type='assistant' and json_extract(data,'$.agent') is not null)||'/'||(select count(*) from session_message where type='assistant');"
Q2="select (select count(*) from session_message where session_id='$S' and type='assistant' and json_extract(data,'$.agent') is not null)||'/'||(select count(*) from session_message where session_id='$S' and type='assistant');"
Q3="select (select count(*) from session_message where session_id='$S' and type='user' and json_extract(data,'$.agent') is not null)||'/'||(select count(*) from session_message where session_id='$S' and type='user');"
R1=$(sqlite3 "$DB" "$Q1"); R2=$(sqlite3 "$DB" "$Q2"); R3=$(sqlite3 "$DB" "$Q3")
test "${R1%/*}" = "${R1#*/}" && test "${R2%/*}" = "${R2#*/}" && test "${R3%%/*}" = "0" \
  && grep -qF "$R1" docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md \
  && grep -qF "$R2" docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md \
  && grep -qF "$R3" docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md \
  && echo "AGENT-COVERAGE OK $R1 $R2 $R3"
```

Because the denominators move with the live session, this check is a hard equality: recapture, rewrite
the three output lines in §1 with the new UTC timestamp, and re-run until it prints
`AGENT-COVERAGE OK <n>/<n> <m>/<m> 0/<k>`. A frozen count would fail the next day; that is the point.

## 8. Live verification — FAILED on `18fea68`, root-caused, fixed by issue 114

**Status: the first live run of this send produced no write.** The three code-side sections above are
unchanged by that run; what follows is the live observation, its cause and the fix, recorded here
because this file's §8 was the wave's only live oracle.

### 8.1 The observation (`MEASURED` read-only, 2026-11-02T06:24:41Z)

```sh
DB="file:$HOME/.local/share/opencode/opencode.db?mode=ro"
sqlite3 "$DB" "select count(*), sum(agent is null), sum(parent_id is not null) from session_v2;"
sqlite3 "$DB" "select seq, type from session_message
  where session_id='ses_e8839ef3fffeIiAusczdXyJPHU' and seq>=6863 order by seq;"
sqlite3 "$DB" "select count(*) from session_message where type='compaction'
  and time_created > strftime('%s','2026-11-01')*1000;"
sqlite3 "$DB" "select count(*) from session_message where type='synthetic'
  and data like '%\"description\":\"memory context\"%'
  and time_created > strftime('%s','2026-11-01')*1000;"
```

```
2639|5|2583
6863|compaction
6866|idle
17
0
```

- **Zero** `synthetic` rows carrying `"description":"memory context"` exist after 2026-11-01 — the
  newest is still `2026-10-30 22:49:04` (+07), text `## Restored Session Memory`. The send never landed.
- The engine log recorded **five** `Compaction memory injected` lines after the host restart
  (`12:31:01`, `12:41:44`, `12:43:22`, `12:43:56`, `12:46:23` +07). Those are **two ingest/restore
  pairs** (`ses_eba84b…`, `ses_e8839…`) plus one orphan ingest whose session has no completed
  compaction. Of the 17 `compaction` rows since 2026-11-01, five are dated 2026-11-02 and **three** are
  after the restart (`12:31:01`, `12:43:04`, `12:43:22`) — so the wave's denominator, corrected, is:
  **two restore injections fired, zero rows written**. The five-timestamp framing in the handoff
  counted engine lines, not compaction rows.
- At restore time the session's `session.context` was exactly `seq 6863 compaction` + `seq 6866 idle` —
  the two rows above, both answers to a query, not a reconstruction of what the wrapper saw (the
  restore injection at `12:43:56.167` is in the same second as the `idle` row).
- `session_v2` has drifted since the wave's plan (`2639` rows, `5` without `agent`, `2583` children, vs
  the plan's 2631/5/2575): the store is live. The ratios are the facts; the counts are observations.

### 8.2 The cause (`MEASURED` by replay, `READ` by source)

`readSession` returns `null` under V2 — `createLegacyClient` exposed no `session.get` — so
`resolveSessionAgent` fell back to `client.session.messages` → `ctx.session.context({sessionID})`,
which the host bounds to the latest completed compaction
(`packages/core/src/session/store.ts:194` → `packages/core/src/session/history.ts:319-323`). That slice
is `[compaction, idle]`; the resolver skips the summary row by design
(`plugin/src/compaction.js:73-82`), so the agent was `undefined` and the gate returned before the send.
Replayed against the shipped `dist` twin with the live slice plus an answerable `session.get`:
`readSession -> null`, `resolveSessionAgent -> undefined`.

This is exactly what §1's premise cannot see: "every assistant row carries `$.agent`" was true of the
rows that exist, and the post-compaction slice contains **no assistant rows at all**.

### 8.3 The fix and the correction to this file's own review advisory

Issue 114 restores `session.get` in `plugin/src/v2/legacy-client.js` (upstream has it at
`example/opencode-mem/src/v2/legacy-client.ts:265-269`; the port dropped it). The agent and the model
now come from `Session.Info`, and the restore reaches the send. `docs/adr/0017-compaction-hook.md`
records the reversal of D-112-4; `.scratch/rust-port/issues/114-v2-restore-agent-gate.md` carries the
live evidence and the `session_v2` table gotcha.

**Advisory correction (this file's review note, not the code):** the M1 mutant for `textFromParts`
(`parts.map(p => p.text).join("\n")`) yields `'A\n\nB'`, **not** the `"A\nundefined\nB"` this plan family
predicted — `Array.prototype.join` renders a missing element as an empty string, it never stringifies
`undefined`. The distinguishing input still separates the two (`'A\nB'` vs `'A\n\nB'`), so
`test_v2_legacy_client_prompt_uses_only_text_parts` remains non-vacuous; only the predicted mutant
value was wrong.

### 8.4 The re-run (owner-run; the host runs this session and must not be restarted by an agent)

After the fix is committed and the host reloads its plugin generation from `dist/` (the watcher covers
`plugin/dist/**`; no engine restart, no `stage-engine.sh`):

1. `/compact` a session with at least one visible user message
   (`packages/app/src/session/commands/use-session-commands.tsx:392-400`), wait for completion.
2. Expect the **Memory Restored** toast.
3. Then read-only:

```sh
DB="file:$HOME/.local/share/opencode/opencode.db?mode=ro"
sqlite3 "$DB" "select datetime(time_created/1000,'unixepoch','+7 hours'),
  substr(json_extract(data,'\$.text'),1,40) from session_message
  where type='synthetic' and data like '%\"description\":\"memory context\"%'
  order by time_created desc limit 3;"
```

A row dated after the restart whose text starts `## Restored Session Memory` is the pass. If it still
fails, the next discriminating observations are `parent_id` on the compacted session in `session_v2`
(a child means the gate, not the agent) and `json_extract(data,'$.agent')` on that row (`NULL` means
the transcript fallback, which needs an agent-bearing row the slice cannot contain). Query
`session_message` for rows and `session_v2` for the session: the live table is `session_v2`, and
`session` is a stale 126-row pre-rename leftover whose query would "prove" the session does not exist.
