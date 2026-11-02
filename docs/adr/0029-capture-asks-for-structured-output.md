# 0029 — Capture asks for structured output, through a capability that cannot drop the schema

- **Status:** Proposed
- **Date:** 2026-11-01
- **Builds on:** `docs/adr/0027-host-endpoint-discovery.md` (the host transport),
  `docs/adr/0028-manual-capture-provider.md` (which provider capture uses),
  `docs/adr/0008-w53-capture-injection.md` D8 (filter the request, never the reply).
- **Owner ruling:** the capture summarizer must ask for a schema-bound completion instead of
  repairing seven adapters, so an eighth adapter cannot repeat the defect.
- **Amended:** 2026-11-01 (issue 88, the wire wave) — D-29-6 restated: the fixtures answer the
  **documented** wire, which is READ from the API description and has never been captured; the schema
  still reaches the model, now in the prompt TEXT; the scope guard's item 1 is narrowed to what is left.
- **Amended:** 2026-11-01 (issue 108, the stateless-capture wave) — D-29-7..D-29-11 replace the session
  lifecycle with ONE stateless call, delete the loop guard with its subject, name the parse miss, carry
  retryability on a closed wire token, and record two tracked deferrals.
- **Amended:** 2026-11-02 (issue 110, the capture-scheduling wave) — D-29-10's dead-letter trigger narrows to
  the `!retryable` class and its `Deferred` rejection ground loses the "no scheduling column" reason; the
  >30 s paragraph is folded into the new D-29-12; D-29-11's deferral (1) is resolved.

## Context

Auto-capture had persisted nothing since the port went live. Every attempt dead-lettered with
`reason: "summarizer_failed"` against a provider answering HTTP 200 — a caller/contract mismatch, not a
provider fault.

`ProviderCaptureSummarizer::summarize` set `request.json_schema = Some(CAPTURE_OUTPUT_SCHEMA)` and then
called the **streaming** `AiProvider::complete()`. The capability it needed was already in the tree as
`mem_ai::provider::AiProviderStructured::complete_structured`, and the registry's factories always built
adapters that implement it — but both factory aliases returned `Arc<dyn AiProvider>`, so the capability
was discarded at the door.

MEASURED, and re-derived here:

- **8** concrete `fn complete` impls exist in `mem-ai`. **7 of 8** drop `request.json_schema`: five build
  their own body with the schema slot `None` (`openai_chat`, `anthropic`, `minimax`, `gemini`,
  `openai_responses`) and two delegate through `openai_chat::complete_with` (`atlas_cloud`,
  `orcarouter`). **1 of 8** honours it (`opencode_host::run_chunks`).
- Probe E — the engine's exact streaming body — returned 80 deltas / 308 bytes of markdown,
  `parses as JSON: NO` (`.tmp/scratch/probe-stream.out`).
- Probe A — the non-streaming structured arm — returned a valid `tool_calls` payload with a non-empty
  `summary` in **25.5 s** (`.tmp/scratch/probe-capture-format.out`).

## Decisions

**D-29-1 — The schema-free path is unrepresentable AT THE SUMMARIZER, and only there.**
`ProviderCaptureSummarizer` holds an `Arc<dyn StructuredCapture>` and **no `AiProvider` at all**. The
new seam exposes exactly one method, `complete_structured(request, schema)`; there is no `complete`,
so no schema-free call site exists and no streaming fallback can be added without reintroducing the
field this decision deleted. A `StructuredCapture` that answers `Unsupported` therefore surfaces as a
named failure, not as a quiet downgrade.

This is **not** a guarantee at `CaptureProvider`. Its `base` and `structured` fields are not
type-checked to describe the same provider: nothing in the type system stops a caller from pairing the
host's `base` with the manual arm's `structured`. The pairing is a **composition invariant**, held by
`engine_capture_provider`'s two adjacent construction sites and by code review. A discriminating row is
constructible (drive both halves through one recording `HostTransport` and compare the resolved
origin), and this decision deliberately does **not** spend one — the pair is two adjacent lines in one
function, and a row that re-reads them buys a fixture, not an invariant.

**D-29-2 — The registry hands out the capability, and `create` keeps its old contract.**
Both factory aliases now return `Result<Arc<dyn AiProviderStructured>, ProviderError>`;
`ProviderRegistry::create` upcasts once and is byte-identical for every caller, and
`create_structured` is the explicit door capture uses. `manual_provider::build_manual_provider` and
`manual_capture_provider` return the structured handle for the same reason.

**D-29-3 — One filtering rule, with the fail-loud guard attached to it.**
`privacy::filter_request` is the pure rule (extracted verbatim from the private inherent method);
`filter_request_for_delivery` applies it and refuses to dial a conversation it emptied
(`ProviderError::Config("all messages were filtered as injected")`, computed from `had_messages` BEFORE
filtering). `PrivacyFilterProvider::complete` and the new `FilteredStructured` both call the guarded
form; `FilteredStructured` returns the inner value unfiltered (D8 — filter the request, never the
reply).

**D-29-4 — The host arm is warmed once and hands over BOTH halves; composition mints the guard.**
`engine_capture_provider` returns `Option<CaptureProvider>`; `host_arm` keeps `let _ = host_provider.warm();`
(load-bearing: `Auto-capture provider ready` is emitted only from `LazyHostProvider::resolve()`, and on
the guard configurations — host named, auto-capture off, empty inbox — nothing else resolves). Both arms
are unfiltered; `build_composed_runtime` wraps the structured handle in `FilteredStructured` and is the
only mint. `ComposedRuntime.provider` now means "the filtered capture handle"; the consolidation
summarizer deliberately keeps the UNWRAPPED `base` (D-52-7).

**D-29-5 — The profile lane keeps the bare rule, declared not dropped.**
`PrivacyFilterProvider::complete_structured` still calls `filter_request` without the delivery guard,
because it is unreachable in production today: `PrivacyFilterLayer::wrap_structured` has no production
caller (only its own tests), and `mem-profile`'s `AiProviderCompletion` has no construction site outside
its own module and its own test. The capture path is guarded regardless, by `FilteredStructured`. When a
production caller of `wrap_structured` lands, it must adopt `filter_request_for_delivery` or declare why
not.

**D-29-6 — The host fixtures answer the DOCUMENTED wire.**
The rows in `crates/mem-bridge/tests/host_capture_provider.rs` answer the response shapes the OpenCode HTTP
API description declares — `{"data": {"id": …}}` on session create and `{"data": {"text": …}}` on the
prompt — plus the permanent bare `{"id": …}` fall-through. Those shapes are **READ** from
`docs/evidence/issue-88-host-wire-contract-20261101/`, a schema sample and not a transcript: the rows
evidence the WIRING, never the owner's live host. `host_capture_e2e.rs` now drives the same sampled bodies
through the real summarizer and shows a capture reaching persistence **against those documented shapes** —
which does not establish that the live host answers so.

The schema is not on the wire as a field any more (the `/generate` route declares only `prompt`): it is
appended to the prompt TEXT by `capture_context::build_bounded_summary_prompt`, under
`capture_context::SCHEMA_ANSWER_INSTRUCTION`, and charged against the same budget it spends. Two recorded
losses, both deliberate: `format`/`tools` leave the host arm (superseded by the prompt text), and the
assistant-side error name/data is no longer surfaced (the documented reply has no field for it; a non-2xx
is named by its status). No model left the wire — the create body carries `Model.Ref` and `agent`.

**D-29-7 — Capture is ONE stateless call; the session is gone (issue 108).**
The provider no longer creates, names, prompts and deletes a transient session (three HTTP calls plus a
process-global loop guard). `HostState::generate` makes exactly one `POST {base}/experimental/generate/text`
with a body of `{"prompt": …}` plus `{"model": {"id", "providerID", "variant"?}}` when a model is resolved
(an empty model, the `"inherit"` sentinel, or a missing provider id omits the key). There is **no query
string**: the measured call carried none and the route declares `parameters: []`. The reply is
`{"data":{"text": …}}`. This is what the reference implementation does, and it is MEASURED working against
the live 2.2.5 host: `200`, 1098 ms, `{"data":{"text":"ok"}}`, and the host's session count unchanged
(50 → 50) — the old create polluted the owner's TUI list and failed with a 503 naming an agent the host
never registered. `additionalProperties:false` is a runtime contract here, unlike the old create body's
undeclared `permission` ruleset.

**D-29-8 — The loop guard is deleted with its subject.**
An internal capture session no longer exists, so `internal_capture`'s tracked set, its reserved title, its
60 s grace window, `note_structured_step` and the client-side step cap have nothing to guard. Upstream keeps
them because upstream still creates a session. The plugin's `hooks.config` agent registration (issue 75) is
deleted in the same wave: the host runs V2 and our adapter forwards only `event`, so the hook never reaches
it (MEASURED: `GET /api/agent` → 28 agents, ours absent).

**D-29-9 — One format authority, and the parse miss is named.**
`CAPTURE_SYSTEM_PROMPT` loses its markdown `FORMAT:` block — its one production reader always supplies a
schema, so the single format instruction is `capture_context::SCHEMA_ANSWER_INSTRUCTION`. Because the route
cannot enforce `json_schema`, the reply is recovered by upstream's three-shape scan
(`parse_structured_lenient`: direct JSON → first fenced block → the LAST balanced object that parses) and a
miss is `ProviderErrorKind::ModelOutput` (`capture_output_not_json`, retryable), carrying only the reply's
byte length. **Admitted blind spot:** a well-formed but wrong balanced object parses silently — the scan
cannot tell a model's example from its answer, and the last-object rule can prefer the former. `parse_capture_value`
stays fail-closed (no summary → no capture), but it cannot detect a wrong summary with the right shape.

**D-29-10 — Retryability is decided by the closed wire token, never by a message or the status class.**
`decode_generate_failure` maps every declared `_tag` (both spellings) onto eleven tokens
(`wire_reason`: `invalid-request`, `unauthorized`, `provider-not-found`, `model-not-found`,
`location-not-found`, `model-unsupported`, `provider-request-retryable`, `provider-request-terminal`,
`service-unavailable`, `route-moved`, `unrecognised`), builds a bounded `detail` from the DECLARED error
fields only (`field`/`kind`, `providerID`, `modelID`, `capability`, `location`, `service`, `reason`,
`status`, `retryAfterMs`, `tag`), and keeps the message fixed per status. `classify_provider_error` branches
on the token first; `is_retryable()` applies ONLY to the untagged default, because it returns true for every
5xx and would erase the two 502 classes. `record_failure` dead-letters at once, with no attempt bump, for a
`!retryable` class — checked BEFORE any hint, so a terminal class carrying a stale `retryAfterMs` still dies
there. A `retryAfterMs` of ANY **POSITIVE** magnitude instead SCHEDULES the row and consumes no attempt; a
`retryAfterMs` of **`0`** orders no deferral (`not_before = now + 0` is due at once), so it is treated as the
ABSENCE of a hint and runs the unchanged five-attempt ladder (D-29-12). Negative hints are already dropped by
the decoder (`Value::as_u64`), so `0` was the only non-positive value that reached the scheduling branch.
`EntryOutcome::Deferred` was rejected on two grounds: it consumes no attempt (nothing bounds the loop), and
it already means "a live lock or an unreadable scope shard", so reusing it would erase a diagnostic
distinction. Its third ground — "it honours no hint without a scheduling column" — is retired with the
column (D-29-12).
**Chosen bounded behaviour:** a `ProviderRequestError` with an absent or unknown `reason` stays RETRYABLE —
`reason` is required with a closed enum, so a conformant host cannot send a permanent 502 without one; the
cost asymmetry (four extra requests versus permanent loss) decides it, and the ladder bounds it at five
when the host supplies no hint (D-29-12).

**D-29-11 — Two tracked deferrals (one resolved).**
(1) **RESOLVED by issue 110 (D-29-12).** The capture inbox had no scheduling column, so a bounded
`retryAfterMs` was honoured as a refusal, not a delay, and a >30 s hint was permanent —
`.scratch/rust-port/issues/110-capture-inbox-not-before-column.md`; that wave added the column, the drain
predicate and the schedule write.
(2) `build_bounded_summary_prompt`'s byte accounting omits the two-byte `\n\n` join that
`opencode_host::prompt_text` folds between the system prompt and the user text (the schema block itself is
charged exactly) —
`.scratch/rust-port/issues/111-summary-prompt-budget-two-byte-gap.md`. Neither is a defect this wave may
absorb silently.

**D-29-12 — A retry hint schedules; the ceiling retires.**
`HOST_RETRY_AFTER_CEILING_MS` is DELETED, and this decision is the only place that still spells it, because
the sentence has to say why it is gone: its single reader was the dead-letter trigger this decision removes,
so retaining it even as a log threshold would leave a policy that looks retired but still gates a behaviour.

The mechanism is a nullable column and a drain predicate. `capture_inbox.not_before INTEGER` (episodic schema
**v7**, applied by a probe-then-ALTER leg inside `apply_episodic_schema`, gated on `options.max_known >= 7`,
**no backup** — one transactional batch plus one pragma converge under the probe, so a restore would have
nothing to roll back beyond the batch the engine already rolls back itself) holds an absolute unix-millis
instant; `NULL` means "no schedule", which is exactly the pre-v7 behaviour, so no row is backfilled. The drain
selects `status = 'queued' AND (not_before IS NULL OR not_before <= ?)`, keeping `status` as the leading
equality on `idx_capture_inbox_status_created`: the new predicate is a residual filter on the rows that seek
already returns, so NO index is added. A `(status, not_before, created_at)` index would be a third index
maintained on every enqueue, attempt bump and schedule write, for a table capped at 1,024 live rows — and it
would not remove the sorter the `capture_id` tie-break forces, so no sort argument is made.

**A hinted failure never consumes an attempt and never dead-letters, at any POSITIVE magnitude.** `schedule_retry`
stamps `not_before = now + retryAfterMs` on the row, which stays `queued`; the writer and the drain read ONE
clock (`EpisodicStore::now()`, the store's injectable seam), so a fake-clock test and a real row cannot
disagree. The magnitude is the host's: `i64::try_from(ms).unwrap_or(i64::MAX)` then `saturating_add`, never an
`as` cast — a wrap would go negative and make an absurd hint retry immediately, the silent opposite of the
intent. No horizon is applied; a ceiling under another name is what the owner retired. A deadline survives a
process restart (the row is durable) but not a backwards wall-clock correction, which can hold a capture for
the size of the step; neither direction loses the row, and a monotonic clock would not survive a restart.

**A zero hint is not a hint.** `schedule_retry` with `retryAfterMs: 0` stamps `not_before = now`, which the
drain predicate (`not_before <= ?`) makes eligible again at once: the row would consume no attempt and never
reach the ladder — one host request and one `capture.retry_scheduled` line per drain for the life of the row,
with `attempts` pinned at 0 so nothing in the observability set can find it. The scheduling branch therefore
requires `retryAfterMs > 0`. This is not the retired ceiling under another name: nothing dead-letters because
of a hint's MAGNITUDE — the ladder counts futile attempts exactly as it does for a failure that carries no
hint. `0` is the only non-arbitrary boundary, because the drain has no fixed cadence (it is spawned per
accepted capture, plus the startup drain, `engine.rs:621,860`), so any positive floor would be an arbitrary
constant that silently overrides a live host's hint. The input is contract-reachable, not hostile-only:
`ProviderRequestErrorEncoded.retryAfterMs` declares no positive minimum
(`docs/evidence/issue-88-host-wire-contract-20261101/`). The decoder stays a faithful decode — a `0` is still
carried into the failure payloads, where it is the diagnostic that identifies this class — and the
ladder-versus-schedule policy lives in `mem-bridge`, never in `mem-ai`.

**The dead-letter remainder.** A failure still dead-letters on the first drain, with no attempt bump, when the
class is `!retryable` — and that check runs BEFORE the hint, so a permanent class carrying a stale
`retryAfterMs` is never scheduled. The branch is reachable, not theoretical: `opencode_host` assigns the
terminal token for `content_filter` / `authentication` / `invalid_request` and attaches `retryAfterMs` on BOTH
branches, and the capture classifier copies the hint through unconditionally. A hintless retryable failure
keeps the five-attempt ladder. A dead-lettered row and its `payload` still survive
(`DEADLETTER_CAPTURE_INBOX` is an `UPDATE … status='dead'`) and are still recovered only by hand: the
`requeue-captures` tool remains unshipped.

**Compensating bounds, stated because a hinted class retries indefinitely.** (a) *Capacity*: a parked row
stays `status = 'queued'` and therefore still occupies one of the `INBOX_CAPACITY` = 1,024 slots; at the cap
`enqueue` returns `InboxError::Full` and the capture request fails VISIBLY to the caller — that visible
failure is the accepted trade for retiring silent loss, and it is why a parked row must never be excluded from
the queued count. (b) *Observability*: `capture.retry_scheduled` — one structured warn line per scheduling
attempt carrying `capture_id`, `project_id`, `retry_after_ms`, `not_before`, `reason` and `status?`, never the
payload — is the only signal on a path whose dead-letter line no longer fires. Its cadence is per failed
attempt, not per tick: a parked row is not dequeued, so it writes nothing while it waits. **Named gap:** a
hostile or broken `retryAfterMs` near `u64::MAX` parks the row at `i64::MAX` — queued, never eligible,
functionally a dead-letter that keeps the `queued` status; it is logged exactly ONCE and never again, it keeps
consuming a capacity slot, and the payload survives, so only that one line with the absurd value makes it
findable.

**Rollback is a precondition, not a procedure.** Dropping the column would be allowed at the row level (only
scheduling hints would be lost, which no pre-v7 row ever had), but there is no `DROP COLUMN` step in the tree
and `EPISODIC_MAX_KNOWN` reverts to 6, so a reverted build refuses a v7 file with
`SchemaNewerThanBuild { found: 7, supported: 6 }` (D-21-3). Any copy of a live root therefore requires the
engine STOPPED, `cp -a` of the WHOLE root (a clean close never checkpoints, so committed data may sit entirely
in the `-wal`), and `PRAGMA user_version` on the copy reading 6 BEFORE the upgrade starts; a copy that cannot
be shown to be v6 is not a rollback point. The v7 step takes no backup of its own.

## Consequences

- Capture latency grows from ~4.2 s to ~25.5 s per attempt — and that is the point: 4.2 s bought a
  dead-letter. Capture stays a spawned background drain; the first-inject barrier still fail-opens at
  2 s. Do **not** wire `autoCaptureIterationTimeout` (30 s) into this path: the measured structured call
  is 85 % of that budget, and the provider's own 60 s `DEFAULT_TIMEOUT` bounds it.
- `ComposedRuntime.provider` changing type is a compile error for anything that expected an
  `AiProvider` there — by design.
- `HostStructured` invalidates the cached origin on ANY error, not only `Network`/`Timeout`: a foreign
  process on a recycled port answers `401`/`404`/junk, and caching that origin pins a stranger for the
  engine's life.
- New public surface: `StructuredCapture`, `CaptureProvider`, `HostStructured` + `new`, `Structured` +
  `new`, `privacy::filter_request_for_delivery`. Everything else stays `pub(crate)`.

## Scope guard — this change does NOT make capture persist

This ADR fixes the manual/fallback arm's structured output only. As amended by the wire wave, what
remains outside it:

1. **The create-request rebuild.** Session create, the prompt route and the reply are now on the
   documented wire, and the model/agent travel on the create body — but the port still sends
   `permission` where the document declares `permissions: Permission.Ruleset`. That is a rebuild, not a
   rename, and `requeue-captures` is still unshipped. Runtime confirmation that the live host behaves as
   documented is likewise unmeasured: no exchange has ever been captured. **Closed by D-29-7:** issue 108
   removed the create/prompt/delete lifecycle entirely; `requeue-captures` remains unshipped and is named
   in D-29-12.
2. **The wrapper never sends `agent_id`** (`plugin/src/plugin.js`), and `mem-episodic` rejects an empty
   one — `episodic_write_failed`. **Addressed by `95857b7`**; whether that closes persistence live is not
   measured here.

No sentence in this ADR, and none in the landing commit, may claim that capture resumes end to end
because of this change.
