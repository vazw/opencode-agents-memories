# PRD 171 §4 — the post-deploy observation, measured on the live host (2026-11-26)

The blueprint's `[TEST PLAN & ACCEPTANCE CRITERIA]` carried one observation that no unit test could run, and both
review rounds listed it as unverified:

> a session whose first `context.hook` in an activation reads `has_block:false, turn_index:0` is followed in the same
> activation by `context.hook has_block:true, turn_index:0` for that session, plus an `inject.requested` line carrying
> `query_chars:0` and no `turn_index`. Zero such sequences ⇒ `ctx.session.context` returns no messages mid-turn ⇒ the
> fallback is dead code ⇒ close §4 as measurement.

**It fired on the first opportunity after the deploy, and it repaired a live instance of issue 166 while doing so.**

Labels: **MEASURED** = in `run.txt`; **READ** = from the log's own fields.

## The sequence (MEASURED, `run.txt`)

The deploy is `plugin/dist/v2/adapter.js` written at **2026-11-26 16:43:05.435 +0700** (owner's call; the dist write is
itself the in-process re-activation).

| time (+07:00) | session | event | reading |
|---|---|---|---|
| 16:43:01.457 | `ses_e9d5dd698…` | `context.hook` | `has_block:true, turn_index:8` |
| 16:43:05.435 | — | dist written | **the re-activation** |
| 16:43:06.990 | `ses_e9d5dd698…` | `context.hook` | **`has_block:false, turn_index:0`** |
| 16:43:06.998 | `ses_e9d5dd698…` | `inject.requested` | **`query_chars:0`, no `turn_index`, `host_contract:"setup"`** |
| 16:43:10.710 | `ses_e9d5dd698…` | `context.hook` | **`has_block:true, turn_index:0`** |
| 16:43:14.007 → 16:43:55.455 | `ses_e9d5dd698…` | 7 more `context.hook` | `has_block:true, turn_index:0` |

Three things in one window:

1. **Issue 166 reproduced live.** The `turn_index` drop `8 → 0` 1.5 s after the dist write is the cache clear — this
   session's block was lost mid-turn by the re-activation, in production, exactly as filed.
2. **The fix recovered it.** 8 ms after the lost-block hook, the replay ran query-less (`query_chars:0`, no
   `turn_index`, `host_contract:"setup"` — a shape the wrapper's ordinary prompt path does not emit), and 3.7 s later
   the same session read `has_block:true` and stayed true across seven further steps.
3. **`ctx.session.context` returns messages mid-turn on this host.** That is the one claim neither reviewer could
   verify: no declaration exists in this tree or in `@opencode-ai/plugin@1.19.0`, and issue 112's measured shape is
   `[compaction, idle]`. The fail-closed floor would have returned `[]` and produced no replay at all; the replay
   happened, so the read returned at least one message during a `context` hook.

## Limits, stated before anyone cites this

- **One sequence, one session, one activation.** It proves the mechanism can fire and did fire; it establishes no rate,
  and it says nothing about sessions whose `ctx.session.context` returns `[]` (those fail closed to today's behaviour).
- **Two other `has_block:false, turn_index:0` lines fall in the same minute and are not counted**: `ses_de9d6c80…` at
  16:41:55.496 and 16:42:04.748, both **before** the deploy — the pre-fix adapter ran there. Their lack of recovery is
  not evidence against the fix; they are the baseline it replaces.
- The window starts at 16:43:00 and reads all 25 log files; `run.txt` is frozen at 2026-11-26T09:44Z, so later traffic
  is not in it.
- The observation is **not** the acceptance oracle — the 20 adapter rows are. This only closes the blueprint's named
  post-deploy question.

## Reproduce

```sh
cd /home/opencode/Code/Personal/Rust/opencode-agents-memory
python3 docs/evidence/issue-171-s4-postdeploy-20261126/extract.py            # default window
python3 docs/evidence/issue-171-s4-postdeploy-20261126/extract.py 2026-11-26T16:43:00
```

Read-only: reads `~/.opencode-mem/opencode-mem*.log`. Writes nothing. Live from here on, so a fresh run reflects
current traffic, not the frozen sample in `run.txt`.
