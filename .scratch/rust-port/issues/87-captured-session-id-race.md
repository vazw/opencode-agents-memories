# 87 — `capturedSessionId` is a module-global read after two awaits; concurrent idle events attribute a capture to the wrong session

Status: ready-for-agent — 2026-11-01 (premise MEASURED by the `opsec` audit §S7; a plan gate independently
reproduced the mechanism and confirmed the existing test could not fail on it. Work will modify
`plugin/src/plugin.js` (`fireIdleCapture` / `currentSessionId` use) and `plugin/tests/wrapper-glue.test.mjs`
(the driver below) or a new test file, and re-copy `plugin/dist/plugin.js`.)

## What is missing

`plugin/src/plugin.js:1037` latches `capturedSessionId = sessionId`, then `:1041` calls
`fireIdleCapture(sessionId)` — which `await`s `ensureStarted()` and `client.session.messages()`
(`:1155-1175`) and only afterwards builds `session_id: currentSessionId()` (`:1188`).

Another idle event landing inside those awaits overwrites the module-global, so capture A carries session
B's id. The correct value is already the function's own parameter and is ignored. Fan-out subagent
sessions idle inside the same host process, so overlap is the normal case, not the pathological one.

Downstream damage: wrong per-session recall, wrong `excludeCurrentSession` (issue 85), wrong compaction
restore, and a journal that mixes sessions.

## What lands

- `fireIdleCapture` uses its own `sessionId` parameter rather than re-reading the global after awaits.
- A row whose fixture retargets the global **from inside the suspension** — the fake host call fires an
  event for session B before returning — asserting the capture still carries session A. Today that row
  yields B and fails; an anti-vacuity assertion additionally requires B's own capture to exist.
- The test needs a driver change, and it is not expressible with the harness as it stands. In
  `plugin/tests/wrapper-glue.test.mjs`, `DRIVER_SOURCE` (`:132-180`) awaits each event **in order**
  (`for (const event of events) { … await … }`, `:174`) and binds **one shared** `stubData`
  (`:150`, `:154`). Two shapes are acceptable:
  1. fire event B **un-awaited** while event A is suspended inside `session.messages`, with the fake host
     dispatching a per-session stub (so A's suspension can be released by B's arrival); or
  2. a dedicated two-event un-awaited driver in a new test file, which is the cheaper option and does not
     perturb the nine existing glue cases.
  Neither exists today — the current harness cannot express "an event lands during another event's
  await".

## Acceptance criteria

- [ ] The RED must be failable **today**: the gate measured that "chat.message for B, then idle for A"
      is already green, because the latch at `:1037` happens immediately before the capture. Only a
      retarget from inside the await distinguishes the fix.
- [ ] With a retarget inside the suspension, the capture records `session_id: "ses-A"` after the change
      and `ses-B` before it.
- [ ] The companion assertion (B's own capture exists, carrying `ses-B`) is present so the row cannot
      pass by capturing nothing.
- [ ] The driver requirement in "What lands" is met, named in the test file, so a later reader can see
      why the case needs an un-awaited dispatch.

## Parent

Slice issue 94 (`94-slice3-switches-and-attribution.md`).

## Comments

**Why it is easy to miss:** the code reads correctly at a glance — the latch looks like it exists to
solve this exact problem. It solves it for the *synchronous* window only.

**Why the causal story an earlier revision told was wrong:** it claimed "the capture path became
asynchronous when agent resolution was added". It did not: `git log -S resolveSessionAgent --
plugin/src/plugin.js` returns only `6f8801b` (`feat(bridge): compaction hook — ingest before, restore
after (issue 33)`), and `fireIdleCapture` never calls `resolveSessionAgent` at all. The two awaits on the
capture path are `ensureStarted()` (`:1155`) and `client.session.messages()` (`:1165`), both older than
the agent-resolution work. The defect needs no agent resolution to exist — it needs only the suspension.

**A second occurrence, same class, different call path:** the inject path holds the same global.
`plugin/src/plugin.js:960` sets `capturedSessionId = hookInput.sessionID` inside `chat.message`, `:1134`
reads it back through `injectParams`, and `:346` falls back to `"oc-bridge-" + process.pid` when nothing
has been latched. Between `:960` and `:1134` sits `await sendRequest("inject", …)` (`:967`) — so a
concurrent `chat.message` can retarget the global before the inject of the first turn is built. The
same fix shape applies (read the hook-local id), and no issue tracks this path today; it is in scope
here if the change is one read, otherwise it must be filed rather than left silent.

**Scope note:** this is the same defect class as issue **85** — a value handed to a layer, then read
from a shared slot instead of from the input. Issues 83 (a missing producer) and 84 (a hardcoded `None`
schema slot) are *not* this shape; an earlier revision over-generalised them into it.
