# 114 — the V2 restore's agent gate: `session.get` restored

Status: done — 2026-11-02 — implemented in the wave that created this file (local commit, nothing
pushed); the live re-run is owner-run after review and **PASSED** (2026-11-02 14:09–14:10 +07, see the
last section).

Issue 112 shipped the V2 post-compaction restore's send and passed review (`18fea68`), then failed live:
no restore row and no toast were ever produced. This issue records the live evidence, the confirmed
cause, the fix (restore `session.get` in the legacy client) and the owner's ruling that the
**pre-compaction ingest stays ungated**.

## Evidence (read-only; `file:$HOME/.local/share/opencode/opencode.db?mode=ro`)

### The failure: two restore injections fired, zero rows written

The engine log after the host restart carries five `Compaction memory injected` lines
(`~/.opencode-mem/opencode-mem.log`): `12:31:01`, `12:41:44`, `12:43:22`, `12:43:56`, `12:46:23` (+07).
Those are **two ingest/restore pairs** — `ses_eba84bcedffeNcYVGM5wTAWW0R` (`12:31:01` + `12:41:44`) and
`ses_e8839ef3fffeIiAusczdXyJPHU` (`12:43:22` + `12:43:56`) — plus one **orphan ingest** at `12:46:23`
(`ses_e65578af7ffeJ96Fn4CoHcONSm`, `memory_count:2`) whose session has no completed compaction at all.

Denominator, corrected (re-derived 2026-11-02T06:24:41Z, not inherited from the brief): the brief's
"five compactions" counted engine lines. `session_message` holds 17 rows of type `compaction` since
2026-11-01; five are dated 2026-11-02, and **three** are after the restart — `12:31:01` ses_eba84b,
`12:43:04` ses_e657d4e2bffeaBrtTxXvtbhXB7, `12:43:22` ses_e8839. So: two restore injections fired, zero
rows written.

`session_message` `type='synthetic' and data like '%"description":"memory context"%'` → **0** rows after
2026-11-01; the newest row is still `2026-10-30 22:49:04` (+07), text `## Restored Session Memory`.

### The post-compaction slice the wrapper actually saw

`ctx.session.context({sessionID})` is bounded to the latest completed compaction
(`packages/core/src/session/store.ts:194` → `packages/core/src/session/history.ts:319-323`). For
`ses_e8839` the rows from the boundary on are:

```
seq 6863  compaction  2026-11-02 12:43:22
seq 6866  idle        2026-11-02 12:43:56
```

The restore injection is at `12:43:56.167`, i.e. inside the same second the `idle` row was written — so
the slice at restore time was exactly `[compaction, idle]`, with no agent-bearing row. Replayed against
the shipped `dist` twin with that slice plus an answerable `session.get`:
`readSession -> null`, `resolveSessionAgent -> undefined`.

### The child-session scale (the ruling's basis)

`select count(*), sum(agent is null), sum(parent_id is not null) from session_v2;` →
`2639|5|2583` at 2026-11-02T06:24:41Z. The wave's plan recorded `2631|5|2575` — the store is live and the
counts drift; the ratios are the facts. 2583 of 2639 session rows are children, and one child was
ingesting at `12:46:23` with `memory_count:2`: gating the ingest would have removed the block from ~98 %
of the store.

### The table gotcha (this is why a "does the session exist" check can lie)

**The live table is `session_v2`. `session` is a stale 126-row pre-rename leftover (last written
2026-10-11), so a verification query against `session` proves the session does not exist.** Rows live in
`session_message`; the session row (agent, model, `parent_id`, `location`) lives in `session_v2`. Every
query in this issue names its table for that reason.

## Cause

`handleCompacted` (`plugin/src/plugin.js`) resolves the agent before sending. `readSession`
(`plugin/src/compaction.js:31-40`) returns `null` under V2 because `createLegacyClient` exposed no
`session.get`; `resolveSessionAgent` therefore fell back to `client.session.messages` →
`ctx.session.context`, which after a compaction is `[compaction, idle]`; the resolver skips the summary
row by design (`compaction.js:73-82`), found nothing, and the gate returned before the send.

Not a defect in `18fea68`'s send path — a **capability gap in the shim** that upstream does not have
(`example/opencode-mem/src/v2/legacy-client.ts:265-269` implements `session.get`; the port dropped it).

D-112-4's justification ("assistant rows carry `$.agent` in 100 % of rows") was true and irrelevant: the
slice contains no assistant rows at all.

## Fix (this wave)

- `plugin/src/v2/legacy-client.js`: `session.get` restored, before `messages`, with the header's false
  "deliberately absent" claim replaced by the restored-capability rationale and the post-compaction
  finding. Accepts `{sessionID}` and `{path:{id}}`, unwraps `{data}`, throws on a missing id and on an
  absent host capability (`readSession` turns the throw into `null` = "not a child, proceed").
- `plugin/dist/v2/legacy-client.js`: byte twin, copied in the same step (`cmp -s`).
- `plugin/src/plugin.js`: the pre-compaction handler's `readSession` + `isChildSession` lines are
  **deleted** (D-114-2, owner ruling) — the ingest stays ungated on both routes, because the handler is
  the one handler both routes delegate to.
- Agent and model are read for the **gate**, not the write: the V2 synthetic route drops both, and the V1
  route pins both in its send body, so the gate must not be "simplified" away.
- Tests: new `plugin/tests/v2-restore-agent.test.mjs` (the oracle the issue-112 wave lacked — the live
  slice **plus** an answerable `session.get`), red on `18fea68`; the child-gate case replaced by a
  de-gated ingest case plus a new restore-gate case; `compaction-v2-inheritance.test.mjs`'s
  `sessionGetCalls` oracle moved from `["sess-A"]` to `["sess-A","sess-A"]` (locator, then
  `readSession`); `v2-legacy-client.test.mjs` gained the `get` case.
- Records: ADR-0017 DECISION 2/5/11 amendments; `docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md`
  §8; a comment on issue 112.

## Decisions

1. **D-114-1 — fix by restoring `session.get`.** +5 lines, one file, the capability the port dropped from
   its own upstream reference. Rejected: deleting the agent gate — the V1 route pins the resolved agent,
   the model and the variant in its send body (`plugin/tests/compaction-hook.test.mjs`), so that option
   needs a V2-only branch plus a new route test, and it deletes the gate's only pin.
2. **D-114-2 — the pre-compaction ingest stays UNGATED for child sessions (owner ruling, binding).**
   Evidence it was decided against: 2583/2639 session rows are children and a child was ingesting live.
   Reversible by re-adding three lines.
3. **D-114-3 — the restore keeps its child gate, and it becomes live for the first time.** A child
   returns before the inject: no engine call, no row, no toast.
4. **D-114-4 — agent/model are advisory on V2.** Read for the gate, dropped by the synthetic route, pinned
   by the V1 route; the code comment states this.
5. **D-114-5 — the regression test is a new file, red on `18fea68`.** Its fixture is the live
   post-compaction slice plus an answerable `session.get` — which is why an APPROVED review still shipped
   a dead restore.
6. **D-114-6 — the records are part of the fix.** A reversed decision that lives only in `.tmp/` is one
   `rm` from gone.

## Blast radius

- **Ingest** (`plugin.js`, `experimental.session.compacting`): children keep ingesting, unchanged.
- **Restore** (`plugin.js`, `handleCompacted`): top-level sessions now resolve their agent from the
  session row and send; children return at the gate before the inject.
- **Idle capture** (`plugin.js`, `fireIdleCapture`, the third `readSession` caller): the first idle after
  a compaction stops being eaten by `capture.agent_unresolved` (same root cause, a second victim — the
  wrapper's stderr is a socket, so nobody saw it), so expect **new** capture rows after compactions; and
  attribution now prefers `session.agent` over the transcript's last assistant, which differs only inside
  a `switchAgent` window. The child gate is **not** extended to capture here — issue 63 owns it.
- A missing session row must never disable the restore: `readSession` returns `null` on a throw and
  `isChildSession(null)` is `false`, so a host `NotFoundError` degrades to "not a child, proceed".

## Follow-ups (recorded, not done here)

- Follow-up 2026-11-03 — the restore's child gate is now opt-in via `chatMessage.captureChildSessions`
  (ADR-0017 amendment 2026-11-03, `blueprint-20261103-0724-compaction-child-restore-r3`); D-114-2's
  ingest ruling is unchanged.

- DONE d481ed9 (this issue's follow-up): the module note in plugin/src/compaction.js no
  longer claims the V2 legacy client exposes no `session.get`. It now names the
  absent-capability branch as the V1 SDK's and cites 8a13ce3. Both twins moved together
  and `cmp -s` passed.
- `plugin/src/plugin.js` (ingest) has no child gate by ruling; the capture child gate belongs to issue 63.

## Live verification — PASSED 2026-11-02T14:09–14:10 +07 (owner-run)

`/compact` on two sessions after the host restart, then read-only
(`docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md` §8.5 carries the full block):

```
ses_e90309e58ffeUH4XeVmlAW3zpa  compaction 14:05:38 → restore row 14:09:08  (10 memories)
ses_e9d5dd698ffe5lUV7W7s2KDpbh  compaction 14:09:39 → restore row 14:10:19  (10 memories)
```

Both rows carry `description: "memory context"` and open `## Restored Session Memory`; both sessions
have an empty `parent_id`. The newest such row before the run was `2026-10-30 22:49:04`, so these are
the first restores in three days. The loaded `dist` was verified equal to the `8a13ce3` tree blobs by
`git hash-object` before the run was credited. The **Memory Restored** toast appeared too
(`REPORTED`, owner, 2026-11-02 — it is not machine-observable: ADR-0017 DECISION 11 keeps
`attention.notify` unused and the toast crosses `MemToast` → `tui.js` → `ctx.ui.toast.show`).

If a future run regresses, the next discriminating observations are `parent_id` in `session_v2` (a
child means the gate, not the agent) and `json_extract(data,'$.agent')` on that row (`NULL` means the
fallback). Query `session_message` for the row and `session_v2` for the session — not `session`.

## Parent

ADR-0017 DECISION 2 amendment, DECISION 5 second amendment, DECISION 11 note (all 2026-11-02); issue 112
(D-112-4 reversed); debug report `.tmp/progress/blueprint-20261102-0426-v2-compaction-restore-send.debug-1.md`.
