# 01 — W0.1 · Workspace & crate skeleton

Status: done — 2026-10-18

## What to build

Stand up the multi-crate Cargo workspace that every later wave lands in, with the split: `mem-core`, `mem-store`, `mem-embed`, `mem-ai`, `mem-profile`, `mem-episodic`, `mem-web`, `mem-bridge`, plus a thin `main` binary composing them. Create per-crate `CONTEXT.md` stubs matching the repo's multi-context domain-doc layout, a root `CONTEXT-MAP.md` listing every context, and codify crate dependency rules (no cycles). Hello-level skeleton only — everything compiles clean. Wave detail and crate ownership notes: `.scratch/rust-port/roadmap.md` §W0.1.

## Acceptance criteria

- [ ] `cargo build`, `cargo clippy -D warnings`, `cargo test` green across the workspace.
- [ ] All 8 crates exist per the roadmap split; acyclic dependency rule documented and enforced.
- [ ] `CONTEXT-MAP.md` at root lists all crates; per-crate `CONTEXT.md` stubs in place.
- [ ] Thin `main` binary composes the crates (no behavior yet).

## Blocked by

None - can start immediately.

## Comments

- **2026-10-18 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261018-0935-w01-workspace-skeleton.md` (sha256 `6119647bd091`): coder → one rework iteration (ledger-shape fix) → reviewer `STATUS: APPROVED` (independent re-run: `cargo fmt --all --check` / `cargo build --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` green, `cargo test --workspace` 3/3, 10-mandate security gate clean). Wave ledger: `/tmp/opencode/progress/20261018-0935-w01-workspace-skeleton.ledger.md` — 7/7 steps DONE. Deliverable: 8-crate workspace (`mem-core`…`mem-bridge`) + thin composition binary, cargo-tree dependency-edge enforcement test, `CONTEXT-MAP.md` + per-crate `CONTEXT.md` stubs, ADR-0001.
