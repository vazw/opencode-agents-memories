# 03 — W0.3 · Test conventions & shared utils (context-limit, paths)

Status: done — 2026-10-18

## What to build

Document the upstream-test → Rust-test mapping convention (integration tests in `crates/*/tests/`; upstream fixtures become Rust subprocess/helper fixtures) in `docs/agents/testing.md`, with three dispositions per upstream test (port / covered-by / N-A). Port the shared utils: `context-limit` (token-budget trimming — order must match upstream), math utils, windows-path handling. Wave detail: `.scratch/rust-port/roadmap.md` §W0.3.

## Acceptance criteria

- [ ] Behaviors ported: `context-limit.test.ts`, `windows-path.test.ts`.
- [ ] Testing-conventions page written, including the fixture-translation recipe.
- [ ] Shared utils live in `mem-core` and are consumable by later waves.

## Blocked by

- 02 · W0.2

## Comments

- **2026-10-18 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261018-1135-w03-test-conventions-utils.md` (sha256 `eb5320a589a9`, SYS-APPROVED 2026-10-18T11:42Z): coder (8/8 steps, RED→GREEN per slice) → reviewer `STATUS: APPROVED` with **no rework** (independent re-run: `cargo fmt --all --check` clean / `cargo build --workspace` 0 warnings / `cargo clippy --workspace --all-targets -- -D warnings` exit 0 / `cargo test --workspace` **126 passed / 1 ignored / 0 failed**; focused suites `context_limit` 12, `math` 12, `windows_path` 15, `fixture_recipe` 2+1 (bounded-kill proven live); L1–L6 ledger-shape asserts pass; `cargo audit` 0 vulnerabilities; 10-mandate security gate clean; ports verified faithful against the reference clone). Accepted notes: E0433-vs-E0432 RED-class difference; documented slice interleaving via the shared `utils/mod.rs`; test-side 22→21-byte marker fix (non-vacuity evidence); clippy final form `split('/').rev().find(...)`. Wave ledger: `/tmp/opencode/progress/20261018-1135-w03-test-conventions-utils.ledger.md` — 8/8 steps DONE. Deliverable: `mem-core::utils` (`context_limit` — byte-exact marker + frozen branch order per R9; `math` — f64 cosine/L2 normalize; `paths` — `normalize_separators` + lexical `get_project_name`), `docs/agents/testing.md` (three-disposition taxonomy + fixture-translation recipe + seeded mapping registry), executable fixture-recipe demonstrator (`crates/mem-core/tests/fixture_recipe.rs` + `common::run_fixture_child`).
