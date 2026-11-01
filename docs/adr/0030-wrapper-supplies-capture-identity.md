# 0030 — The wrapper supplies the capture's agent identity; the engine keeps strict validation

- **Status:** Accepted
- **Date:** 2026-11-01
- **Builds on:** `docs/adr/0027-host-endpoint-discovery.md` (D-27-7: the seven capture reasons),
  `docs/adr/0014-*` (D-2: `mem_core::logging::signatures` is the single spelling home).
- **Owner ruling (2026-11-01):** the wrapper sends the field; the engine keeps strict validation.

## Context

`fireIdleCapture` sent `{ session_id, capture_id, transcript, metadata }` (plus `project_id` when
the host supplied a directory) and no `agent_id`. `CaptureParams` is container-`#[serde(default)]`,
so the field deserialised to `""`; the inbox row carried it into `commit_episodic`, where
`upsert_session`'s validation rejects an empty `agent_id` (`check_id("agent_id", …)`). Both
`commit_episodic` call sites mapped any error to the fixed reason `episodic_write_failed`, which
`retryable_or_dead` turns into a dead letter on the fifth attempt. The rejection named no field
anywhere.

## Decision

1. **Option (a), producer-supplies, chosen.** The wrapper resolves `agent_id` through the seams the
   compaction leg already uses (`readSession`, `resolveSessionAgent`: `session.get` →
   `session.agent`, then the latest message's `agent`/`mode`) and sends it.
2. **Option (b), engine-defaults, rejected.** Letting the engine default an empty `agent_id`
   (or `project_id`) would hide a producer/consumer disagreement behind a green test — the third
   such defect this week. `test_empty_agent_id_still_dead_letters` and
   `test_empty_project_id_still_dead_letters` pin the strict direction.
3. **The `session_id` race is in scope.** The payload stamped `currentSessionId()`, a
   process-global latched by any hook, after two host awaits. A correct `agent_id` on another
   session's row is the same defect one layer down. The payload now stamps the hook's own id.
4. **`project_id` fails closed too.** With no host directory the key was omitted, which dead-letters
   through the same path under the other field name. The wrapper now sends nothing and writes
   `capture.project_unresolved`. Order is a decision: project first, so a host that supplies
   neither gets one reason and no host round-trips.
5. **An unresolvable agent is dropped and named** (`capture.agent_unresolved`), never sent empty
   and never replaced by a sentinel: a capture row must describe the agent that produced it.
6. **The engine names the field it rejects.** A new writer-side line,
   `Auto-capture episodic write failed` (`signatures::AUTO_CAPTURE_EPISODIC_FAILED`), is emitted
   once per failed `commit_episodic` attempt with `{ captureId, projectId, sessionId, agentId,
   errorKind }` plus `field` and a 120-character `detail` for validation errors only. `Failure`
   stays `Copy` with `&'static str` (D-27-7 fixes the seven reasons). The `detail` precondition was
   audited: every `EpisodicError::validation` producer interpolates a constant, a count or a
   length, never a value.
7. **Placement of the signature.** The constant lives in `signatures.rs` (single spelling home),
   is deliberately not one of the nine scraped signatures, and has no stats-contract row:
   `classify_log_message` is frozen by ADR-0014 D-2 and matches none of it, like
   `AUTO_CAPTURE_PROVIDER_FAILED` before it.

## Deferred

- **`parent_session_id`.** `isChildSession` is applied on both compaction legs and nowhere on the
  capture path, so a child session's capture is written as a **top-level row today** — silent
  mis-attribution, recorded here rather than hidden. Sending `session.parentID` would introduce
  `EpisodicError::NotFound { entity: "parent_session" }` retries in a race nothing can measure
  while nothing persists.
- **`inject.agent_id`.** `InjectParams.agent_id` has zero readers in `inject.rs`; sending it would
  add a field to the per-turn hot path with no consumer.

## Risk — needs-input

On a host whose session object **and** whose message infos both lack `agent`/`mode`, this change
turns a dead letter into a permanent, named drop (the capture never enters the inbox). That is the
owner's chosen trade. It does not bite today: the V2 adapter's `legacyMessage` propagates `agent`,
and the V1 host exposes `session.agent`.

## Non-claim

This change makes the producer send an identity the engine accepts, on the hook's own session, with
a named drop otherwise, and makes the engine name the field it rejects. It does **not** make capture
persist end-to-end and it does not move the live log: the summarizer path is a separate fix
(ADR-0029). No dead-letter count is recorded here; the live count moves.
