# 40 — W8.2 · Make `POST /api/deduplicate` real (memory deduplication)

Status: needs-triage

## What to build

`POST /api/deduplicate` currently returns the documented 501 stub (D-15):
`pub async fn deduplicate() -> Response { envelope::not_implemented(DEDUPLICATE_UNAVAILABLE) }` —
no `State`, so no port can reach it. Implement the memory-deduplication service behind
`MaintenancePort::deduplicate()` and rewrite the handler to call it.

## Upstream behaviour to match

`example/opencode-mem/src/services/deduplication-service.ts::detectAndRemoveDuplicates` (read-only
reference):

1. Refuse to run when `CONFIG.deduplicationEnabled` is false (upstream **throws**; decide whether the
   port answers `PortError::Invalid` → 200 `success:false`, or 501 — not a 500).
2. Per shard, load all memories with their extracted vectors.
3. Group by the key `container_tag + ":" + content`; a group with >1 member is an **exact** duplicate
   set.
4. Within each group, sort by `created_at` **descending** and delete everything except the newest,
   decrementing the vector count per deletion.
5. Near-duplicate grouping via `cosineSimilarity`, reported as
   `{ representative: { id, content, container_tag, created_at }, duplicates: [{ id, content,
   similarity }] }` — **reported, not merged**.
6. Answer `{ exactDuplicatesDeleted, nearDuplicateGroups }`.

## Measured surface (2026-10-25, HEAD `065ffdb`)

| piece | state |
|---|---|
| `MaintenancePort::deduplicate()` | exists, default body `Err(PortError::Unavailable)` (`crates/mem-web/src/api/ports.rs`) |
| `DedupReport` | `{ exact_duplicates_deleted: u64, near_duplicate_groups: Vec<Value> }` — the groups are **untyped `Value`**, so either a DTO or a documented JSON shape is required |
| `mem_core::dedup` | **must NOT be wired.** Its own module doc says it is the *profile-conflict* grouping kernel (upstream `user-profile-manager.ts::detectConflicts`, the `cos >= 0.65 && cos < 0.9` band plus an exact `(category, description)` rule) and that "it must not be wired here". Memory dedup is a different service with a different key. |
| cosine | `mem_core::utils::math::cosine_similarity` exists (used by `mem_core::dedup`) |
| vector deletion | `mem_store::vector::delete_memory_by_id(db, memory_id)` |
| `ShardManager::decrement_vector_count` | exists |
| config | `deduplication_enabled` **and** `deduplication_similarity_threshold` both already parsed (`crates/mem-core/src/config/schema.rs`) — **no new config** |

## Open decisions (grill before planning)

1. **Near-duplicate threshold.** Upstream uses `deduplicationSimilarityThreshold` for the band while
   reporting groups. Confirm the exact comparison (a group per representative above the threshold?
   transitive closure?) — reading `deduplication-service.ts` past the exact-duplicate block is
   required before planning, since this issue only summarises it.
2. **`Vec<Value>` vs typed DTO.** `DedupReport.near_duplicate_groups` is untyped today. A typed DTO is
   better for the UI and for tests; changing it is a port-contract change.
3. **Disabled-config posture.** `PortError::Invalid` → HTTP 200 `success:false` (the handler's
   documented business-error shape) vs a 501. Upstream throws.
4. **Scope of "keep newest".** Upstream sorts by `created_at` desc. Tie-breaking (equal timestamps)
   must be deterministic, or the endpoint is non-idempotent.
5. **Cross-shard duplicates.** Upstream dedups **within** each shard. Confirm that is the intended
   contract — a memory present in two shards is not a duplicate under this rule.

## Acceptance criteria (draft — finalise after the grill)

- [ ] `POST /api/deduplicate` answers 200 with real counts on a seeded store; no `unwrap`/`expect`
      on the path (ADR-0013).
- [ ] Two identical `(container_tag, content)` rows → the newer survives, the older is deleted, count
      is 1; the vector count decremented; no orphan vector.
- [ ] Near-duplicate groups are reported and **nothing is merged or deleted** on that path.
- [ ] Deterministic tie-breaking on equal `created_at` (proven by a test with equal timestamps).
- [ ] `deduplication_enabled = false` produces the chosen documented posture, tested.
- [ ] No new dependencies; production panic-free.

## Comments

- Split out of issue 37 on 2026-10-25, alongside issue 39 (cleanup). Both were AC6 of 37 until
  measurement showed the maintenance handlers are stateless stubs.
- The `mem_core::dedup` warning is the main trap: the name matches, the semantics do not. Reusing it
  would dedup profile items and silently leave memory duplicates in place.
