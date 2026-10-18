# 05 — W1.2 · Turso connection lifecycle & shard management (US 2.1)

Status: done — 2026-10-18

## What to build

`mem-store` on the `turso` crate 0.8.x (`Builder::new_local`, tokio-native — see roadmap §0b D1): connection manager, readiness gates, RAII (`Drop`) handle release, shard manager/rotation/inventory/path-migration. Start with the shared `turso-db` wrapper port; then connection/lifecycle, rotation/inventory, and path-migration as parallel slices. Wave detail: `.scratch/rust-port/roadmap.md` §W1.2.

## Acceptance criteria

- [ ] Behaviors ported: `turso-connection-manager`, `turso-connection-lifecycle`, `turso-ready-gate`, `sqlite-handle-release`, `turso-shard-rotation`, `turso-shard-recreate`, `shard-inventory`, `shard-path-migrate`, `turso-auxiliary-db-upgrade`.
- [ ] Handle release is deterministic via Rust `Drop` (no leaks under repeated open/close).
- [ ] Verbatim upstream SQL statements run unchanged on the `turso` crate (golden transliteration begins here).

## Blocked by

- 04 · W1.1

## Comments

### 2026-10-18 — Closed: W1.2 complete (post-rework)

Wave complete and verified after one rework cycle.

- **Original review**: REJECTED — two rejection-grade defects: (1) `migrate()` never acquired the operation lock; (2) `assert_path_inside_storage` admitted mid-path `..` chains.
- **Rework** (debugger diagnosis + coder iteration 1): both closed — `migrate()` acquires `.turso-operation.lock` at mutation-phase entry (RAII through teardown; live foreign lock → `MigrationLocked` before any rename); containment now lexically resolves `..`/`.` in both paths (mid-path chains rejected on open + journal-recovery seams; `..x.db` / `sub/../ok.db` still allowed; no `canonicalize`).
- **Re-review**: APPROVED — 6 new tests (non-vacuous, fresh-RED spot-checked), workspace battery **292 passed / 0 failed / 1 ignored**, ledger 16 steps (L1–L6 green), archive-override site immune + test-locked, `shard_manager.rs` / `operation_lock.rs` untouched.

Evidence: re-review report (2026-10-18); debugger diagnosis + FIX PRESCRIPTION; ledger `/tmp/opencode/progress/20261018-1332-w12-turso-connection-shards-r1.ledger.md`. First git commit landed at this close (W0.1–W1.2 snapshot).
