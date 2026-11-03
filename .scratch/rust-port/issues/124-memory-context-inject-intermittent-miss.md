# 124 — the pre-turn `<memory_context>` inject silently misses some sessions

Status: needs-triage — measured 2026-11-03 20:38–21:36 (Asia/Bangkok) on the live host, engine
`0ba4e0f1` (pid 12684), wrapper `12af073c`, UI `web-ui-321d4859fd9a772f`.

Owner report: *"`◈memory context` not pop-up on the last planner after compaction."*

## It is a real miss, not a rendering artifact

`chat.message` pushes the engine's block as a synthetic text part
(`plugin/src/plugin.js:1301-1318`: `output.message.parts.push({type:"text", text:block, synthetic:true})`),
and the host **does** persist synthetic parts, so `session_message.data LIKE '%<memory_context>%'` is ground
truth for "this session got the block". Verified by probe: two throwaway `explore`/`planner` children
spawned at 21:35 answered that their own prompt contains the block, and their stored messages carry it
(`ses_e5e29d6f5ffejdVfJ5Byhak1q2` → 2, `ses_e5e297486ffem7RlEuiNF93qmW` → 1).

| spawned | session | agent | background? | messages | messages carrying `<memory_context>` |
|---|---|---|---|---|---|
| 20:38:06 | `ses_e5e5e5f88ffeThwLnyIduOz8AQ` | planner | yes | 158 | **2** ✅ |
| 20:50:06 | `ses_e5e53655cffe0wT9S2UXaX8Z9X` | architect | yes | 155 | **0** ❌ |
| 21:07:35 | `ses_e5e436208ffeRBmvZJxwOXHs3f` | syseng (gate) | yes | 137 | **0** ❌ |
| 21:29:18 | `ses_e5e2f7ea0fferC06656tPzBz3M` | planner | yes | 33 | **0** ❌ |
| 21:35:22 | `ses_e5e29d6f5ffejdVfJ5Byhak1q2` | explore | **no** | small | **2** ✅ |
| 21:35:54 | `ses_e5e297486ffem7RlEuiNF93qmW` | planner | **no** | small | **1** ✅ |
| ~21:37 | `ses_e5e28a8f7ffekOHx8l8b9w40HZ` | explore | yes | — | pending |

Every row's parent is this session (`ses_e9d5dd698ffe5lUV7W7s2KDpbh`), which itself carries 125.

## The engine answered every time

`~/.opencode-mem/opencode-mem.log` logs `profile inject` (the profile section was rendered) at
`20:38:06`, `20:50:06`, `21:07:35`, `21:27:00`, `21:29:18`, `21:29:26`, `21:35:22`, `21:35:29`, `21:35:54` —
i.e. at each spawn second, **including the three misses**. So the request reached the engine and the engine
rendered a profile; what came back was either an empty `block` or never attached. No `inject.failed` /
`chat.message.failed` warning is present in any plugin sink for these times.

The plugin's attach condition requires `output && output.message && Array.isArray(output.message.parts)`
(`plugin.js:1312-1317`); `queryFromParts` deliberately accepts either `output.message.parts` **or**
`output.parts` (`plugin.js:1462-1476`). If the shape were the problem, no session would ever inject — the
probes would fail too. So the live hypothesis is **the engine returned an empty `block`**, not a dropped
mutation.

## Candidate mechanisms (unranked, all untested)

1. **Query-dependent retrieval miss.** `injectParams(queryFromParts(output))` sends `include.query`; the
   three misses carried long, design-heavy prompts (2–4 KB), the hits were short prompts. A semantic
   threshold or budget that returns no rows for some queries renders a profile-only response — which would
   also explain "profile inject" being logged with no block. The 20:38 planner had a long prompt and hits,
   so length alone is not sufficient.
2. **Timeout.** `sendRequest("inject", …, TIMEOUT)` fails open by design; the engine would still finish and
   log `profile inject` after the wrapper gave up. There is no warning in a plugin sink for these times, but
   the plugin's `writeStructured` sink has not been located, so "no warning" is not yet evidence.
3. **~~Background vs foreground spawn.~~ ELIMINATED.** The 21:37 background probe
   (`ses_e5e28a8f7ffekOHx8l8b9w40HZ`) reported `HAS_MEMORY_CONTEXT: yes`. Background spawns inject fine.

## The contradiction that narrows it

`profile_tier` logs `PROFILE_INJECT` only when it **rendered a block**
(`crates/mem-bridge/src/inject.rs:732`), and `assemble` then always returns a block: the profile-only early
arm is `wrap_context(profile_block.as_deref(), None)` → `block: Some(…)` with `memory_count: 0`
(`inject.rs:527-541`), and the normal arm does the same with history (`:550-556`). **There is no path that
logs `profile inject` and returns `block: None`.**

So either

- those `profile inject` lines at the spawn second belong to a *different* request (another session's turn,
  a web/UI profile read, a compaction restore), and the child's `chat.message` inject never reached the
  engine at all — a wrapper-side drop before `sendRequest`; or
- the block came back non-empty and was dropped by the host after the hook's `parts.push`.

Both are wrapper/host-side. A test that separates them: log the outgoing `inject` request and the returning
`block_len` in the wrapper (one line each, `session_id` only, never content). Until that exists the miss is
unlocated, and any "fix" is a guess.

## What would close this as measurement, not a deliverable

Instrument first, then fix: have the engine log `inject.rendered` with `{session_id, block_len,
memory_count, query_len}` on every pre-turn inject (the compaction path already logs `memory_count`), and
have the wrapper log the block length it received. With that, the next miss says in one line whether the
block was empty (engine side, mechanism 1) or non-empty and unattached (wrapper/host side). A fix without
that line is a guess.

**Also worth stating:** the inject is fail-open by design (`D1`, the pre-turn inject is ungated), so a miss
is silent — the session simply runs without prior context. That is why this needed a transcript probe to
see at all.

## How to reproduce cheaply

Spawn a subagent whose whole task is three lines of self-report (`HAS_MEMORY_CONTEXT: yes|no`) and then read
its session in `~/.local/share/opencode/opencode.db` (read-only):

```sh
sqlite3 "file:$HOME/.local/share/opencode/opencode.db?mode=ro" \
  "select count(*) from session_message where session_id='<ses_id>' and data like '%<memory_context>%';"
```

Zero across the whole session means the block never reached it.
