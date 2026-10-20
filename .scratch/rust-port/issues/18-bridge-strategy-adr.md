# 18 — W5.0 · Bridge-strategy decision — JSON-RPC stdio vs N-API (ADR) (US 5.2)

Status: done — 2026-10-19 (grilling complete: 12 decisions resolved; ADR to be written at W5.1 start per issue 19 prerequisite)

## What to build

The Phase 5 entry decision gate: choose the bridge between the JS/TS OpenCode plugin shell and the Rust engine — JSON-RPC over stdio (default stance in the roadmap: reversible, host-agnostic, matches the V1→V2 server/client split) vs an N-API addon (higher ceiling, per-host prebuild burden). **User grilling is mandatory** before closing. Output: an ADR in `docs/adr/`. Wave detail: `.scratch/rust-port/roadmap.md` §W5.0.

## Acceptance criteria

- [ ] ADR written: decision, alternatives (stdio JSON-RPC vs N-API), reversibility analysis, latency/dependency trade-offs.
- [ ] User grilling session completed on the trade-offs.
- [ ] Message-schema implications for W5.1 noted (frozen before W5.1 starts).
- [ ] W5.1 unblocked with the chosen strategy.

## Blocked by

- None — gates 19 (W5.1); take at Phase-4 completion.
