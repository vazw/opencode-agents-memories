# 108 — capture should use the host's stateless generate route, not a session it creates and deletes

Status: ready-for-agent — 2026-11-01. All claims below are **MEASURED against the live 2.2.5 host**
(`.tmp/progress/host-exchange-20261101T162001Z/`, plus a direct call recorded here) or **READ** from the
spec that host serves at `GET /openapi.json` (138 paths, fetched live into `.tmp/scratch/live-openapi.json`).

## What is wrong today, in one line

The capture summarizer creates an OpenCode session, names an agent that does not exist on the host, prompts
it, and deletes it — and the owner's session screen shows the debris.

```
POST /api/session               → 200  {"data":{"id":"ses_…","agent":"opencode-mem-structured",…}}
POST /api/session/{id}/generate → 503  {"_tag":"ServiceUnavailableError",
                                        "message":"Agent not found: \"opencode-mem-structured\"",
                                        "service":"session generation"}
GET  /api/agent                 → 28 agents, ours absent
```

The agent is registered only in the V1 `server()` hook (`plugin/src/plugin.js:938` → `:955`); our V2 entry
exports `{ id, setup }` with no `server` (`plugin/src/v2/plugin.js:61`), and the host runs V2 — so the
registration never executes.

## The route that should be used instead — measured

`POST /api/experimental/generate/text`, documented on the live build as:

> "Run one **stateless** model generation using the server's base configuration and return the assistant
> text. Uses the base configuration's default model when none is specified."

```
request   {"prompt": string, "model"?: Model.Ref{id, providerID, variant?}}   additionalProperties:false
200       {"data":{"text": string}}
errors    400 InvalidRequestErrorEncoded{_tag,message,kind,field}
          404 ProviderNotFound | 422 UnsupportedModelErrorEncoded{_tag,providerID,modelID,capability,message}
          502 ProviderRequestErrorEncoded{_tag,providerID,reason,message,status,retryAfterMs}
          503 ServiceUnavailableErrorEncoded{_tag,message,service}
```

Live call, this session:

```
POST /api/experimental/generate/text
  -d '{"prompt":"Reply with the single word ok","model":{"providerID":"opencode-go","id":"beam-free"}}'
→ 200 in 1098 ms,  {"data":{"text":"ok"}}
sessions before: 50 · after: 50 · delta 0        ← nothing appears in the user's session screen
```

The `/experimental/generate` path is documented on the same build as a **deprecated alias** of
`/experimental/generate/text`; use the `/text` spelling and treat a 404 on it as the signal that the route
moved.

## What this replaces

One call replaces four steps and removes four defects rather than working around them:

| today | after |
|---|---|
| create session → pollutes the user's session list | no session exists |
| name an agent the host has never registered → 503 | no agent field exists |
| send an undeclared `permission` ruleset (issue 106) | no session body exists |
| delete the session so "it never pollutes" | nothing to delete |
| 503 on a permanent error, retried 5× (issue 107) | 400/422/502/503 are distinct and carry `_tag` + fields |

**Issue 106 becomes moot for capture** if this lands: the deny-all ruleset only existed because the session
did. It stays open only if sessions are created anywhere else. **Issue 107 narrows**: at this route a
provider failure is `502` with `providerID`, `reason`, `status` and `retryAfterMs`, so retryability is
readable instead of guessed — a `503` here is the service, not the request.

## What lands

1. `crates/mem-ai/src/provider/opencode_host.rs`: replace the session dance — `create_session`,
   `prompt_session`, `session_create_body`, `delete_session` on the capture path, and the guard/deferral
   machinery around them — with **one** request to `{base}/experimental/generate/text`, body
   `{"prompt": …, "model": {"providerID": …, "id": …}}`, decoding `{"data":{"text": …}}`.
2. `STRUCTURED_OUTPUT_AGENT` (`:47`) and `session_create_body`'s `agent`/`permission` fields go with the
   session path. Deleting them is not optional: `-D warnings` fails on dead code.
3. `plugin/src/plugin.js`: `applyStructuredOutputAgentConfig` (`:921-936`) and its config-hook call
   (`:954-956`) are dead once nothing names the agent — remove them and the test that pins the constant.
4. Error mapping: classify by `_tag` and its fields, not by status class alone, so `422 UnsupportedModel`
   (a configuration error the operator can fix) never reads as `summarizer_failed`/retryable.
5. Preserve whatever the deleted session path handled that still matters — in particular the bounded
   retry/backoff decision must now be driven by `502`/`retryAfterMs` rather than by a bare status.

## Risks to state in the blueprint, not to discover later

- **The route is under `/experimental`.** Its stability is not promised. A 404 must degrade to a named,
  non-retryable reason, not to a generic failure — so the next host upgrade names itself.
- **The schema-in-prompt contract is unchanged and still load-bearing.** This route has no `format` field
  either, so the JSON schema keeps travelling inside the prompt text (`capture_context`'s append). The
  blueprint must not quietly drop it.
- **What remains unmeasured:** whether a capture-sized prompt (tens of KB of context + schema) returns
  parseable JSON from the configured model. The wire is proven; the model's compliance is not. Only a live
  capture settles it, and the acceptance criterion below says so.

## Acceptance criteria

- [ ] The capture summarizer makes exactly **one** HTTP request per attempt, to
      `POST {base}/experimental/generate/text`, pinned by a test asserting the URL and body shape.
- [ ] No session is created on the capture path — pinned by a test that fails if `POST /api/session` is
      issued at all.
- [ ] `{"data":{"text":…}}` is decoded from the measured shape, with the measured 200 body as the fixture.
- [ ] A 404 on the route produces a distinct named reason (route-moved), not `summarizer_failed`.
- [ ] A `502` carrying `retryAfterMs` is retried per that value; a `422 UnsupportedModel` is not retried and
      names the provider/model/capability.
- [ ] After the next re-stage, a live capture either persists or fails with a reason that names the
      remaining cause — recorded in `docs/evidence/` with host version and date. The model's compliance with
      the schema is the one thing this issue cannot promise.

## Parent

No slice parent. Supersedes the fix sketched in this issue's first revision ("drop `agent` from the create
body"): that would have stopped the 503 while keeping the session, and the session is the objection.

## Comments

**Why the first revision was wrong.** It accepted the session as a given and asked only how to make it
work — so the best it could do was remove the failing field. The owner's objection ("POST /api/session
pollutes the user session screen history") exposed that the session was never necessary: a stateless route
with the same `{prompt}` in and `{data:{text}}` out has been served by this host all along. Four defects,
one of them filed twice, existed only because we chose the stateful route.

**The generalisable rule.** Before fixing a field on a request, check whether the request is needed. The
spec that showed the failing field also listed the routes that make the whole object unnecessary — it was
in the same 138-path document the whole time.
