# 09 — W2.1 · Provider trait + OpenCode host provider (US 4.1)

Status: done — 2026-10-19

## What to build

`mem-ai::provider` — `#[async_trait] pub trait AiProvider` (all providers behind one deep seam), provider factory/config resolution, OpenCode host provider (via host SDK/HTTP), and the `internal-capture-sessions` loop guard that prevents capture feedback loops. Wave detail: `.scratch/rust-port/roadmap.md` §W2.1.

## Acceptance criteria

- [ ] Behaviors ported: `ai-provider-config`, `internal-capture-sessions`, `opencode-provider`.
- [ ] Every provider (host + remote) is reachable through the single trait; no provider-specific leakage above it.
- [ ] Capture-loop isolation provable by test (internal sessions never re-captured).

## Blocked by

- 02 · W0.2
- 07 · W1.4

## Comments

- **2026-10-19:** Closed. Reviewer APPROVED (session `ses_ead0a4f35ffebtOpti61OjZt93`). Test evidence: 459 passed / 0 failed / 2 ignored (baseline 439 + 20 new). Fan-out: both lanes absorbed by integration-wave owner (rejected-and-absorbed rationale recorded in ledger). Security gate 10 Mandates all PASS/N/A. SecretResolver trait defined in mem-ai (W2.3 implements later). Host provider uses HTTP host-fetch/SDK (reqwest), not stdio. One divergence: count_tokens returns ceil(chars/4) estimate (no host endpoint). Next: W2.2 (remote providers, issue 10) and W2.3 (privacy, issue 11) both unblocked.
