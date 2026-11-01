# 98 — `chatMessage.enabled` does not gate capture: a user turning memory off keeps writing rows

Status: needs-triage — 2026-11-01. The behaviour is MEASURED; the fix site is genuinely undecided (wrapper
or engine — see "Open question"), so this is filed for a maintainer decision before any wave. What is
already known: no consumer of the key exists on the capture path in either layer.

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

## Open question (why this is `needs-triage`, not `ready-for-agent`)

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

- [ ] The fix site is chosen and recorded (ADR or an issue comment), with the reason the other site was
      rejected.
- [ ] With `chatMessage.enabled = false`, an idle event produces **no** capture request at the wire —
      asserted with the stub-engine harness (issue 91 / `plugin/tests/wrapper-glue.test.mjs`), not by
      reading code.
- [ ] With it `true` (the default), the same event still captures — the gate is a gate.
- [ ] An unreadable or absent config does not silently disable capture: the fail-open direction is stated
      and tested.
- [ ] Issue 63's premise table gains a row naming this key's **capture**-side reader, so the same key is
      not claimed unread twice.

## Blocked by

None. Coordinate with issue 63, which owns the **injection** gate for the same key — the two must agree on
where enforcement lives, or the key will be honoured on one path and silently ignored on the other.

## Evidence

`example/opencode-mem/src/index.ts:646`; `plugin/src/plugin.js:1153-1198`;
`crates/mem-core/src/config/mod.rs:357-386`; `crates/mem-ai/src/privacy/layer.rs:74-77`; issue 63. Found
by the 2026-11-01 issue-set audit as a finding with no owning issue.
