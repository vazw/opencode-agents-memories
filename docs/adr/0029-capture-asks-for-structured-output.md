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
`!retryable` class or a `retryAfterMs` above `HOST_RETRY_AFTER_CEILING_MS` (30 000 — a policy choice for
the owner, not a measurement), and otherwise runs the unchanged five-attempt ladder.
**A >30 s hint dead-letters PERMANENTLY:** dead rows are excluded by `SELECT_CAPTURE_INBOX_BATCH`
(`status = 'queued'`) and nothing in the tree requeues them; the row and its `payload` survive
(`DEADLETTER_CAPTURE_INBOX` is an `UPDATE … status='dead'`), so a future requeue tool can recover it.
`EntryOutcome::Deferred` was rejected on three grounds: it consumes no attempt (nothing bounds the loop), it
honours no hint without a scheduling column (the next tick re-issues immediately), and it already means "a
live lock or an unreadable scope shard", so reusing it would erase a diagnostic distinction.
**Chosen bounded behaviour:** a `ProviderRequestError` with an absent or unknown `reason` stays RETRYABLE —
`reason` is required with a closed enum, so a conformant host cannot send a permanent 502 without one; the
cost asymmetry (four extra requests versus permanent loss) decides it, and the ladder bounds it at five.

**D-29-11 — Two tracked deferrals.**
(1) The capture inbox has no scheduling column, so a bounded `retryAfterMs` is honoured as a refusal, not a
delay, and a >30 s hint is permanent — `.scratch/rust-port/issues/110-capture-inbox-not-before-column.md`.
(2) `build_bounded_summary_prompt`'s byte accounting is two bytes short of the schema block it appends —
`.scratch/rust-port/issues/111-summary-prompt-budget-two-byte-gap.md`. Neither is a defect this wave may
absorb silently.

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
   in D-29-10/D-29-11.
2. **The wrapper never sends `agent_id`** (`plugin/src/plugin.js`), and `mem-episodic` rejects an empty
   one — `episodic_write_failed`. **Addressed by `95857b7`**; whether that closes persistence live is not
   measured here.

No sentence in this ADR, and none in the landing commit, may claim that capture resumes end to end
because of this change.
