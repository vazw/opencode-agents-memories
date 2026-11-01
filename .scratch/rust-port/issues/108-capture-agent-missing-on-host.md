# 108 — the custom `agent` we send at session creation does not exist on the host, so every capture dead-letters 503

Status: ready-for-agent — 2026-11-01. **MEASURED against the live 2.2.5 host** (probe evidence
`.tmp/progress/host-exchange-20261101T162001Z/`, second round `case1/2/3`), not inferred from the spec.

## What is wrong

`session_create_body()` sends `"agent": "opencode-mem-structured"` plus a deny-all `"permission"` array.
The host accepts the create (it does not validate `agent` there — `POST /api/session` declares only
200/400/401/404 and has **no** required array) and echoes it back. Then the prompt call resolves that
agent against the host's registry and fails:

```
POST /api/session              → 200  {"data":{"id":"ses_…","agent":"opencode-mem-structured","model":{…}}}
POST /api/session/{id}/generate → 503 {"_tag":"ServiceUnavailableError",
                                       "message":"Agent not found: \"opencode-mem-structured\"",
                                       "service":"session generation"}
GET  /api/agent                → 28 agents, ours absent
```

## Why it does not exist

We register the agent **only inside the V1 `server()` hook**
(`plugin/src/plugin.js:938` → `applyStructuredOutputAgentConfig` at `:955`). Our V2 entry exports
`{ id, setup }` with no `server` (`plugin/src/v2/plugin.js:61`), and the host runs the V2 contract — so
the registration never executes on 2.2.5. `grep applyStructuredOutputAgentConfig plugin/src/v2/` → 0 hits.

## The measurement that gives the fix

Three sessions, each created then prompted then deleted (evidence dir, `case1/2/3`):

| case | create body | create | generate | body |
|---|---|---|---|---|
| 1 | **no `agent`** | 200 (8 ms) | **200** (1.53 s) | `{"data":{"text":"ok"}}` |
| 2 | `agent: "build"` | 200 (6 ms) | **200** (1.26 s) | `{"data":{"text":"ok"}}` |
| 3 | control: our agent | 200 (5 ms) | **503** (3 ms) | `Agent not found: "opencode-mem-structured"` |

So the prompt route itself is sound; **the only thing that breaks it is the agent field we send.** The
`{"data":{"text":…}}` reply is now MEASURED, not READ — this closes the last unmeasured shape in issue 88.

Also measured: the model echo in the create response is exactly what we send
(`{"id":"beam-free","providerID":"opencode-go"}`), with `variant:"default"` host-added; `/api/config`
carries no `model` key, so nothing was silently defaulted.

## What lands

1. `crates/mem-ai/src/provider/opencode_host.rs` `session_create_body()`: **remove** `"agent"` and the whole
   `"permission": [deny *, allow StructuredOutput]` array, leaving `title`, `model`, `metadata`.
   Removing `permission` is part of the same edit, not a separate one: with no `agent` it would attach the
   deny-all ruleset to the host's **default** agent, and what that does to `/generate` has **not** been
   measured. (The undeclared-key defect is filed separately as issue 106 — do not conflate them.)
2. `STRUCTURED_OUTPUT_AGENT` (`opencode_host.rs:47`) becomes unused → delete it, or the workspace
   `-D warnings` clippy gate fails on dead code.
3. `plugin/src/plugin.js`: `applyStructuredOutputAgentConfig` (`:921-936`) and its `config`-hook call
   (`:954-956`) are dead on a 2.2.5 host — remove rather than leave a registration that cannot run.
4. `plugin/tests/structured-output-agent-config.test.mjs:43` pins the Rust const's literal string in both
   directions; that pin goes with the field it pins.

## The one thing still unmeasured

Whether a **capture-sized** prompt (schema + context, tens of KB) returns parseable JSON from the host's
default agent. The wire does: `{"data":{"text":"ok"}}`. The structured-extraction step is a model judgement,
not a route property, and only a live capture settles it. Do not let the fix's evidence claim more than
"the request is now accepted and the reply decodes".

## Acceptance criteria

- [ ] `session_create_body()` sends `title`, `model`, `metadata` only — no `agent`, no `permission`.
- [ ] `STRUCTURED_OUTPUT_AGENT` is gone, and the workspace builds under the `-D warnings` clippy gate.
- [ ] The plugin no longer carries a registration path that cannot execute on a 2.2.5 host, and the test
      that pinned the removed constant is gone with it (not skipped).
- [ ] A bridge-level test pins the **create body shape** against the measured 200, so reintroducing
      `agent`/`permission` fails a test rather than a capture.
- [ ] The prompt-path test asserts decoding `{"data":{"text":…}}` from the measured reply.
- [ ] After the next re-stage, the live log shows a capture reaching the summarizer with a non-503 outcome,
      and that outcome is recorded in `docs/evidence/` with the host version and date.

## Agent-side effect on issue 88

Issue 88's `model`-and-`agent`-move-into-create decision is **half wrong**: `model` was right (measured,
echoed), `agent` was wrong (measured, 503). Record that on issue 88's Comments when this lands — the ADR
text that says the structured-output agent travels on the create body becomes false.

## Parent

No slice parent. Found by probing the live host after the issue-88 wire wave was re-staged.

## Comments

**Why the earlier reasoning missed it.** Issue 88's gates treated the create body's `agent` as a *declared
field, therefore safe* — and it is declared, optional, and accepted. What no gate could see is that the
value names an entity registered by a **plugin hook the host does not run**. Declared-and-accepted is not
existent. That is the third distinct instance today of "the spec permits it" being mistaken for "it works".
