# 04 — W1.1 · Memory scope & tags (US 2.1)

Status: done — 2026-10-18

## What to build

`mem-core::scope` + `mem-core::tags` — scope hashing, container-tag parsing/extraction, strict tag validation with traversal rejection; tag CRUD plus migration-skip behavior. Wave detail: `.scratch/rust-port/roadmap.md` §W1.1.

## Acceptance criteria

- [ ] Behaviors ported: `memory-scope.test.ts`, `memory-scope-helper.test.ts`, `client-scope-traversal.test.ts`, `tags.test.ts`, `project-scope.test.ts`, `tool-scope.test.ts`.
- [ ] Scope/container-tag traversal attempts are rejected (path and tag validation).
- [ ] Container-tag parsing matches upstream `extractScopeFromContainerTag` behavior.
- [ ] Tag CRUD + migration-skip behavior (`tag-migration-*` behaviors) covered.

## Blocked by

- 02 · W0.2

## Comments

- **2026-10-18 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261018-1208-w11-scope-and-tags-r1.md` (sha256 `de18f22f3821`, SYS-APPROVED 2026-10-18T12:43Z): coder (9/9 steps, RED→GREEN per slice) → reviewer `STATUS: APPROVED` with **no rework** (independent re-run: `cargo fmt --all --check` clean / `cargo build --workspace` 0 warnings / `cargo clippy --workspace --all-targets -- -D warnings` exit 0 / `cargo test --workspace` **188 passed / 0 failed / 0 ignored** (126 baseline + 62 new: 60 pinned integration + 2 in-crate SHA-256 unit tests); docs content battery + root `context_docs` green, registry at 13 rows; L1–L6 ledger-shape asserts pass with stamps cross-checked live against file mtimes; `cargo audit` exit 0, no advisories (64 deps); 10-mandate security gate clean — no violations (trusted-git shim refusal test-pinned, strict lowercase-hex parse + frozen branch order, probe no-panic collapse); `NAME-CHECK` over 29 spot-pinned identifiers — none missing/renamed). Accepted notes: 62-vs-60 test composition (both in-crate unit tests pin real standard constants); two GREEN-phase test-side fixes in the sha256 slice (`hex` fed digest bytes; lowercase-digit assertion) — implementation never wrong; `tags.rs` held at the pinned 9 (3 redundant drafts removed); row-9 in-band ledger correction confirmed benign; win32 `git.cmd` branch `cfg(windows)` + UNVERIFIED-HERE with three win32 upstream tests recorded N/A. Wave ledger: `/tmp/opencode/progress/20261018-1208-w11-scope-and-tags-r1.ledger.md` — 9/9 steps DONE. Deliverable: `mem-core::sha256` (vendored std-only FIPS 180-4, NIST-vector-verified, constants compile-time-derived, no new deps); `mem-core::scope` (ScopeHash traversal wall; `extract_scope_from_container_tag` frozen branch order; `resolve_memory_scope` user-then-project wildcards; `resolve_scope_selection`); `mem-core::tags` (`TagInfo`/`TagResolver` identity algebra marker > git-common > remote > path; `GitProbe` seam; `SystemGitProbe` trusted-git scan with canonicalize-before-containment, repo-local git-shim refused; `tags::migration` skip/soft-failure/reset/clamp); `docs/agents/testing.md` testing-conventions + registry (13 rows); `crates/mem-core/CONTEXT.md` glossary update.
