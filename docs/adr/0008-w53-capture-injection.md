# 0008 — W5.3: session capture, dual-tier persistence and chat-message injection

- **Status:** Accepted
- **Date:** 2026-10-21
- **Wave:** W5.3 (Phase 5; issue 21 — US 5.3)
- **Builds on:** `docs/adr/0005-episodic-storage-model.md` (the separate `episodic.db`, D-1 … D-16),
  `docs/adr/0006-episodic-consolidation.md` (the consolidation lock discipline),
  `docs/adr/0007-bridge-strategy.md` (D-36 capture ids, D-42 canonical wire, D-48 durable inbox before ack).
- **Blueprint:** `20261020-2157-w53-session-capture-injection-r5` (sha256 `be8c928f5bbd`, `SYS-APPROVED 2026-10-20T22:33Z`).

## Context

Issue 21 closes the capture→persist→inject loop: the host drives a capture on `session.idle`
(`input.client.session.messages` gathers the eligible prompt, assistant responses and tool calls), the
engine durably enqueues it before acking, the drain writes an episodic staging row **and** one
session-linked tier-1 capture summary, and the next eligible `chat.message` gets one bounded
`<project_agent_session_history>` block appended as a synthetic part. Nothing of this is a session-start
hook: upstream captures on `session.idle` and injects on `chat.message`, and the port preserves exactly
that split.

Two tiers, one scope family. The episodic tier is `episodic.db` (staging rows keyed by
`(project_id, session_id, agent_id)`); the tier-1 tier is a memory shard row. Both are addressed through
the SAME `TagResolver::project_tag_info` → `extract_scope_from_container_tag` derivation, and the two
writes are two LOCAL transactions — there is no cross-database transaction and this ADR never claims one.

## Decisions

### D-W5.3-1 — The recapture key is the scope triple plus the capture id ladder

`AgentSession`/`NewAgentSession` carry `capture_id`. The unique key stays
`(project_id, session_id, agent_id)`; within that triple the client-minted UUIDv7-shaped capture id is a
monotonic ladder: the same id is a no-op, a lexicographically newer id updates the live row in place, an
older id is superseded, and a tombstone is never mutated. The update path re-runs the parent
visibility/same-project check and the embedding-dimension check, so a recapture can never smuggle a row
into another project's scope.

### D-W5.3-2 — ONE port-owned history block

Injection renders exactly one `<project_agent_session_history>` block with
`## Project facts`, `## Captured session summaries`, `## Recent sessions` and
`## Related older sessions`. The W5.3 path emits no second `<memory_context>` and no
`<project_knowledge>` block. A capture-summary row whose session is already rendered as an episodic row
is suppressed (deduplicated, never counted twice); otherwise it renders under the summaries heading.

### D-W5.3-3 — `token_budget` is converted to bytes with checked arithmetic

`token_budget.unwrap_or(2000)` maps to `4 × tokens` bytes with CHECKED multiplication (an overflow is a
rejected request, never a wrap), and the returned `tokens` figure is `used / 4` — a byte-based estimate,
labelled as such. A candidate is appended only when
`used + candidate_bytes + required_marker_bytes <= budget`; a skipped candidate marks `truncated`; the
tier-1 subsection is capped at 25 % of the total budget; UTF-8 is never split. Nothing included ⇒
`block: None`.

### D-W5.3-4 — Capture-summary row ids are deterministic

`auto-capture-<64 lowercase hex>` over SHA-256 of the domain-separated tuple
`("capture-summary-v1", container_tag, capture_id)`. Upstream mints a fresh `mem_<millis>_<random>` per
write; the port's id must be stable so a retried partial commit cannot duplicate the row. The hash is row
identity only, never authorization. An existing id is verified against scope and session before it is
accepted as a replay; a mismatch is a collision (structured event, no overwrite).

### D-W5.3-5 — Two local commits and a defined partial-failure contract

Commit order per dequeued entry: (1) the episodic upsert, (2) the tier-1 capture summary, (3) the inbox
`mark_done` purge — each its own local transaction. Consequences, stated honestly:

- a tier-1 failure after the episodic commit leaves the episode row committed and the inbox entry
  retryable; the retry's same episodic `capture_id` is a no-op and the deterministic tier-1 id prevents
  duplicates;
- five CONSUMED tier-1 failures dead-letter the entry; the terminal state is an episodic-only row;
- a lock deferral (own or foreign) consumes NOTHING: the entry stays queued with `attempts` unchanged,
  because the engine's own consolidation holds `.turso-operation.lock` for a whole run and a lock window
  must never exhaust the bounded retry budget;
- `type == "skip"` writes neither tier and completes the item; tombstone/parent-validation failures
  write neither tier;
- the shutdown `db_flush` leg flushes only entries whose summary was already supplied (never an LLM) and
  leaves the rest queued for the next start's durable replay.

### D-W5.3-6 — Capture-summary retention is the existing tier-1 policy

A capture summary is a tier-1 memory row; it is deleted/pruned by the existing tier-retention policy and
this wave adds no capture-specific deletion path. It is not a promoted fact and this wave does not
promise it survives session deletion. Promoted facts (`source == "episodic-consolidation"`,
`session_id: None`) keep their own lifecycle and remain the only "durable fact" tier.

### D-W5.3-7 — The wrapper's `project_id` is the host directory (sequenced W5.1 follow-up: issue 34)

`plugin/src/plugin.js` was W5.1-owned. W5.3 changes exactly one thing in it: the wrapper freezes the host
`input.directory` once as `project_id` and passes it on every capture and inject — never a per-hook
override, never from the transcript or the event payload. The change is carried as the NAMED, USER
authorized sequenced W5.1 follow-up **issue 34**
(`.scratch/rust-port/issues/34-w51-wrapper-project-id-followup.md`), gated behind W5.1 landing first, and
`dist/plugin.js` stays byte-identical (`dist-sync.test.mjs` enforces it).

### D-W5.3-8 — Two distinct consolidation bounds

`CONSOLIDATION_MAX_PROJECTS = 64` bounds how many project ids ONE scheduler tick scans; the tree's
`DEFAULT_BATCH_LIMIT = 200` bounds how many rows one agent's consolidation batch claims. The scheduler's
cadence is `CONSOLIDATION_INTERVAL_SECS = 3600` (the episodic config exposes only the age threshold). Both
W5.3-new constants live in `crates/mem-bridge/src/engine.rs`. A busy lock is a clean skip
(`report.busy`), never a stolen lock, and the promotion sink's own-lock shard path is unchanged.

**Startup tick scheduling (r1).** The startup tick and the periodic loop run in ONE tracked task
(`spawn_consolidation_lifecycle`) that is SCHEDULED, not awaited, before `serve_stdio`: the tick can scan 64
projects and await an LLM per project while the plugin's `hello` handshake is timeout-bounded, so awaiting it
would let an aged store degrade the plugin to no-engine. Injection stays correct because the tick's RAII guard
sets `consolidation_active` around every `Consolidator::run`, and the injector skips only the tier-1 leg while
the flag is set. Consequence, by design: the first `inject` after launch may run before promoted facts exist
and may skip the tier-1 leg while a tick is in flight. The awaiting drain before the loop is unchanged.

## The capture-time trim port

`crates/mem-bridge/src/capture_context.rs` ports upstream's capture-time budgeting as four distinct
functions over the generic UTF-8 primitive (`truncate_to_max_bytes_with_marker`) with the upstream marker
`\n[... truncated to autoCaptureMaxContextBytes ...]\n` supplied explicitly:

- `fit_text_responses` — chronological responses in, newest-suffix-preserving output: unchanged when it
  fits, otherwise ≤ the byte budget, scanning newest→oldest, truncating the boundary response only when
  the marker fits; multi-byte characters are never split. Invariant: while scanning, `kept` is the
  original-order suffix of visited responses and `used_bytes` is exactly its UTF-8 byte count including
  separators, `used_bytes ≤ max_bytes`; the index strictly decreases, so it terminates.
- `get_auto_capture_markdown_budget(total)` — reserves `min(24,576, floor(total × 0.25))`, returns
  `max(4,096, total − reserve)`.
- `build_markdown_context` — `Previous Memory Context → User Request → AI Response → Tools Used`, with
  the upstream wrapper/skeleton budgets, the `min(4,096, floor(maxContextBytes × 0.25))` AI reserve when
  responses exist and the user budget exceeds 1,024 bytes, then the total UTF-8 cap.
- `build_bounded_summary_prompt` — reserves `min(16,384, floor(total × 0.125))` for output and subtracts
  the system prompt, JSON schema, that reserve and 1,024 overhead before truncating context plus suffix.

`mem_core::utils::context_limit` is the primitive only; it does not implement this family.

## Consequences

- The bridge's production composition root builds the capture service, the injector and the Q8
  consolidator from the resolved `mem_core::Config` (which `src/main.rs` loads synchronously and passes
  into `run_in_runtime`); no scheduler or runtime lives in `main.rs`.
- Injection is through the eligible `chat.message` hook, never a session-start hook; the wrapper does not
  capture on every message (the idle event owns the capture).
- Documented residuals: non-promotable milestone content is not injected, and semantic retrieval can
  observe stale vectors.

## Documented limitations

(i) No cross-database atomicity: the partial-commit contract above is the honest boundary.
(ii) A live lock defers a capture rather than blocking the request path.
(iii) Child attribution remains on milestone contributors; neither tier-1 row gains durable agent
attribution.
