# 02 — W0.2 · Config loading (jsonc) + error/logging strategy

Status: done — 2026-10-18

## What to build

`mem-core::config` — loads `opencode-mem.example.jsonc`-shaped config: jsonc parsing (comments tolerated), env overrides, defaults, validation; surface later-needed thresholds in the schema now (e.g. episodic consolidation age). Establish the repo-wide error strategy (typed errors at seams, `thiserror`-based) and `tracing`-based logging mirroring upstream `logger.ts`. Wave detail: `.scratch/rust-port/roadmap.md` §W0.2.

## Acceptance criteria

- [ ] Behaviors ported from upstream: `config.test.ts`, `config-resolution.test.ts`, `jsonc.test.ts`.
- [ ] Resolution chain: defaults < config file < env overrides; validation errors name the offending key.
- [ ] Typed error tree in place; no stringly-typed errors at crate seams.
- [ ] Logging levels/formatting match upstream behavior.

## Blocked by

- 01 · W0.1

## Comments

- **2026-10-18 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261018-1027-w02-config-errors-logging-r1.md` (sha256 `dd9d5e1321f3`, SYS-APPROVED 2026-10-18T10:36Z): coder (11/11 steps, RED→GREEN per slice gate) → reviewer `STATUS: APPROVED` with **no rework** (independent re-run: `cargo fmt --all --check` clean / `cargo build --workspace` 0 warnings / `cargo clippy --workspace --all-targets -- -D warnings` exit 0 / `cargo test --workspace` 82 mem-core + 3 root tests, all ok; L1–L6 ledger-shape asserts pass; `cargo audit` 0 advisories across 64 deps; 10-mandate security gate clean). Two coder deviations accepted as improvements: `DatabaseEncryptionCipher` = 7 variants (upstream `config.ts:145-151` beats the blueprint's "8"); `Error::Config`/`Error::Logging` use `#[error("{0}")]` delegation instead of `transparent` — preserves the `source()` chain hop `test_error_source_chain_preserved` requires. Wave ledger: `/tmp/opencode/progress/20261018-1027-w02-config-errors-logging-r1.ledger.md` — 11/11 steps DONE. Deliverable: `mem-core` config (strict jsonc parse seam, schema/defaults/normalizers, resolution chain, provider status), typed error tree, `tracing` logging sink + init; `docs/adr/0002-error-logging-strategy.md`; `crates/mem-core/CONTEXT.md`.
