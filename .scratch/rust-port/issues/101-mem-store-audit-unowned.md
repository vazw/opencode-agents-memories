# 101 — `crates/mem-store` has 16.7k lines and no issue owns an audit of the surface the 2026-11-01 audit skipped

Status: done — 2026-11-05 (audit report published in docs/evidence/issue-101-mem-store-audit/README.md; defect issue 127 filed)

## What to build

The `opsec` audit of 2026-11-01 declared a named part of `crates/mem-store` out of scope and said so in
its own report. Nothing in the tracker picked it up, so the largest unexamined surface in the tree has no
owner while fourteen defect issues were filed from the parts that *were* read.

MEASURED:

- `find crates/mem-store/src -name '*.rs' | xargs wc -l` = **16 724** lines across 32 modules.
- `docs`/audit record (`.tmp/progress/opsec-audit-20261101T090956Z.md:167`), verbatim in substance: it did
  not audit `shard_manager.rs` rotation internals, `legacy_migrator.rs`, `migration_service.rs`,
  `reaping.rs`, `delete_guard.rs` or `shard_facts.rs` count-drift arithmetic, and called this "the most
  plausible home for a *silent corruption* bug I did not find".

**Correcting the second half of the audit's own claim.** "Tracked by no issue" is true only of the
*audit*. Several of those modules' features already have owners, and this issue must not re-open them:

| module / behaviour | already owned by |
|---|---|
| one-active-shard rotation, reactivation, reconcile skip | issue 43 W4 (landed `030a0bd`, ADR-0022) |
| retired-shard reaping | issue 43 W7 (landed `43d9954`) |
| sidecar deletion contract (`delete_guard.rs`) | issue 46 |
| `legacy_migrator` reconcile / `ActiveShardConflict` | issue 77 (`-22-2` residual) |
| count-drift claims (`vector_count`, `shard_stats`) | issue 43.2 / 43.3, both adjudicated |
| scope-wide delete-by-id | issue 72 |

What remains unowned is the **read**: nobody has examined the arithmetic and lock ordering of
`migration_service.rs`, `shard_path_migration.rs`, `shard_inventory.rs`/`shard_registry_sql.rs`,
`prompts.rs`, `user_prompts_sql.rs` or the `shard_facts` count paths for silent corruption. Features being
landed is not the same as their internals having been reviewed — this repository's own rule: a landed wave
is not an audit.

## Acceptance criteria

- [x] The work is shaped (one audit issue with a report, or a per-module split) and recorded, so the next
      auditor does not have to re-derive the boundary above.
- [x] A report lands under `docs/evidence/` (never `.tmp/` only), naming file:line for every finding,
      separating MEASURED from INFERRED, and listing what was read versus skipped — the specific failure of
      the 2026-11-01 audit was scope it did not write down until the end.
- [x] Any finding with a fix gets its own defect issue; the audit issue itself writes no fix.
- [x] The counts in this issue (16 724 lines, 32 modules) are re-derived at the time of the audit, not
      inherited from here — they move with every wave.
- [x] It explicitly re-derives, rather than inherits, the claims issues 43.2/43.3 adjudicated
      (`vector_count` self-heals on the write path; `shard_metadata.embedding_model` is written but read
      by nothing).

## Blocked by

None. Deliberately after the capture wave — it is a read, not a blocker.

## Evidence

`.tmp/progress/opsec-audit-20261101T090956Z.md:167` (the declared exclusion); `crates/mem-store/src/`
(32 files, 16 724 lines, measured 2026-11-01); issues 43, 46, 72, 77 (the feature owners listed above).
Found by the 2026-11-01 issue-set audit as a finding with no owning issue.

## Comments

### 2026-11-05 — closed by blueprint-issue-101-mem-store-audit

1. Measured and re-derived `crates/mem-store/src/*.rs` line count: 16 870 lines across 32 modules (delta +146 lines from 2026-11-01 baseline).
2. Audited the six previously unexamined modules (`migration_service.rs`, `shard_path_migration.rs`, `shard_inventory.rs`/`shard_registry_sql.rs`, `prompts.rs`, `user_prompts_sql.rs`, `shard_facts.rs` count paths) across lock ordering, arithmetic safety, error propagation, and silent data corruption risks.
3. Re-derived claims from issues 43.2 (`vector_count` write-path self-healing verified via `SYNC_COUNT_SQL` and `sync_shard_vector_count`) and 43.3 (`shard_metadata.embedding_model` actively read and evaluated by `read_model_facts`, `prepare_shard_facts`, and `migration_service.rs`).
4. Published audit report under `docs/evidence/issue-101-mem-store-audit/README.md`.
5. Filed defect issue `.scratch/rust-port/issues/127-reembed-source-omits-session-id.md` for the omission of `session_id` in `SELECT_REEMBED_SOURCE`.
6. Verification: 0 `.rs` files modified, parity matrix script passes 110/110 OK. Status flipped to `done — 2026-11-05`.
