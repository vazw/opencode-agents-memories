# 0032 — Capture-Time Retention Stamp with Deterministic Grace

- **Status:** Accepted (owner ratification, 2026-11-09).
- **Date:** 2026-11-10
- **Extends:** ADR-0019 (erasure posture; closes 47.4 residual row), ADR-0021 (schema version discipline: v4 migration), ADR-0013 (production error handling: panic-free run guard).
- **Supersedes:** Alternatives A (superseded 2026-10-25 design), B (ratified restart design, doubly superseded), C (never-delete-prompts).

## Context

Issue 39 replaces the 501 stub at `POST /api/cleanup` with an operational retention sweep. Prior to this work, memory retention faced a dual challenge:
1. Memory shards and user prompts live in separate SQLite database files with no distributed transaction support.
2. An offline gap (where the engine does not run for a period exceeding the retention window) must not cause memories to immediately co-expire without grace, while repeated or failed sweeps must never immortalize memories.

Earlier attempts to solve this problem led to three rejected or superseded alternatives:

### Grounds for Rejected / Superseded Alternatives

- **Alternative A (superseded 2026-10-25 design — sweep-time prompt link traversal):**
  Traversed `linked_memory_id` foreign keys at sweep time across database boundaries. Rejected because: cross-file transactions are impossible without distributed locks; prompt links are unpopulated at runtime (`UserPromptManager::link_memory_to_prompt` has zero callers); and scanning unindexed cross-shard links creates unbounded latency and partial failure modes.
- **Alternative B (the ratified restart design, doubly superseded — sweep-time stamp-to-now):**
  Proposed mutating memories during the sweep by stamping candidate rows with `now` to grant a fresh grace window. Rejected because: introducing stateful mutation at sweep time violates determinism, makes sweep runs order-dependent, risks immortalizing memories if repeated sweeps fail or abort midway, and depends on wall-clock time rather than immutable stored facts.
- **Alternative C (never-delete-prompts):**
  Proposed keeping prompts in `user-prompts.db` indefinitely so prompt-birth facts and links remain intact forever. Rejected because: violates data minimization, allows `user-prompts.db` to grow unboundedly, and directly breaches privacy erasure mandates (ADR-0019).

## Decisions

### D-32-1 — The Hybrid Rule with Corrected Deterministic Grace ($k=2$)

Memory eligibility for retention sweep deletion is governed by the hybrid formula:
$$\text{eligible} \iff \text{updated\_at} < \text{cutoff} \land (\text{source\_prompt\_created\_at IS NULL} \lor \text{source\_prompt\_created\_at} < \text{cutoff} - \text{retention})$$

To rescue memories created shortly before an offline gap, a deterministic grace window is computed strictly from stored facts:
$$\text{grace\_exempt} \iff \text{prompt\_birth} + 2 \cdot \text{retention} > \text{cutoff}$$

**Why $k=2$:**
In revision 1 ($k=1$, $p + r > c$), for stamped rows where $s = p$ (the production case: the stamp is the prompt birth), deletion required $p + r < c$ ($p < c - r$) while exemption required $p + r > c$. These conditions are strictly disjoint for every $p$, rendering grace a provable no-op for production stamped rows. Revision 2 adopts $k=2$ ($p + 2r > c$), the minimal integer multiple where the deletion candidate set ($p < c - r$) and exemption set ($p > c - 2r$) overlap in the non-empty window $(c - 2r, c - r)$.

The effective boundary for stamped rows moves from $s < c - r$ to $s \le c - 2r$.

For pre-v4 NULL-stamp rows, grace is granted if their live linked prompt satisfies $p' + 2r > c$. If phase 3 deletes that prompt first, the next run finds no link and the NULL-stamp row is deleted by the base rule; the row survives exactly one run after its prompt's death.

### D-32-2 — The D2 Writer Rule: Capture-Time Stamp by a Single Named Writer

The `source_prompt_created_at` column is written **solely at capture time** by `write_capture_summary`.
When persisting a new memory summary, `write_capture_summary` queries the prompt's stored `created_at` from `user-prompts.db` via `PromptStampSource::get_prompt_created_at` and populates `MemoryInsert.source_prompt_created_at`. No other code path writes or mutates this column.

### D-32-3 — Cross-File Discipline and the Ordering Invariant

The prompts read/delete and the memory deletes live in separate files; no cross-file transaction spans them. The ordering invariant holds:

> Every interleaving leaves grants no shorter than intended: the grace is `prompt_birth + 2·retention > cutoff`, recomputed each run from stored facts — in-shard from the stamp for stamped rows, from the linked prompt's `created_at` for NULL-stamp linked rows — a failed run writes nothing and moves no deadline, and the failure mode is "a prompt lingers one run", never "a memory dies early".

The sweep executes in four idempotent phases:
1. **Read (prompts DB):** reads expired prompt facts (`created_at < cutoff`); derives `grace_ids` and the watermark for profile invalidation.
2. **Sweep per shard (`ShardManager::run_retention_sweep`):** applies migrate-on-touch (`ensure_shard_schema`), queries candidates with `RETENTION_CANDIDATES_SELECT`, applies the single Rust filter (skipping pinned memories, stamped grace $s + 2r > c$, and NULL-stamp grace `grace_ids`), and deletes survivors in a single `BatchMode::Write`.
3. **Erase + Delete Prompts + Clear Links (`UserPromptManager`):** joins profile invalidation (B2′) before deleting expired prompts, then clears `linked_memory_id` on surviving prompts whose memories were deleted.
4. **VACUUM:** best-effort vacuum on `user-prompts.db`.

### D-32-4 — Profile Invalidation Join (47.1-B2′) and ADR-0019 47.4 Residual Closure

Retention pruning of prompts joins profile invalidation, reusing the exact B2′ machinery landed in issue 48 (`ProfileEraser` hook on `UserPromptManager`).

Phase 3 queries `MIN(created_at)` across the expired prompt batch and calls `erase_derived_from(batch_min)` before executing `DELETE FROM user_prompts`. Following the E2 rule (erase before delete), a failed erasure aborts before prompt deletion, failing safely toward privacy.

**ADR-0019's 47.4 residual row ("retention-pruned prompts are a named residual until 39 decides") is hereby CLOSED** (resolved, not renamed).

### D-32-5 — Shard v4 Schema Migration and INSERT Freeze

- Shard schema migrates to v4 via `ALTER TABLE memories ADD COLUMN source_prompt_created_at INTEGER;` (`MEMORY_SHARD_MAX_KNOWN = 4`). Pinned columns count advances from 18 to 19.
- Golden SQL freeze is preserved: `crates/mem-store/src/sql.rs` and `tests/golden_sql.rs` remain byte-untouched. `crates/mem-store/src/vector.rs` defines local constants (`INSERT_WITH_STAMP_TAGS` and `INSERT_WITH_STAMP_WITHOUT_TAGS`) with `source_prompt_created_at` appended last, matching the `encryption_migrator.rs:151-157` local-copy precedent.

### D-32-6 — Run Guard & Fused Maintenance Port

- `ProductionDeduplication` implements `MaintenancePort::cleanup()` using an `AtomicBool` run guard. Concurrent requests receive `PortError::Invalid("cleanup is already running")` mapping to HTTP 200 with `success: false`.
- The handler delegates to `MaintenancePort::cleanup()` and maps `PortError::Unavailable` to HTTP 501 with `"cleanup service unavailable"` (fixing the generic 500 error).

## Residuals and Accepted Limitations

1. **Far-Edge Residual:**
   An offline gap returning with memory age $\ge 3r$ ($p + 2r \le c$) finds the grace lapsed — prompt and memory co-expire in the same run. The grace is a fixed window of one retention period past the base rule; beyond it protection ends by design. This is the accepted semantics of the owner's ruling.
2. **Re-embed / Import NULL Residual:**
   Rows inserted via re-embedding batches (`migration_service.rs:761`) or legacy migration set `source_prompt_created_at: None`. Under the hybrid rule, NULL-stamp rows read a fresh full retention window based on `updated_at < cutoff`. A memory never dies early. Upgrading the frozen `SELECT_REEMBED_SOURCE` is out of scope.
