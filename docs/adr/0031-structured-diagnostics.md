# 0031 — Structured Diagnostics via the stderr Seam

- **Status:** Proposed
- **Date:** 2026-11-07
- **Extends:** ADR-0014 (stats log-signature emission; sink contract), ADR-0015 (log sink
  write path). **Supersedes:** nothing.

## Context

The engine-silent-500 wave (debug `engine-silent-500.debug-1.md`) proved mapped-500
diagnostics invisible by construction: `mem_web::api::port_failure` logs under the module
target `mem_web::api`, while the production file sink accepts exactly `mem_core::logging`
(ADR-0014:82, ADR-0015 D-1) — the sink sees nothing and no other subscriber exists. Widening
`LOG_TARGET` would change the file contract (an ADR-0014 amendment) to solve a diagnosability
problem that the engine's second channel, structured stderr, already solves. mem-web cannot
call mem-bridge's emitter (workspace_deps.rs forbids the edge).

## Decision

`structured_stderr` is hoisted verbatim into `mem_core::diagnostics` (pub fn, byte-identical
output; mem-bridge re-exports it — exactly one emitter). `port_failure` emits one
`api.handler_failed` line (component `api`) carrying endpoint + answered status + error-variant
token only — never raw fault text (ADR-0015 D-1 posture; issue-129 fixed-token precedent).
Event names stay in the wrapper's message-slot grammar (`[A-Za-z0-9_.-]`).

## Consequences

- ADR-0014 and ADR-0015 are unchanged: `LOG_TARGET`, the sink, and the `[<ts>] <message>`
  file format stay exactly as pinned; the file sink never sees diagnostics targets.
- The stderr NDJSON seam gains one producer class (`api.handler_failed`) the wrapper fans into
  the host log; no plugin change (names are grammar-legal, payloads are closed tokens).
- Diagnosability posture: a mapped 5xx is now observable in every launch mode (stdio,
  standalone, wrapper) without any log-file dependency; F3-class failures (typed store errors
  hidden by fixed-token audits) gain a visible trigger line.
