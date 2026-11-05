# 84 — structured-output schema is dead on 7 of 8 streaming provider arms; capture is the only caller that sets one

Status: done — 2026-11-04 (landed — 2026-11-01, commit `585956d` (`fix(bridge): capture asks for structured output, not the
streaming arm`), residual owned here: the measured probe artifacts still sit in gitignored `.tmp/scratch/`
and must be copied to `docs/evidence/` (AGENTS.md forbids `.tmp`-only evidence). Gate record:
`.tmp/progress/20261101-0953-capture-schema-r5.ledger.md` steps 1–7 (final `cargo test --workspace`
= 2306 passed / 0 failed; `node --test plugin/tests/*.test.mjs` = 167 pass / 0 fail). The acceptance
criteria below were met per that record and are left unticked because no gate was re-run for this edit.

## What is missing

`crates/mem-bridge/src/capture_prompt.rs` used to set `request.json_schema = Some(CAPTURE_OUTPUT_SCHEMA)`
and then call `self.provider.complete(request)` — the **streaming** arm. Seven of the eight concrete
`complete` impls build their stream body with the schema slot hardcoded `None`:

| provider | stream body |
|---|---|
| `openai_chat.rs:242` | `build_body(…, None, true, …)` |
| `anthropic.rs:265` | own builder, `None` |
| `minimax.rs:132` | delegates to the anthropic builder with `None` |
| `gemini.rs:260` | own builder, `None` |
| `openai_responses.rs:237` | own builder, `None` |
| `atlas_cloud.rs:105` | → `openai_chat::complete_with` |
| `orcarouter.rs:112` | → `openai_chat::complete_with` |

Only `opencode_host.rs:895-900` → `run_chunks` honours it (`:466-472`). So "7 arms drop it" is
**5 distinct defective wire builders plus 2 delegations** — a fix in `openai_chat::complete_with` alone
would close three arms, which is exactly why the chosen design does not patch builders at all.

MEASURED with the engine's exact request shape (`.tmp/scratch/probe-stream.py`, output in
`.tmp/scratch/probe-stream.out`): 80 SSE deltas, 308 bytes of markdown
(`## Request / ## Outcome … **Tags**: …`), `parses as JSON: NO`. The same model answers a non-streaming
structured call with a valid `tool_calls` payload whose arguments carry a non-empty `summary`
(`.tmp/scratch/probe-capture-format.py`, probe A, 25.5 s).

Upstream parity: upstream capture is **non-streaming** structured output
(`example/opencode-mem/src/services/auto-capture.ts` → `generateStructuredOutput`;
`services/ai/opencode-provider.ts:291,434`). The port invented "stream with a schema it then ignores".

## The consequence that survives a dead-letter fix

The schema is what asks for `filesTouched` / `completedWork` / `decisions` / `openThreads`. Without it the
model returns upstream's base `{summary, type, tags}`; `parse_capture_output` accepts it and
`CaptureExtension::default()` (`capture.rs:770-777`) stores four empty vectors. Nothing errors. US 1.1's
structured payload and US 1.4's `touched_files` filter then match nothing, with a green suite.

## What lands

- Capture goes through `complete_structured` via a mem-bridge-local seam (the `AiProviderStructured`
  trait is sealed at `crates/mem-ai/src/provider/trait.rs:51-56`, so an out-of-crate `impl` is
  impossible; coercing `Arc<dyn AiProviderStructured>` into the seam's halves is legal).
- The privacy layer gains `filter_request_for_delivery`, carrying the empty-conversation guard that
  used to exist only inside `PrivacyFilterProvider::complete` (`layer.rs:226-241`) — otherwise moving
  capture off `layer.wrap` turns a named fast failure into a full round-trip with only the system
  prompt left.
- Provider selection's return type narrows so "a capture provider with no structured capability"
  becomes unrepresentable at the summarizer; selection precedence itself is untouched line-for-line.
- `host_arm` keeps `let _ = host_provider.warm();` (`engine.rs:1409` as of this edit; `:1391` when this
  issue was filed — the only production `.warm()` call) — without it `Auto-capture provider ready` never
  fires, because that line is emitted only from `LazyHostProvider::resolve()`
  (`host_provider.rs:119`).
- Residual: `docs/evidence/` gains the two probe transcripts and their scripts. Today they exist only
  under gitignored `.tmp/scratch/`, which is not a durable artifact; the 308-byte markdown and the
  `parses as JSON: NO` observation are quoted above so the measurement survives the copy.

## Acceptance criteria

- [ ] RED row: a provider whose streaming arm drops the schema replays the measured 308-byte markdown;
      `summarize()` must return `Ok(Capture { summary non-empty })`. It failed before the fix with
      `capture summary JSON parse failed`, at `.expect(..)`, before either counter assertion.
- [ ] The RED is not satisfiable by fixing only the manual arm — a second row pins the host arm.
- [ ] The **four** selector rows in `crates/mem-bridge/tests/memory_provider.rs` (`:225` precedence,
      `:319`, `:343`, `:556` claimed-but-unusable) stay green and their declared mutants stay lethal.
- [ ] `crates/mem-bridge/tests/composition_privacy.rs`'s D-52-4 oracle keeps both original assertions
      and is re-pointed at `complete_structured`; the crate still compiles (its
      `build_composed_runtime` call sites pass the narrowed type).
- [ ] `filter_request_for_delivery` has a row for "all messages injected → `Err`, inner calls `0`".
- [ ] Neither the wire (issue 88) nor the payload (issue 83) is claimed fixed by this change, and no
      doc says capture now persists.
- [ ] The probe transcripts and scripts are copied out of `.tmp/scratch/` into `docs/evidence/`.

## Parent

Slice issue 93 (`93-slice2-structured-extension-populated.md`).

## Comments

**Why the tests missed it:** every capture test stubbed `AiProvider` (`ScriptedProvider`,
`RecordingProvider`, `CountingProvider`) and emitted whatever chunks the test chose, so the real
streaming body was never built. The adapter's own test asserted `tool_choice == "required"` on the
**structured** body only (`openai_chat.rs:390-396`).

**The type's own doc comment was wrong:** `crates/mem-ai/src/provider/request.rs:67-69` says providers
"pass the schema through unchanged". Nothing enforced that, and 7 of 8 did not.

**Scope note:** this repairs the **fallback** arm. While host-first precedence is live, capture fails on
the host wire instead (issue 88), so this alone changes nothing observable.

**Approval is not verifiable by hashing the blueprint.** `sha256=bb6587cedf3e` is
`blueprint_sha256:` inside the header of `.tmp/blueprints/blueprint-20261101-0953-capture-schema-r5.md`;
`sha256sum` of that file is `4125cea37a1f…`. Cite the header line and the gate/ledger records
(`.tmp/progress/20261101-0953-capture-schema-r5.ledger.md`), not a computed digest.
