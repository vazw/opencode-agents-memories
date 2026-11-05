# 107 — a deterministic 503 is retried five times, because the host encodes a validation failure as `ServiceUnavailableError`

Status: done — 2026-11-04 (STALE-PREMISE — session trigger gone; ServiceUnavailable has wire reason)
and READ from the live host's own spec (`GET /openapi.json` → 138 paths, served by the running 2.2.5 build).

## What is wrong

The host answers a **permanent** input error with a transient status class:

```
503 {"_tag":"ServiceUnavailableError","message":"Agent not found: \"opencode-mem-structured\"",
     "service":"session generation"}
```

Same request, same body, 3 ms, twice across separate probe runs. The engine's retry policy sees
`http-status`/503 and burns its budget:

```
Auto-capture provider failed: {"errorKind":"http-status","reason":"summarizer_failed","status":503}
… ×5 attempts within 1 s → "Auto-capture final error after 5 attempts" … "outcome":"dead_lettered"
```

15 such lines in one second after the re-stage. Nothing was transient; nothing could succeed.

## What the host's own spec says (READ, from the live 2.2.5 build)

- `ServiceUnavailableErrorEncoded` = `{_tag: "ServiceUnavailableError", message: string, service?: string|null}`
  with `required: ["_tag","message"]`. There is **no** `retryable`, `code`, `transient` or `retry-after`
  field, and the 503 response carries no description distinguishing causes.
- `POST /api/session` declares **no** 503 at all — only 200/400/401/404 — so creation-side validation is
  expected to be a 4xx. The 503 appeared only once the session was used.
- Nothing in the spec or the docs says which conditions map to 503 rather than `InvalidRequestError`.

So the *host* is arguably wrong to use 503 here, and we cannot change it. What we control is our reading
of it.

## Why this matters beyond one retry storm

The 503 is the only signal, so the engine's reason label (`summarizer_failed`) is the least informative
possible description of "the host's agent registry has no such agent". That is why a one-line wiring defect
looked like a flaky upstream provider for several rounds of investigation, and why a dead-letter counter
grew all evening without a single actionable log line.

## What would settle the design

Do **not** special-case this message. Generalise: a 503 whose body carries a **structured error tag and a
named precondition** (here `_tag` + `service` + a message naming a missing entity) is not retryable in the
transient sense, while a 503 with no body, or with provider/upstream identity, is. The discriminator is the
body, never the status alone.

## Acceptance criteria

- [ ] A 503 whose body carries `_tag: "ServiceUnavailableError"` **and** a `service` field is classified
      distinctly from an upstream/transient 503, and the resulting reason names the host's message (or a
      bounded excerpt of it), not `summarizer_failed`.
- [ ] That class is **not** retried, or is retried at most once — pinned by a row asserting the attempt
      count for a body-shaped 503 versus a bodyless one.
- [ ] A bodyless or provider-identified 503 keeps the existing retry policy — the change must not make
      genuine transient failures non-retryable.
- [ ] The row uses the two measured bodies (the agent-not-found body, and an empty 200/503) as fixtures, so
      the classification is pinned to observed shapes rather than invented ones.

## Parent

No slice parent. Surfaced by the same probe as issue 108; it is the *reason* 108 took an evening to find.

## Comments

**Not filed as "the host is buggy".** We cannot change the host, and the spec permits the shape it emits.
The defect we own is that a structured error body, which the host does provide, is discarded: the engine
reads the status class and throws the payload away. Both this and issue 88's earlier "the log does not
discriminate" finding are the same underlying habit — reading a status where a body was available.

**The taxonomy that makes this fixable was in the live spec all along (2026-11-01).** Issue 108's route,
`POST /api/experimental/generate/text`, declares distinct errors per condition:
`400 InvalidRequestError{kind,field}`, `404 ProviderNotFound`, `422 UnsupportedModel{providerID,modelID,capability}`,
`502 ProviderRequestError{providerID,reason,status,retryAfterMs}`, `503 ServiceUnavailableError{service}`.
So on that route a retryable provider failure is **502 with `retryAfterMs`**, an unfixable configuration
mistake is **422**, and a 503 is the service itself. Retryability becomes readable from the body instead of
inferred from the status class — which is what this issue asks for, available without inventing a scheme.
The acceptance criteria therefore apply at that route; if 108 lands, close the "agent not found as 503"
example above as superseded and keep the general rule.
