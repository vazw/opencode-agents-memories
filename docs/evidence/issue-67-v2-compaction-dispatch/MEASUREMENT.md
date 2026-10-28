# Issue 67 — the V2 compaction hook's dispatch, measured by source read

- **Host:** `/home/vaz/Code/Personal/JS/opencode` @ `8faeb81e92` (`git log --oneline -1`:
  `8faeb81e92 fix(cli): follow upstream dev-channel sharing in service config paths`); `package.json`
  `"version": "2.2.0"`.
- **Label:** **READ** — source-level. This wave did not execute the host: no local 2.2.0 install is wired
  to this repo, and the cited code is the code the host executes.
- **Date:** 2026-10-28.

## The re-derivation commands

Run from the host root. All five exit 0; the output below is verbatim.

### 1. The hook exists in the session hook domain

```sh
grep -n 'readonly compaction: SessionCompaction' packages/plugin/src/promise/session.ts
```

```
142:  readonly compaction: SessionCompaction
```

`SessionHooks` opens at `:139` and lists twelve fields, `prompt` and `context` included; `compaction` is
the third. `SessionContext extends SessionRequest` (`:38-41` of the same file) is
`{sessionID, model, system: SystemPart[], messages, options, agent, tools}` (`:30-41`), and
`SessionCompaction` adds `result?: SessionCompactionResult {summary, native?, metadata?, tokens?}`
(`:38-49`).

### 2. The compaction model request triggers the hook

```sh
grep -n 'agentHook("compaction", input.agent)' packages/core/src/session/model-request.ts
```

```
414:      compaction: (input) => prepare("compaction", input, agentHook("compaction", input.agent)),
```

### 3. `agentHook` is a `hooks.trigger` of the session domain

```sh
grep -n 'hooks.trigger("session", name, { ...draft, agent, tools })' packages/core/src/session/model-request.ts
```

```
410:        hooks.trigger("session", name, { ...draft, agent, tools })
```

`PluginHooks.trigger` iterates the registered callbacks and `yield*`s each one
(`packages/core/src/plugin/hooks.ts:88-94`), so a dispatched callback is awaited, not scheduled.

### 4. Native compaction is a different branch

```sh
grep -n 'const native = context.model.compaction?.type === "native"' packages/core/src/session/compaction.ts
```

```
303:      const native = context.model.compaction?.type === "native"
```

The dispatch is `const compaction = swapped ? … : native ? compactNatively(…) : summarize(…)`
(`:317-321`). Only `summarize` reaches the request builder: it is the function whose body calls
`requests.compaction({…})` at `:840`. A native model never calls `prepare`, so it never triggers the
hook.

### 5. The promise bridge awaits the callback

```sh
grep -n -A 1 'host.session.hook(name' packages/plugin/src/promise/adapter.ts
```

```
602:                host.session.hook(name, (event) => Effect.promise(() => Promise.resolve(callback(event))), options),
603-              ),
```

`Effect.promise` awaits the returned promise, so an `async` callback is awaited — and a rejected promise
becomes an Effect **defect**, which aborts the host's compaction rather than being a no-op.

## The chain, in one line

`compaction.ts:840` (`summarize` → `requests.compaction`) → `model-request.ts:414`
(`prepare("compaction", input, agentHook("compaction", input.agent))`) → `model-request.ts:408-410`
(`agentHook` → `hooks.trigger("session", name, { ...draft, agent, tools })`) → `hooks.ts:88-94`
(iterate and `yield*` each callback) → `adapter.ts:599-603` (`Effect.promise` wraps `callback(event)`).

The hook's input is a single object and there is **no output pair**: the array the request builder reads
back is `draft.system` (`SessionContext extends SessionRequest`, `system: Array<SystemPart>`), which the
callback receives mutably. The event payload cannot inject context: `session.compaction.ended` carries
`{...Base(sessionID), reason, model, native, text, cost, tokens}`
(`packages/schema/src/session-event.ts:608-624`, `Base = {sessionID}` at `:29-31`) and nothing else.

## The three-way compaction shape (ADR-0017 DECISION 5)

- **Summarise (`native: false`)** — `summarize` runs `requests.compaction` (`:840`) and the hook runs
  before the request is built.
- **Native (`native: true`)** — `compactNatively(selected, budget, settings.keep)` (`:319-320`) is chosen
  instead and never reaches `prepare`, so the hook does not run and no ingest is possible.
- **Precomputed** — a background `precompute` (`:337-389`) runs `summarize` **earlier** (its own
  `requests.compaction` at `:840` with a message prefix), so the hook runs there and the held summary
  carries the ingest; swapping that summary in (`usePrecomputed`, `:395-419`, chosen at `:317-318`) runs
  no new hook. No second hook exists or is added.

## What this measurement does and does not prove

**The source read is the only dispatch evidence in this wave.** The plugin's driver cases
(`plugin/tests/v2-compaction-hook.test.mjs`, `mode: "v2-compaction"`) call the *registered callback*
themselves; they never exercise `hooks.trigger`. They are the oracle for **adapter behaviour** —
session-id forwarding, the `event.system` append shape, the inherited capability gate, the no-send
restore and the non-application of the child gate — and they would stay green if the host stopped
dispatching. Nothing here executes the host.

**The residual measurement is a live host run:** register the plugin on a real 2.2.0 host, compact a
session, and read back the compaction request (or the resulting summary). The **inverted world** that
would refute this document's claim: a live run in which the registered `compaction` callback either
never runs or whose `event.system` append does not appear in the compaction request. Until that run
exists, the dispatch claim stays `READ`, not `MEASURED`.

The adapter-side inverted world is cheaper and already covered: revert
`plugin/src/v2/adapter.js`'s `await ctx.session.hook("compaction", …)` and
`test_v2_adapter_registers_no_tool_and_no_skills`'s key set plus all eight cases in
`v2-compaction-hook.test.mjs` go red (the driver at `no compaction hook`, the in-process cases at
`hooks.get("compaction") === undefined`).
