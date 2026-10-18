# 10 — W2.2 · Remote provider clients — 7 adapters (US 4.1)

Status: needs-triage

## What to build

Shape-identical adapter clients behind the W2.1 trait: Anthropic Messages, OpenAI chat-completion, OpenAI responses, Google Gemini, Minimax, Atlas Cloud, OrcaRouter. Build shared request/response plumbing first (retries, timeouts, structured-output parsing), then one exemplar (Anthropic), then the remaining providers as independent parallel slices — the phase's maximal-parallelism wave. Host-path variants of the connectivity tests are ported in W5 (see roadmap). Wave detail: `.scratch/rust-port/roadmap.md` §W2.2.

## Acceptance criteria

- [ ] Behaviors ported: `anthropic-provider`, `openai-chat-completion-provider`, `minimax-provider`, `atlas-cloud-provider`, `orcarouter-provider` (plus `v2-provider-connectivity`/`v2-provider-refresh` host variants in W5).
- [ ] All adapters meet the shared trait contract; shared plumbing is not duplicated per provider.
- [ ] Retry/timeout/structured-output behavior uniform across adapters.

## Blocked by

- 09 · W2.1
