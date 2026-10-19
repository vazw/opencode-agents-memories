# 10 — W2.2 · Remote provider clients — 7 adapters (US 4.1)

Status: done — 2026-10-19

## What to build

Shape-identical adapter clients behind the W2.1 trait: Anthropic Messages, OpenAI chat-completion, OpenAI responses, Google Gemini, Minimax, Atlas Cloud, OrcaRouter. Build shared request/response plumbing first (retries, timeouts, structured-output parsing), then one exemplar (Anthropic), then the remaining providers as independent parallel slices — the phase's maximal-parallelism wave. Host-path variants of the connectivity tests are ported in W5 (see roadmap). Wave detail: `.scratch/rust-port/roadmap.md` §W2.2.

## Acceptance criteria

- [ ] Behaviors ported: `anthropic-provider`, `openai-chat-completion-provider`, `minimax-provider`, `atlas-cloud-provider`, `orcarouter-provider` (plus `v2-provider-connectivity`/`v2-provider-refresh` host variants in W5).
- [ ] All adapters meet the shared trait contract; shared plumbing is not duplicated per provider.
- [ ] Retry/timeout/structured-output behavior uniform across adapters.

## Blocked by

- 09 · W2.1

## Comments

- **2026-10-19:** Closed. Reviewer APPROVED (session `ses_eace4ef69ffeAufWcdozaFjEJo`, DeepSeek V4.1 Flash Free after 7 Beam Free rate-limit failures). Test evidence: 532 passed / 0 failed / 2 ignored (baseline 459 + 73 new). Fan-out: all 7 slices absorbed by lead (rejected-and-absorbed — `registry.rs` cannot compile until every provider module exists, so per-shard standalone verification was impossible as sequenced). Security gate 10 Mandates all PASS/N/A; `cargo audit` exit 0 / 0 vulns (3 pre-existing warnings via `turso`/`tokenizers`). RED→GREEN non-vacuousness corroborated by reviewer (plumbing.rs sha256 byte-identical to pre-mutation backup, no mutation residue). Non-blocking findings: (1) ledger Step-10 stamp stale (recurring habit, bug-pattern reinforced); (2) `tower-http`/`http` listed in blueprint deps but only `tower` added (benign, transitive); (3) hardening — `base_url` not constrained to https, `PROTECTED_BODY_KEYS` omits `text`/`generationConfig`/`responseSchema` (operator-controlled config only). Recorded divergences: Gemini key via `x-goog-api-key` header (not `?key=` URL param). Next: W2.3 (privacy filters, issue 11).
