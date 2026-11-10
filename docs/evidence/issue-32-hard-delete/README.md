# Issue 32 — Hard Delete & Milestone Scrub Smoke Evidence

## Overview

This document records the smoke evidence and residual analysis for **Issue 32 (W4.4 · Hard delete & milestone scrub)**, closing the documentation gap in AC4.

## Acceptance Criteria Verification

### AC1 — Hard delete of session row (text + embedding)
- **Path**: `EpisodicStore::hard_delete_session` in `crates/mem-episodic/src/purge.rs`.
- **Proof**: `crates/mem-episodic/tests/hard_delete.rs`.
  - Direct store query: `SELECT id, summary, length(embedding) FROM agent_sessions WHERE id = ?` returns 0 rows.
  - Context & segments: `raw_context_for_session` returns `None`, `segments_for_session` returns empty, and no `capture_inbox` row remains.

### AC2 — Milestone contributor scrub with preserved attribution
- **Path**: `SCRUB_MILESTONE_CONTRIBUTORS` batch update in `crates/mem-episodic/src/purge.rs`.
- **Ruling**: Owner ruling 2026-10-27 (amending issue 43 decision 9). Contributor entry is retained with `summary == ""` while attribution (`agent_id`, `agent_name`, `created_at`) is preserved intact.
- **Proof**: `crates/mem-episodic/tests/hard_delete.rs`.
  - Consolidated recap columns (`summary`, `embedding`, `source_count`, `window_start`, `window_end`) are asserted byte-identical across the purge (no re-summarization, no re-embed).
  - Co-contributors' summaries in the same milestone remain unmodified.
  - Report field `milestones_scrubbed` accurately reflects rewritten milestones.

### AC3 — Child orphan semantics under foreign key delete
- **Behavior**: `agent_sessions.parent_session_id` foreign key defines `ON DELETE SET NULL`.
- **Proof**: `test_hard_delete_orphans_children_and_the_fk_nulls_the_link` in `crates/mem-episodic/tests/hard_delete.rs` and `test_sessions_delete_is_orphan_safe` in `crates/mem-web/tests/sessions.rs`.
  - Child rows survive with `orphaned == 1` and `parent_session_id IS NULL`.

## Residuals Status

1. **Residual 1 (Milestone recap & embedding)**: SETTLED (Owner, 2026-10-21). Merged recap and embedding are retained. Named in CLI report `not_erased()` under `milestones-recap`.
2. **Residual 2 (Tier-1 promoted facts)**: SETTLED (Owner, 2026-10-21). Promoted durable facts are kept. The purge passes an empty milestone-id list to `purge_sources`, tested by `test_purge_keeps_a_fact_sourced_only_by_a_surviving_milestone`.
3. **Residual 3 (Delete response copy)**: SETTLED. Stated in CLI and API documentation that deleting a session does not erase consolidated recap summaries or promoted durable facts.
4. **Residual 4 (Orphan semantics under hard delete)**: SETTLED. Proven by AC3 tests.
5. **Residual 5 (Age threshold & consolidation interaction)**: OPEN. Bounded staging row retention threshold governed by issue 39/41 retention framework.
6. **Residual 6 (Stale vectors after compaction)**: OPEN. Documented limitation; compaction strips raw text without re-embedding episodic rows.
