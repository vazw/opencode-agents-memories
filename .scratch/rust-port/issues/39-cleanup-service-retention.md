# 39 — W8.1 · Make `POST /api/cleanup` real (retention cleanup)

Status: needs-triage

## What to build

`POST /api/cleanup` currently returns the documented 501 stub (D-15):
`pub async fn cleanup() -> Response { envelope::not_implemented(CLEANUP_UNAVAILABLE) }` — the handler
takes **no `State`**, so no port can reach it. Implement the retention-cleanup service behind
`MaintenancePort::cleanup()` and rewrite the handler to call it.

Also fix, in the same wave or explicitly defer with a reason: **`/api/migration/*` answers 500 where
it should answer 501.** `maintenance_failure` funnels `PortError::Unavailable` through
`port_failure` → `internal_error()` (500), while `sessions` correctly answers 501 for the same
condition (`session_failure`, `PortError::Unavailable => not_implemented`). Splitting the issue out of
37 surfaced this conflation; it is a mem-web defect, not a wiring gap.

## Upstream behaviour to match

`example/opencode-mem/src/services/cleanup-service.ts::runCleanup` (read-only reference):

1. `cutoff = now - autoCleanupRetentionDays`.
2. Collect `pinnedMemoryIds` across **all** user and project shards (`is_pinned = 1`).
3. `promptCleanupResult = userPromptManager.deleteOldPrompts(cutoff)` → returns
   `{ deleted, linkedMemoryIds }`.
4. `protected = pinned ∪ linkedMemoryIds`.
5. Per shard: `SELECT id, container_tag, is_pinned FROM memories WHERE updated_at < cutoff`; skip
   pinned, skip protected; else delete the vector and `decrementVectorCount`; count user vs project
   by `container_tag` containing `_user_` / `_project_`.
6. `VACUUM` the user-prompts DB; count AI sessions cleaned (may be N/A in the port — decide).
7. Answer `CleanupReport { deletedCount, userCount, projectCount, … }`.

## Measured surface (2026-10-25, HEAD `065ffdb`)

| piece | state |
|---|---|
| `MaintenancePort::cleanup()` | exists, default body `Err(PortError::Unavailable)` (`crates/mem-web/src/api/ports.rs`) |
| `CleanupReport` | `{ deleted_count: u64, user_count: u64, project_count: u64 }` — **3 fields**, upstream returns 7 (`promptsDeleted`, `linkedMemoriesDeleted`, `pinnedMemoriesSkipped`, `aiSessionsDeleted` dropped) |
| `UserPromptManager::delete_old_prompts` | exists but returns **`u64` only** — it does not read `linked_memory_id`, so the prompt-linked protection set is invisible (`crates/mem-profile/src/prompt.rs`) |
| `user_prompts.linked_memory_id` | **column exists** (`crates/mem-store/src/sql.rs:53`, index at `:132`; DTOs already carry `Option<String>`) |
| `UserPromptManager::vacuum` | exists |
| `ShardManager::decrement_vector_count` | exists |
| vector deletion | `mem_store::vector::delete_memory_by_id(db, memory_id)` |
| config | `auto_cleanup_enabled`, `auto_cleanup_retention_days` both already parsed (`crates/mem-core/src/config/schema.rs`) — **no new config** |

## Open decisions (grill before planning)

1. **Protection set.** Pinned-only, or pinned ∪ prompt-linked? Upstream protects prompt-linked
   memories; taking the pinned-only route silently deletes rows a prompt still references. If
   protected, `delete_old_prompts` needs a variant that returns the linked ids (or a second query).
2. **Contract width.** Keep `CleanupReport`'s 3 fields, or widen to upstream's 7? The port contract is
   already published in `crates/mem-web/src/api/ports.rs`, and the UI may read it.
3. **`VACUUM` and AI sessions.** In scope, or explicitly dropped with a reason? `vacuum()` exists;
   there is no AI-session store equivalent in the port.
4. **Manual-only or scheduled.** Upstream gates on `auto_cleanup_enabled` + a daily
   `shouldRunCleanup()`. Does this wave wire only the manual endpoint, or also the scheduler?
5. **Deletion semantics across shards.** Confirm per-shard iteration matches upstream's
   "all user + all project shards", and what happens to a shard whose DB errors mid-run (upstream
   logs and continues).

## Acceptance criteria (draft — finalise after the grill)

- [ ] `POST /api/cleanup` answers 200 with real counts on a seeded store; no `unwrap`/`expect` on the
      path (ADR-0013).
- [ ] An expired, unpinned, unlinked memory is deleted; a pinned one survives; a prompt-linked one
      survives (or the chosen narrower rule is stated and tested).
- [ ] Vector count decremented per deletion; no orphan vectors remain.
- [ ] `/api/migration/*` answers **501** (not 500) when the maintenance service is unavailable.
- [ ] Degrade path unchanged: with no composition, the endpoint still answers the documented 501.
- [ ] No new dependencies; production panic-free.

## Comments

- Split out of issue 37 on 2026-10-25. 37 had AC6 = "wire sessions **and** maintenance"; measurement
  showed `cleanup()`/`deduplicate()` take no `State` and are unconditional stubs, so wiring a port
  cannot make them real — the work is service implementation, not composition.
- Sibling issue 40 covers `POST /api/deduplicate`, which shares the shard/vector-count machinery but
  has its own deletion rule.
