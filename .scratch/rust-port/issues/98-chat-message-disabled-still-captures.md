# 98 — `chatMessage.enabled` does not gate capture: a user turning memory off keeps writing rows

Status: ready-for-agent — 2026-11-02. Fix site decided: **coordinated gate** — the engine stays the sole
config authority, the hello handshake advertises the resolved flag, the wrapper gates before any host call,
and the engine refuses at `capture`. Brief at the end of this file.

## What to build

Upstream stops capturing when the user disables chat-message memory. The port does not.

MEASURED:

- Upstream: `capturePrompt` returns `false` before doing anything when the switch is off —
  `example/opencode-mem/src/index.ts:646`:
  `if (!isConfigured() || !CONFIG.chatMessage.enabled) return false;`. The V2 adapter treats that `false`
  as "suppress this turn" (`example/opencode-mem/src/v2/adapter.ts:155-159`).
- Port, wrapper: `fireIdleCapture` (`plugin/src/plugin.js:1153-1198`) runs unconditionally — it awaits the
  bridge, reads `client.session.messages`, and fires the capture. `grep -rn 'chatMessage\|enabled'
  plugin/src` returns **nothing**; the wrapper has no config surface at all.
- Port, engine: the key *is* resolved — `crates/mem-core/src/config/mod.rs:357-386` populates
  `schema::ChatMessage { enabled, … }` (`schema.rs:555`, default `true` at `:624`) — and outside
  `config/mod.rs` only the privacy fields are read (`crates/mem-ai/src/privacy/layer.rs:74-77`:
  `filter_injected_prompts`, `injection_markers`). `chat_message.enabled` has **zero** consumers.
  `grep -rn 'chat_message\.' crates --include=*.rs` is the measurement.

Consequence: a user who turns memory off still gets captures written, prompts stored and the profile fed.
That is a privacy expectation, not a cosmetic switch — it is the one config key whose whole content is
"do not record me".

## Decision (2026-11-02) — coordinated gate, not a single site

Recorded after the design was proposed and then verified against the tree — not accepted on the
proposal's word. Four parts:

1. **`mem_core` stays the sole config authority.** No config parsing is added to the JavaScript wrapper:
   it cannot read this project's config, and a second parser would drift across XDG paths, env overrides
   and schema defaults.
2. **The hello handshake advertises the resolved value**, in a new top-level `config` object in the hello
   result: `{"chat_message_enabled": <bool>}`.
   **Correction to the proposal it came from:** the value must NOT go inside `capabilities`. That map's
   invariant is "this build answers method X" — all fourteen keys are `true` by construction and the
   wrapper's reader is `capabilities[name] === true`, so a config-derived `false` would be
   indistinguishable from an old build that omits the key, which is exactly the fail-open case. Keeping
   it separate also leaves `capabilities.len() == 14` and its test untouched.
3. **The wrapper gates first.** After the existing `ensureStarted()` await in the idle-capture path —
   which resolves only after hello — return when the advertised value is exactly `false`: no
   `client.session.messages(...)`, no capture id, no wire request.
   **Correction (superseded by C1 below):** the first draft said the boolean must be carried on
   `Engine`/`EngineRuntime`. That was wrong — those are filled independently of the runtime, so a carrier
   there can advertise `true` while the enforcing service behaves otherwise. It lives on
   `CaptureService` instead; see C1.
4. **The engine refuses too.** `capture` answers `{"accepted": false, "reason": "chat_message_disabled"}`
   before any enqueue — the existing success shape is `{"accepted": true}`, so this mirrors it. It
   protects direct RPC callers, which never pass through the wrapper.

**Fail-open:** check `=== false`. An engine that predates the key, or an absent/blank config block,
continues to capture — stated and tested, per AC4.

**Rejected alternative:** engine-only enforcement plus rewriting AC2 ("no capture request at the wire")
into "no row persisted". It is a smaller diff, and it was the orchestrator's first call. Rejected because
AC2 is satisfiable as written — one hello key and one fixture update remove the transcript read entirely —
and a privacy switch that still collects the transcript on every idle event is weaker than one that never
asks for it.

**Verified before recording, not inherited from the proposal:** `capabilities` is an object of fourteen
booleans with a test pinning `len == 14`; the wrapper's capability reader is `=== true`; the idle-capture
path awaits `ensureStarted()` before touching the session; the capture ACK shape is `{"accepted": true}`;
and the engine does not reload config at runtime, so the advertised value cannot go stale relative to the
engine's own view.

### Cross-check (architect, 2026-11-02, against upstream `9086c65`)

**VERDICT: ACCEPT-WITH-CHANGES.** The idea holds, and the justification is *stronger* at the new pin than
at ours: upstream gates at the `chat.message` hook **before** it fetches `session.messages`, so with the
switch off the user-observable behaviour is "the transcript is never fetched", not merely "nothing is
stored". Only the wrapper can reproduce that ordering in a split architecture — which is what makes the
coordinated gate right and the engine-only alternative weaker.

Nine changes, all adopted:

- **C1 — the flag lives on `CaptureService`**, not `Engine`/`EngineRuntime`/`EngineConfig`:
  `chat_capture_enabled` (default `true` in `::new`, which gains no argument), builder
  `with_chat_capture_enabled`, getter `chat_capture_enabled()`; `composition.rs` chains it from
  `config.chat_message.enabled` right after `.with_privacy(...)`; `handshake_result` gains the boolean and
  `hello()` derives it from the runtime's capture service; `runtime_for` changes nothing. Evidence: the
  service already consumes resolved config (context bytes, sound memory, privacy); the builder idiom is
  documented for exactly this reason ("seventeen `::new` call sites keep their argument list"); and the
  `EngineRuntime` literals are constructed in six places independently of the runtime, so a carrier there
  can disagree with the enforcer.
- **C2 — an end-to-end oracle in the production wire test**: a real `Config` with the key off must make
  `hello` answer `false` **and** `capture` refuse, from the same engine. Without it, a missed builder call
  leaves every test green and the switch dead, because the failure mode is fail-open.
- **C3 — amend ADR-0007 D-43 *and* D-46** (the `capture` result column is normative in D-46), plus
  `docs/bridge-schema.md` §2/§3. No new ADR, no protocol bump.
- **C4 — bare boolean, no `reason` field.** Upstream's new provider-status toast fires only for
  `autoCaptureEnabled && !provider.ready`, never for this key; that other fact is already resolved
  separately as `auto_capture_provider_status`, so it is a separate key if it is ever surfaced.
- **C5 — refusals are observable once per process**: one latched structured stderr line
  (`capture.refused`, `reason: "chat_message_disabled"`) on the first refused capture. Without it an old
  wrapper against a new engine stops remembering silently — `fireCapture` discards the result.
- **C6 — the wrapper caches per child**: store the advertised value beside `engineCapabilities` and clear
  it in `teardownChild`, or a dead child's `false` leaks onto a restarted child.
- **C7 — gate inside `call_capture`**, after the runtime/degraded checks and before `project_id`
  validation and enqueue — not inside `enqueue` (that would need a new `ObjError` variant to distinguish
  "disabled" from store faults).
- **C8 — do not extend the gate to `add`/manual capture.** Upstream gates the chat path only; the boundary
  is recorded so a later wave does not "fix" it by disabling explicit memory writes.
- **C9 — wrapper: one check, one place**, immediately after `ensureStarted()`, skipping `readSession`,
  `resolveSessionAgent` and `session.messages` (three host round-trips, matching upstream's ordering),
  with one diagnostic line so the no-op is distinguishable from a broken engine.

Also recorded: `plugin/dist/plugin.js` is a byte-identical tracked twin and `hello-response.json` fails an
exact schema-equality compare — both are part of the change surface. Issue 63 needs no wrapper early
return (an empty block is already a no-op), but its premise table's upstream-gate column is now wrong for
three rows: those gates moved to `services/memory-recall.ts` at `c81a04d`.

## Open question (historical — resolved by the decision above)

Two fix sites, neither obviously right:

1. **Wrapper.** Upstream gates there because its config lives there. In this port the wrapper has no config
   reader; the host hands it a `config` hook (`plugin/src/plugin.js:954-956`, currently used only for
   `applyStructuredOutputAgentConfig`), but that hook receives the **host's** config, not this project's
   `opencode-mem` config file. Gating on it would read the wrong file.
2. **Engine.** `CaptureService` receives `CaptureParams` after the wrapper has already fired; gating there
   rejects the write at the inbox door, which costs one round-trip per idle event and stores nothing —
   but it is the only layer that actually knows the resolved config.

Deciding between them is the issue. Do not start a wave that guesses.

## Acceptance criteria

- [x] The fix site is chosen and recorded, with the reason the other site was rejected — see the
      2026-11-02 decision above (this file; the ADR-0007 D-43 handshake precedent is the parent).
- [ ] With the key `false`, an idle event produces **no** capture request at the wire **and** no
      `client.session.messages(...)` transcript read — asserted with the stub-engine harness (issue 91),
      not by reading code.
- [ ] With it `true` (the default), the same event still captures — the gate is a gate.
- [ ] An unreadable or absent config does not silently disable capture: the fail-open direction is stated
      and tested (engine predating the key, absent `config` block, and blank value).
- [ ] A direct `capture` RPC with the key `false` answers `{"accepted": false, "reason":
      "chat_message_disabled"}` and persists nothing — the engine half of the gate.
- [ ] The hello result advertises the resolved value in a `config` object, and `capabilities` keeps
      exactly its fourteen existing keys.
- [ ] **End-to-end, from a real config (C2):** a production-composed engine built from a `Config` with the
      key off answers `hello` with `false` **and** refuses `capture` — the oracle that makes a missed
      builder call fail instead of fail open.
- [ ] **Observable refusal (C5):** the engine emits exactly one latched structured line on the first
      refused capture, and none on subsequent ones.
- [ ] **Per-child cache (C6):** a child advertising `false` that dies and is replaced by one advertising
      `true` (or omitting `config`) captures again — the cached value is cleared with the child.
- [ ] **Boundary (C8):** `add` and manual memory writes are unaffected by this key, and a test says so.
- [ ] Issue 63's premise table gains a row naming this key's **capture**-side reader, so the same key is
      not claimed unread twice; its upstream-gate column is corrected for the three rows whose gates moved
      to `services/memory-recall.ts` at `c81a04d`.

---

## Blocked by

None. Coordinate with issue 63, which owns the **injection** gate for the same key — the two must agree on
where enforcement lives, or the key will be honoured on one path and silently ignored on the other.

## Evidence

Upstream at `9086c65` (v2.39.0): `src/index.ts:670` (`capturePrompt` returns false when the key is off),
`:735` (`memoryContext.enabled()`), `:767` (the `chat.message` early return, **before** the transcript
fetch), and the recall gates that moved to `src/services/memory-recall.ts` at `c81a04d`. Port:
`plugin/src/plugin.js` (idle capture), `crates/mem-core/src/config/mod.rs:357-386` (resolution),
`crates/mem-ai/src/privacy/layer.rs:74-77` (the only other readers, unrelated). Issue 63. Found by the
2026-11-01 issue-set audit as a finding with no owning issue.

---

## Agent Brief

> *This was generated by AI during triage.*

**Category:** bug (privacy)
**Summary:** make `chatMessage.enabled` actually stop auto-capture — in the wrapper before any host call,
and in the engine before any enqueue — without adding a second config reader.

**Current behavior.**
The key resolves into the core config (`chat_message.enabled`, default `true`) and is read by nothing.
On every idle event the wrapper resolves a session agent, reads the full transcript from the host, and
posts a `capture` request; the engine persists a row, embeds it and feeds the profile. A user who turns
memory off keeps being recorded. The JS wrapper has no reader for this project's config file, and the
host-supplied config hook carries the host's config, not ours.

**Desired behavior.**
When the key is `false`:

- the wrapper returns immediately after the engine start await — no session read, no agent resolution,
  no transcript fetch, no capture request;
- the engine refuses a `capture` request from any caller before validation and enqueue, answering
  `{"accepted": false, "reason": "chat_message_disabled"}`, and persists nothing.

When it is `true` (default), absent, blank, or the engine predates the advertisement, capture behaves
exactly as today. The gate covers the chat-capture path only — `add` and explicit memory writes are
unaffected.

**Key interfaces.**

- `CaptureService` gains a `chat_capture_enabled: bool` field defaulting to `true`, a builder
  `with_chat_capture_enabled(bool)`, and a getter `chat_capture_enabled()`. It gains no constructor
  argument: the builder idiom exists precisely so the existing `::new` call sites keep their argument
  lists.
- The composed runtime chains it from the resolved `chat_message.enabled`, beside the other
  config-derived setters on that service. The core config remains the only authority on the value.
- The engine's `hello` result gains a new top-level `config` object: `{"chat_message_enabled": <bool>}`.
  It must not go inside `capabilities` — that map means "this build answers method X", every key is
  `true` by construction, and a config-derived `false` there would be indistinguishable from an old
  build that omits the key.
- The hello value and the `call_capture` refusal read the same service field, so they cannot disagree.
  Gate inside the capture dispatch, not inside the inbox enqueue.
- The wrapper stores the advertised value as per-child state beside the engine capabilities, and clears
  it when the child is torn down.

**Acceptance criteria.**

1. With the key off, an idle event sends no `capture` request and makes no transcript read — asserted
   against the stub-engine harness, not by reading code.
2. With it on (default), the same event still captures.
3. Absent/blank config and an engine that predates the key both continue to capture — fail open, tested.
4. A direct `capture` RPC with the key off answers `{"accepted": false, "reason":
   "chat_message_disabled"}` and persists nothing.
5. The `hello` result carries `config.chat_message_enabled`, and `capabilities` still has exactly its
   fourteen existing keys.
6. A production-composed engine built from a real config with the key off answers `hello` `false` **and**
   refuses `capture` — end to end, from the same engine object.
7. The engine emits exactly one latched structured line on the first refused capture, and none after.
8. A child advertising `false` that dies and is replaced by one advertising `true` (or omitting the key)
   captures again.
9. `add` and explicit memory writes are unaffected, and a test says so.
10. The capture-side reader is recorded in issue 63's premise table, and the three upstream-gate rows
    whose gates moved to `services/memory-recall.ts` at `c81a04d` are corrected there.

**Out of scope.**

- The injection-side gate for the same key — issue 63 owns it; an empty block is already a no-op, so no
  wrapper early return is needed there.
- `add`/manual memory writes, which upstream leaves ungated.
- Config hot-reload: the engine reads config at startup, so a mid-session edit takes effect on the next
  engine spawn. That limitation is stated in the ADR amendment, not fixed here.
- Provider-availability status: upstream's new toast covers "enabled but no provider ready", a different
  fact already resolved as `auto_capture_provider_status`. If it is ever surfaced it enters the same
  `config` object as its own key, never as a `reason` on this one.
- Any config parsing in JavaScript.

**Durable record.** Amend ADR-0007 D-43 and D-46 (the `capture` result column is normative there), and
`docs/bridge-schema.md` §2/§3. No new ADR and no protocol bump: absence of the key remains the
compatibility signal, and default `true` coincides with the fail-open direction.

**Rollback.** Revert the commit. No schema migration, no store format change, no protocol bump.
