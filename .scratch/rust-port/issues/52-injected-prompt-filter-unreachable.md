# 52 — Privacy · The injected-prompt filter is implemented, tested and unreachable on all three paths

Status: ready-for-agent — 2026-10-27 (verified unreachable at HEAD: filter and middleware exist, constructed only in tests; both config keys have no reader; brief under `## Agent Brief`)
user had typed it, and summarises it into memories: the filter that exists to prevent exactly that has zero
production call sites, and the two config keys that would enable it have zero readers.**

## The correction this issue is written under — read first

An earlier finding said *"our port parses `filterInjectedPrompts` and has no filter at all."* **That is
wrong, and no acceptance criterion below may be written from it.**

The filter exists, 204 lines, tested: `crates/mem-ai/src/privacy/injection.rs` —
`contains_injection_marker` (`:31`), `MessagePart` with `synthetic` (`:59-66`), `is_injected_part`
(`:101`), `filter_injected_parts` (`:109`),
`InjectionFilter` (`:139`), `is_internal_prompt` (`:131`) — and it re-exports
`DEFAULT_INJECTION_MARKERS` / `normalize_injection_markers` from `mem-core` (`:25`) so the two cannot drift.
The middleware exists too: `crates/mem-ai/src/privacy/layer.rs` — `PrivacyConfig` (`:38`),
`PrivacyFilterLayer::new` (`:74`), `PrivacyFilterProvider` (`:111`), `filter_request` (`:135`).

The defect is **implemented but unreachable**, the same shape as `LearningRound` (issue 47) and
`IdleScheduler` (issue 48): `PrivacyFilterLayer::new` has exactly **two** call sites, both tests
(`crates/mem-ai/tests/privacy/privacy_layer.rs:118`, `:322`); `PrivacyFilterProvider` is constructed
nowhere outside `layer.rs`; and **no** predicate from the module is called anywhere outside `privacy/`
itself — `filter_injected_parts`, `is_injected_part`, `contains_injection_marker`, `InjectionFilter::new`
and `is_internal_prompt` all return zero hits in `crates/` and `src/` outside the module.

**Do not add a second filter.** `injection.rs:20-21` and `layer.rs` already warn that the markers and the
normalizer are re-exported *"so the two cannot drift"*; a new implementation is the drift.

## Measured state — do not re-derive

| what | where it stands |
|---|---|
| the predicates | `crates/mem-ai/src/privacy/injection.rs` — `contains_injection_marker` `:31`, `MessagePart`/`synthetic` `:59-66`, `InjectionFilter` `:139`, `is_internal_prompt` `:131` |
| the marker list | `crates/mem-core/src/config/normalize.rs:272-281` — eight defaults, including `<!-- OMO_INTERNAL_INITIATOR -->`, `[SYSTEM DIRECTIVE: OH-MY-OPENCODE`, `<system-reminder>`, `<team_mode_status`, `<auto-slash-command>` |
| the middleware | `crates/mem-ai/src/privacy/layer.rs:38-63` (`PrivacyConfig`), `:74` (`new`), `:135-155` (`filter_request`) |
| **request-side semantics** | `privacy/mod.rs:91-102`, **D8**: *"REQUEST-SIDE ONLY"* — filtering applies to what the layer sends; everything returned is forwarded verbatim. `complete` drops injected user messages and errors if all were dropped |
| **production provider mint** | `crates/mem-bridge/src/composition.rs:142-164` — the **only** place a production `AiProvider` is constructed (`OpenCodeHostProvider`), so the wrap point is unique |
| the config keys | `chat_message_filter_injected_prompts` `schema.rs:544` (default `:612`, resolved `config/mod.rs:384`), `injection_markers` `schema.rs:748` — **zero production readers** for either |
| the store seam | `crates/mem-bridge/src/capture.rs:805-810` — *"Private content is stripped from every free text BEFORE any durable write"*, **no injection filter beside it** |
| the adapter's part data | the adapter **does** receive parts (`plugin/src/plugin.js:658-659`, `client.session.messages()`; and `output.message.parts` at `:590-593`), and **flattens them to a string before the boundary** (`extractText` `:450-469`; `parseIdleTranscript` `:698-710`) |

## The three layers — different seams, different risk

The follow-up asked the right question about layer (a). The answer: **the adapter has the part-level data;
the Rust boundary does not.** `extractText` (`plugin.js:450-469`) joins every part's `text` with `\n` and
never looks at `type` or `synthetic`, so `MessagePart.synthetic` in Rust (`injection.rs:59-66`) is a type
nothing ever constructs from real input.

**(a) Structural / `synthetic` — the adapter's problem, not the store's.**
Upstream filters at `chat.message`, where parts are present and `synthetic` is set on every injected block
(`example/opencode-mem/src/index.ts:657-658`, after `:646`). Ours captures on `session.idle` from a
flattened transcript, so the flag is gone by the time any Rust code sees the text. Two honest options, and
the issue must pick one and say why:

- **a1 — filter in the adapter** (`parseIdleTranscript`/`extractText`): the data is there, the change is
  local, and it keeps the boundary a string.
- **a2 — carry part metadata across the boundary**: `transcript` gains a parts-shaped alternative; the
  store-side filter then applies `InjectionFilter` with real `synthetic` values.

**(b) The marker filter on the store path — the live defect.**
Textual markers work on a flattened string, so (b) is implementable without (a) — but the module's own
contract says structural-first is *"not sufficient on its own"* and textual *"carries most of the weight"*
(`injection.rs:9-18`). So (b) can land first and is still incomplete without (a).

Placement ruling: **the filter goes at `capture.rs:805-810`, beside `strip_private_content`**, because that
line already carries the exact invariant this filter needs (*before any durable write*), and it is the
single point every capture passes. The same argument extends to `raw_context` at `:1008` (retained verbatim
and fed to the vault at `:857-863`). One seam, two free-text inputs, no new module.

**(c) Request-side `PrivacyFilterLayer` — real, and not a substitute.**
Wiring is one wrap at the unique mint (`composition.rs:142-164`) plus the missing
`PrivacyConfig::from_core(&mem_core::Config)` — the same shape as D9's deferred config→resolver wiring
(`privacy/mod.rs:103-106`). But D8's semantics make it **insufficient alone for this path**: `complete`
*drops* injected user messages and returns an error when every message was dropped. A capture summarizer
request carries exactly one user message, so a contaminate prompt would turn the layer into a **capture
failure**, not a clean capture. Therefore (c) is defence-in-depth for the module's stated promise
(`privacy/mod.rs:1-2`) and **(b) is what fixes the product defect**.

## The live consequence — and one inference to confirm

- **Memories: live today.** `capture.rs:821` summarises the transcript and `:846-879` commits an episodic
  row and a tier-1 memory. Injected blocks are in that transcript, so boilerplate becomes memories.
- **Retained raw context: live today.** `capture.rs:995-1008` retains `raw_context` verbatim (only
  `<private>` stripped) and the vault feeder takes a copy (`:857-863`).
- **`user_prompts`: latent.** `save_prompt` (`crates/mem-profile/src/prompt.rs:174`, insert at `:220`) has
  **zero production callers** — only tests. This is a fourth link in the same "implemented, never
  constructed" chain, and it matters to issue 47: a wired learner would read an **empty** prompt table.
- **The profile: latent twice** — no prompts are written, and no learner is constructed (issues 47/48).
- **One inference, to be confirmed by the gate in 52.5:** our own injected block is pushed with
  `synthetic: true` (`plugin.js:592`) onto the *current user message's* parts, and `parseIdleTranscript`
  reads the last user message's parts including that block. If the host returns synthetic parts in
  `session.messages()`, then the block we inject is captured on the next idle and summarised into a new
  memory — **a self-feeding loop**, and the marker list does not contain our own
  `<project_agent_session_history>` / `<memory_context>` headers. Confirm before relying on it.

## Ownership and coupling (do not duplicate issue 47/48)

- **This issue owns** the store-side filter ((a)+(b)) and the request-side wrap ((c)).
- **Issue 47 owns** the profile's existence and erasure class; **48** owns the learner wiring. **This issue
  owns the coupling**: a learner wired without 52 learns from boilerplate, so **52 blocks 48's learner leg
  when 47 rules `A1`**. 48 records it; this issue does not restate 47's decision.
- **Issue 50 owns** the `<memory_context>` wrapper. That wrapper will be one more injected block a capture
  can pick up — 52's tests include it once 50 lands (**50 → 52.4/52.5**), and neither issue may assume the
  other's marker coverage.
- **Issue 49 owns** the two config keys' *census disposition*; this issue gives them readers. 49 must cite
  52 and move both keys from "genuinely inert, no landing issue" to "forward declaration with a landing
  issue".

## What to build

Put the existing filter on the three paths that need it, using the existing types, and retire the deferral
sentences that have outlived their wave.

The deferral is stale, and that is a finding. `privacy/mod.rs:84` and `injection.rs:129` defer to *"Phase
5's `plugin-host-config` wave"*; `privacy/mod.rs:112` and `crates/mem-ai/CONTEXT.md:12` defer to *"Phase 5
importing"* the predicate; `normalize.rs:268-271` says only the filter functions are *"the port of **a later
wave**"* — while the port already landed. `plugin-host-config` is **a test name**, not a wave: it appears
once in the roadmap, as one of the **five** items in **W5.1**'s verification list (`roadmap.md:227` — one
`tests/…test.ts` path plus four bare `plugin-*` names), and W5.1 is **issue 19, `done` 2026-10-20**. The two
`plugin-host-config` pointers name a closed target; the three "Phase 5" pointers name a lane of which only
W5.2 (issue 20, V2 adapter + TUI) is still open — and that lane cannot own a prompt predicate. Either way
the item was deferred to a target that closed without it: the same defect as issue 33's phantom lane, and
the reason (c) cannot be left where it is.

## Acceptance criteria

- [ ] **52.1** **No second implementation.** Every change below uses `mem-ai`'s existing predicates, types
      and marker list; the marker list stays `mem-core`'s (`normalize.rs:272`) re-exported through
      `injection.rs:25`. A diff that adds a filter module or a second marker list is rejected on sight.
- [ ] **52.2** **The store-side seam is one place.** The filter runs at `capture.rs:805-810`, adjacent to
      `strip_private_content`, before any durable write; and on `raw_context` at `:1008`. No other caller
      invents its own filter call.
- [ ] **52.3** **Config reaches the filter for the first time.** A `PrivacyConfig::from_core(&mem_core::Config)`
      exists and is the only constructor used in production **where the layer is wired**; if 52.6 re-homes
      the request-side wrap, the re-homed issue owns the first production use and 52.3's behavioural test
      still runs on the store-side seam. `chat_message_filter_injected_prompts` and `injection_markers` gain
      real readers. A test flips each key and observes the behaviour change (filter on ⇒ marker text absent
      from the durable row; filter off ⇒ present).
- [ ] **52.4** **Both refusal shapes are right.** A prompt that is *entirely* injected produces a typed
      skip (the capture writes neither tier), not a memory summarising boilerplate; a *mixed* prompt keeps
      the user text and drops the injected block. `is_internal_prompt` (`injection.rs:131`) exempts our own
      analysis/summary prompts so filtering can never zero a legitimate capture. Tests for all three.
- [ ] **52.5** **(a) is decided and implemented, not assumed.** Either a1 (filter in
      `parseIdleTranscript`/`extractText`, `plugin.js:450-469`, `:698-710`) or a2 (carry part metadata across
      the boundary) — the choice and its reason are recorded. **Gate:** capture one real idle payload and
      record whether our own `synthetic` block appears in `session.messages()`; if it does not, the
      self-feeding loop of §"The live consequence" does not exist and this AC narrows to foreign blocks.
      Either way the test asserts our own pushed block (`plugin.js:592`) is never captured.
- [ ] **52.6** **(c) is wired or explicitly re-homed.** `PrivacyFilterLayer` wraps the provider at the
      unique mint (`composition.rs:142-164`) with `PrivacyConfig::from_core`, **or** a numbered issue owns
      that wiring and `privacy/mod.rs:112`'s deferred-list entry points at it. A deferred list is not an
      owner.
- [ ] **52.7** **The stale pointers are corrected** in `privacy/mod.rs:84`, `:112`, `injection.rs:129`,
      `crates/mem-ai/CONTEXT.md:12` and `mem-core/src/config/normalize.rs:268-271`: they name this issue (or
      52.6's re-homed issue), or — for `normalize.rs`, which names no wave — the sentence is retired because
      the port landed.
- [ ] **52.8** **No behaviour change to `<private>` handling** (`strip_private_content` and its order are
      untouched); the two mechanisms compose, in the order 52.2 places them.
- [ ] **52.9** **The chain link is recorded for 47/48:** `save_prompt` (`prompt.rs:174`) has no production
      caller, so wiring prompt recording is a *separate* step that must land with or after 52 — and 48's
      learner leg is blocked by this issue when 47 rules `A1`. Recorded here and in 48, not duplicated.
- [ ] **52.10** No new dependency; `mem-bridge` already depends on `mem-ai` (`capture.rs` uses
      `mem_ai::privacy::strip_private_content` at `:810`, `:1008`, `:1170`, `:1254`), so the predicates are
      importable where the filter must run.

## Blocked by / Blocks

- **Blocked by:** nothing for 52.1–52.4, 52.7–52.10. 52.5's gate needs one **real** idle payload from a
  live OpenCode host — the bridge (issue 19) is in place but a payload needs a host run. If that is not
  available, record the deferral and the reason rather than guessing the answer.
- **Blocks:** **48's learner leg when 47 rules `A1`** (a profile trained on boilerplate is the failure this
  issue exists to prevent); **49's dispositions** for the two keys (they move from bucket (a) to (b)).
- **Cross-reference:** 50's `<memory_context>` wrapper becomes a captured block candidate once it lands
  (50 → 52.4/52.5). 47 owns the profile's class; this issue owns the contamination that a class cannot
  fix — erasure removes a contaminated profile, prevention stops it being written.

## Comments

- Filed 2026-10-25 as a **follow-up to the 45–51 set**, from the owner's correction of an earlier finding.
  That correction is incorporated verbatim in the header: the filter is not absent, it is unreachable.
- **Facts re-verified at `551dca1` before filing**, because the first version of this finding was wrong:
  `PrivacyFilterLayer::new` call sites (`tests/privacy/privacy_layer.rs:118`, `:322` only); zero
  non-`privacy/` call sites for all five predicates; `ChatMessage.filter_injected_prompts`
  (`schema.rs:747`) and `injection_markers` (`:748`) with no production reader; `save_prompt` with no
  production caller; the provider mint at `composition.rs:142-164`; `engine.rs:908-913` passing `None`
  into the composition root (the root builds the host adapter itself — so the mint is *inside*
  `composition.rs`, not absent).
- **Why a new issue rather than ACs inside 49.** 49 is a census: its deliverable is a table plus one
  behaviour gate. The substance here is three behavioural paths in three crates with three different owners,
  one of which (the request side) 49 cannot even reach. Folding it in would put a cross-crate behaviour
  change into a documentation-shaped issue — and 49's own scope note already says the keys' *wiring* belongs
  to the issues. 49 keeps the census disposition and gains a citation; this issue owns the work.
- **Deviations recorded** (also in `.tmp/drafts/20261025-concerns-issue-map.md`): (c) stays in this issue
  rather than getting a number of its own, because `PrivacyConfig::from_core` is shared plumbing and
  splitting it across two tickets is how the "port it a second time" drift starts — with an explicit
  re-home clause in 52.6 if the owner prefers the Phase 5 lane. And the audit's config buckets shift:
  two of the 32 "no landing issue" keys now have one (**32 − 2 = 30**; **13 + 2 = 15**) — my arithmetic on
  the audit's numbers, not a re-derivation.

## Agent Brief

**Category:** bug
**Summary:** the injected-prompt filter exists, is tested, and has zero production call sites — so injected
boilerplate is still captured as user text.

**Current behavior (verified at HEAD `206cf9f`):**
`mem-ai`'s privacy module carries the filter, the internal-prompt predicate and the middleware, all with tests.
They are **unreachable**: the middleware is constructed only inside tests and no predicate is called from
outside the privacy module. The two config keys that would enable them resolve into the config and are read by
**nothing**; there is no `PrivacyConfig::from_core`. Capture applies only private-tag stripping before its
durable writes, in both the capture path and the raw-context path. The wrapper's transcript extraction flattens
parts and ignores their type, so a synthetic block cannot be distinguished from user text on the way in.

**Desired behavior:**
The filter runs at the store-side seam, adjacent to the existing private-content stripping and before any
durable write, and on the raw-context path too — one place, not a call per caller. The two config keys gain
their first production readers through a single `PrivacyConfig::from_core`, and a test flips each key and
observes the behaviour change (filter on ⇒ marker text absent from the durable row; off ⇒ present). Both
refusal shapes are right: a prompt that is *entirely* injected produces a typed skip and writes neither tier,
while a *mixed* prompt keeps the user text and drops the injected block; the internal-prompt predicate must
exempt the port's own analysis prompts so filtering can never zero a legitimate capture. The request-side
question is decided rather than assumed, with its gate measurement recorded. Any deferred wiring is owned by a
numbered issue, not by a deferred list.

**Key interfaces:**
- The existing predicates, types and marker list — **no second implementation**. A diff that adds a filter
  module or a second marker list is rejected on sight.
- `PrivacyConfig::from_core(&Config)` — the only production constructor where the layer is wired.
- The capture write seam next to the private-content stripper, and the raw-context write.
- The deferral pointers in the privacy module, its `CONTEXT.md` and the config normaliser — they name this
  issue and must be corrected when it lands.

**Acceptance criteria:**
- [ ] The issue's 52.1–52.7, unchanged.
- [ ] The gate measurement in 52.5 is recorded, not inferred: capture one real idle payload and record whether
      the port's own synthetic block appears in the host's message list.
- [ ] A second filter implementation, or a second marker list, fails review.

**Out of scope:**
- Reimplementing or re-tuning the filter and its markers.
- The request-side wrap, unless this wave takes it — a re-homed issue may own it, but a deferred list is not
  an owner.
