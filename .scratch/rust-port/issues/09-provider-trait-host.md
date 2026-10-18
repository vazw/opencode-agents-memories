# 09 — W2.1 · Provider trait + OpenCode host provider (US 4.1)

Status: needs-triage

## What to build

`mem-ai::provider` — `#[async_trait] pub trait AiProvider` (all providers behind one deep seam), provider factory/config resolution, OpenCode host provider (via host SDK/HTTP), and the `internal-capture-sessions` loop guard that prevents capture feedback loops. Wave detail: `.scratch/rust-port/roadmap.md` §W2.1.

## Acceptance criteria

- [ ] Behaviors ported: `ai-provider-config`, `internal-capture-sessions`, `opencode-provider`.
- [ ] Every provider (host + remote) is reachable through the single trait; no provider-specific leakage above it.
- [ ] Capture-loop isolation provable by test (internal sessions never re-captured).

## Blocked by

- 02 · W0.2
- 07 · W1.4
