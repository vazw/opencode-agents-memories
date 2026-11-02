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
   **Correction:** `handshake_result` receives only the engine's launch config today. The resolved
   boolean must be carried on the engine (set from `mem_core` config when the runtime is composed,
   defaulting to `true` on the runtime-less fixture path, which still answers hello).
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
- [ ] Issue 63's premise table gains a row naming this key's **capture**-side reader, so the same key is
      not claimed unread twice.

---

## Blocked by

None. Coordinate with issue 63, which owns the **injection** gate for the same key — the two must agree on
where enforcement lives, or the key will be honoured on one path and silently ignored on the other.

## Evidence

`example/opencode-mem/src/index.ts:646`; `plugin/src/plugin.js:1153-1198`;
`crates/mem-core/src/config/mod.rs:357-386`; `crates/mem-ai/src/privacy/layer.rs:74-77`; issue 63. Found
by the 2026-11-01 issue-set audit as a finding with no owning issue.
