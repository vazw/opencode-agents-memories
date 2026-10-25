# 40 — W8.2 · Make `POST /api/deduplicate` real (memory deduplication)

Status: ready-for-agent — 2026-10-25 (grilled; decisions below are closed. Blueprint may proceed
after issue 39 lands, since both mutate shard vectors.)

## What to build

`POST /api/deduplicate` is today an unconditional 501 stub taking no `State`:
`pub async fn deduplicate() -> Response { envelope::not_implemented(DEDUPLICATE_UNAVAILABLE) }`.
Implement memory deduplication behind `MaintenancePort::deduplicate()` and rewrite the handler.

## Decisions taken (do not re-open)

1. **Replicate upstream's greedy near-duplicate scan exactly**, with two hardening changes that do
   not alter grouping: a **typed DTO** instead of `Vec<Value>`, and a **deterministic order**
   (`created_at DESC, id ASC`). Rationale: near-duplicate output is user-visible, so a transitive
   "improvement" would silently disagree with upstream on the same data; but upstream's iteration
   order is an accident of a `SELECT` with no `ORDER BY`, and an unspecified order is an untestable
   spec.
2. **Disabled posture:** `deduplication_enabled = false` ⇒ `PortError::Invalid` ⇒ HTTP **200**
   `success:false` (the handler's documented business-error shape). Upstream throws and the handler
   catches it into the same envelope. Not 501 — that would conflate "disabled" with "not in this
   build".
3. **Within each shard, same `container_tag` only.** A memory present in two shards is not a
   duplicate. Upstream-identical.
4. **Exact-dedup tie-break is deterministic:** `created_at DESC, id ASC`. Upstream relies on a stable
   sort; the port must not.
5. **Near-duplicate groups are reported, never merged or deleted.** Upstream's near path performs no
   mutation; keep it that way.
6. **Run guard:** `AtomicBool`, mirroring issue 39's cleanup guard. Two concurrent runs would
   otherwise double-count.

## Upstream behaviour (`example/opencode-mem/src/services/deduplication-service.ts`)

1. Refuse when `CONFIG.deduplicationEnabled` is false.
2. Per shard: load all memories with their extracted vectors.
3. **Exact:** group by `container_tag + ":" + content`; a group with >1 member is a duplicate set;
   sort **newest first** and delete everything except the newest, decrementing the vector count per
   deletion. (The sort mutates the group array in place, so the representative that survives *is* the
   newest — consistent with the near-duplicate pass below.)
4. **Near-duplicate:** over one survivor per exact group, greedy O(n²) — for each unprocessed memory
   as representative, scan later ones; same `container_tag` and `similarity >= threshold &&
   similarity < 1.0` ⇒ append `{id, content, similarity}` and mark processed.
5. Answer `{ exactDuplicatesDeleted, nearDuplicateGroups }`.

`similarity < 1.0` matters: identical vectors are already removed by the exact pass.

## Measured surface (2026-10-25, HEAD `9f4853b`; re-measure before planning)

| piece | state |
|---|---|
| `MaintenancePort::deduplicate()` | exists, default body `Err(PortError::Unavailable)` (`crates/mem-web/src/api/ports.rs`) |
| `DedupReport` | `{ exact_duplicates_deleted: u64, near_duplicate_groups: Vec<Value> }` — groups to become a typed DTO |
| `mem_core::dedup` | **must NOT be wired.** Its module doc states it is the *profile-conflict* kernel (upstream `user-profile-manager.ts::detectConflicts`, band `cos >= 0.65 && cos < 0.9`, plus an exact `(category, description)` rule) and that it must not be wired here. Name matches, semantics do not. |
| cosine | `mem_core::utils::math::cosine_similarity` |
| vector deletion | `mem_store::vector::delete_memory_by_id(db, memory_id)` |
| count decrement | `ShardManager::decrement_vector_count(shard_id)` |
| shard enumeration | `ShardManager::get_all_shards(scope, Option<&str>)`, `None` = wildcard |
| config | `deduplication_enabled`, `deduplication_similarity_threshold` (default `0.9`) both already parsed — **no new config** |

## Acceptance criteria

- [ ] `POST /api/deduplicate` answers 200 on a seeded store; no `unwrap`/`expect` on the path
      (ADR-0013); degrade path still 501 with no composition.
- [ ] Two identical `(container_tag, content)` rows ⇒ the **newer** survives, count is 1,
      `exact_duplicates_deleted == 1`, vector count decremented once, no orphan vector.
- [ ] Equal `created_at` ⇒ deterministic survivor (`id ASC` wins the tie), proven by a test with two
      equal timestamps.
- [ ] Near-duplicate pairs above the threshold are **reported** with their similarity and the group's
      representative; nothing is deleted or merged on that path (assert row counts unchanged).
- [ ] A pair with `similarity == 1.0` remaining after the exact pass is **excluded** from near groups
      (`< 1.0` bound).
- [ ] Memories with different `container_tag` never group together.
- [ ] `deduplication_enabled = false` ⇒ 200 `success:false`, nothing deleted.
- [ ] Two concurrent runs cannot double-delete or double-count (guard proven, not asserted).
- [ ] No new dependencies; production panic-free.

## Test plan (seed directly; no dependency on issue 39)

`crates/mem-web/tests/` dedup integration (real shard via `memory_shard_migrations`, registry via
`ShardManager`, rows inserted with known vectors):

1. exact duplicate set of 3 ⇒ newest survives, 2 deleted, count 2.
2. exact duplicates with equal `created_at` ⇒ `id ASC` survivor, deterministic across runs.
3. near-duplicate pair (`similarity ≈ 0.95`) ⇒ one group, representative = newer, duplicate listed
   with its similarity, **zero deletions**.
4. pair at exactly `1.0` ⇒ no near group.
5. pair below threshold ⇒ no group.
6. cross-tag pair at high similarity ⇒ no group.
7. `deduplication_enabled = false` ⇒ 200 `success:false`, row count unchanged.
8. degrade port ⇒ 501.

## Out of scope

- Merging near-duplicates (upstream never merges).
- Cross-shard duplicate detection.
- Any reuse of `mem_core::dedup`.
- Scheduled deduplication (upstream has none).

## Comments

- Split out of issue 37 on 2026-10-25 with issue 39, after measurement showed `deduplicate()` takes no
  `State` and is an unconditional 501 stub.
- The `mem_core::dedup` trap is the main hazard: same name, different tier, and its own documentation
  forbids the wiring.
- Sequenced **after** issue 39: both walk every shard and mutate vectors, so running them as one wave
  would put two deletion surfaces in one diff.
