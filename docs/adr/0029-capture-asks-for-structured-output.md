# 0029 — Capture asks for structured output, through a capability that cannot drop the schema

- **Status:** Proposed
- **Date:** 2026-11-01
- **Builds on:** `docs/adr/0027-host-endpoint-discovery.md` (the host transport),
  `docs/adr/0028-manual-capture-provider.md` (which provider capture uses),
  `docs/adr/0008-w53-capture-injection.md` D8 (filter the request, never the reply).
- **Owner ruling:** the capture summarizer must ask for a schema-bound completion instead of
  repairing seven adapters, so an eighth adapter cannot repeat the defect.

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

**D-29-6 — The host fixture answers a FUTURE wire.**
The rows in `crates/mem-bridge/tests/host_capture_provider.rs` fake top-level `{"id": …}` on session
create and `info.structured_output` on the prompt. The running 2.2.5 host has neither, so those rows
evidence the WIRING; they are not evidence about the owner's live host, and `host_capture_e2e.rs`
continues to pin the measured 2.2.5 body dead-lettering with a named reason.

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

This ADR fixes the manual/fallback arm's structured output only. Two confirmed defects outside this
change still block persistence, each with its own blueprint:

1. **The host arm's 2.2.5 wire drift.** Session create answers `{data: Session.Info}`, the prompt lives
   at `/api/session/{id}/generate`, and structured output is gone — capture currently fails with
   `errorKind: "protocol"` on that arm.
2. **The wrapper never sends `agent_id`** (`plugin/src/plugin.js`), and `mem-episodic` rejects an empty
   one — `episodic_write_failed`.

No sentence in this ADR, and none in the landing commit, may claim that capture resumes end to end
because of this change.
