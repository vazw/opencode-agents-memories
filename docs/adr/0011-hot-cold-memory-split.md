# 0011 — Hot-Cold Memory Split: Tiered Storage by Access Frequency

- **Status:** Proposed (draft for review)
- **Date:** 2026-10-22
- **Wave:** Design (pre-W5.3, feeds issues 16, 17, 21, 24, 26, 31)
- **Supersedes:** nothing. **Amends:** ADR-0005 (episodic storage) and ADR-0010 (Sound Memory) — adds access-frequency-based storage tiering.
- **Source:** MemGPT/Letta (hierarchical paging), OS virtual memory, database buffer pools, Problem #7 (Context preservation), Problem #8 (Memory portability)

## Context

The rust port stores all memory in SQLite (turso). As memory grows (thousands of sessions, millions of facts), query performance degrades. The "Sound Memory" thesis (ADR-0010) says we must never destroy raw context. But we can **tier storage by access frequency** — keeping hot data in fast storage and cold data in slower/cheaper storage, without losing anything.

This is the same principle as:
- **OS virtual memory:** RAM (hot) → disk (cold) → swap (frozen)
- **Database buffer pools:** Hot pages in memory, cold on disk
- **MemGPT/Letta:** Core (hot) → Recall (warm) → Archival (cold)
- **CPU caches:** L1 (hot) → L2 → L3 → RAM (cold)

The key insight: **hot-cold splitting is about access patterns, not data destruction.** It aligns perfectly with ADR-0010's "scarcity of compression" — cold data is not compressed (preserved raw), just moved to slower storage.

## Decision

### D-1 — Four Storage Tiers by Access Frequency

| Tier | Name | Storage | Access Pattern | Latency | Content |
|------|------|---------|---------------|---------|---------|
| **Hot** | Working memory | In-memory (HashMap/cache) | Every query | <1ms | Current session context, active facts, recent profile |
| **Warm** | Recent memory | SQLite (turso, WAL mode) | Frequent reads | 1-10ms | Last 30 days of sessions, active facts, current profile |
| **Cold** | Archived memory | SQLite (separate `archive.db` or compressed) | Infrequent reads | 10-100ms | Older sessions, superseded facts, historical profile |
| **Frozen** | Immutable archive | Append-only file (JSONL/Parquet) | Rare reads | 100ms-1s | All raw context (never deleted), audit trail |

**Rationale:** MemGPT's hierarchical paging (core/recall/archival) demonstrates that tiered storage improves both performance and scalability. The "Sound Memory" thesis requires that cold/frozen tiers preserve raw context (no compression/destruction).

### D-2 — Hot Tier: In-Memory Cache

**What's hot:**
- Current session's episodic segments (active context)
- Frequently accessed facts (top-N by retrieval count)
- User profile (small, always needed)
- Recent session summaries (last 5-10 sessions)

**Implementation:**
- LRU cache in Rust (`lru` crate or custom)
- Cache key: `(project_id, memory_type, query_hash)`
- Cache invalidation: on write, on model upgrade, on TTL expiry
- Size limit: configurable (default 100MB)

**Eviction policy:** LRU (least recently used) — cold data moves to warm tier.

### D-3 — Warm Tier: SQLite (Current Storage)

**What's warm:**
- Last 30 days of episodic sessions
- Active facts (not superseded)
- Current profile
- Recent consolidation results

**Implementation:**
- Current `episodic.db` and facts store (no change)
- Add `last_accessed_at` column for access tracking
- Add `access_count` column for frequency tracking
- Index on `(project_id, last_accessed_at)` for efficient tiering

**Migration to cold:** Background task moves entries with `last_accessed_at < 30 days ago` and `access_count < threshold` to cold tier.

### D-4 — Cold Tier: Archived Storage

**What's cold:**
- Sessions older than 30 days (not accessed in 30 days)
- Superseded facts (replaced by newer facts)
- Historical profile versions
- Consolidation artifacts

**Implementation options:**
1. **Separate SQLite database** (`archive.db`) — same schema, different file
2. **Compressed SQLite** — SQLite with page-level compression
3. **Columnar format** (Parquet) — better for analytical queries

**Recommended:** Separate SQLite database (option 1) for simplicity. Same schema, same query interface. The only difference is the file path.

**Migration:** Background task (tokio) moves cold entries from warm to cold. Updates `tier` column. Preserves raw context.

### D-5 — Frozen Tier: Immutable Archive

**What's frozen:**
- All raw context (never deleted, even after consolidation)
- Audit trail (all memory operations)
- Historical snapshots (for debugging, compliance)

**Implementation:**
- Append-only JSONL file: `archive/{project_id}/frozen.jsonl`
- Each line: JSON entry with full raw context + metadata
- Never modified or deleted (append-only)
- Compressed with zstd for storage efficiency

**Rationale:** "Sound Memory" principle: raw context is never destroyed. Frozen tier is the ultimate backup — if everything else fails, raw context survives.

### D-6 — Automatic Tiering (Hot-Cold Migration)

**Migration rules:**

| From | To | Trigger | Condition |
|------|-----|---------|-----------|
| Hot → Warm | On eviction | LRU eviction | Cache full |
| Warm → Cold | Background task (hourly) | `last_accessed_at < 30 days` AND `access_count < 5` | Low access frequency |
| Cold → Warm | On access | Query hits cold tier | Re-accessed (promote back) |
| Any → Frozen | On consolidation | Consolidation completes | Raw context preserved |

**Anti-thrashing:** Require 2 consecutive accesses to promote cold → warm (prevent oscillation).

### D-7 — Query Routing

```
Query arrives
  → Check Hot cache (in-memory)
    → Hit: return immediately (<1ms)
    → Miss: Check Warm (SQLite)
      → Hit: return + promote to Hot cache
      → Miss: Check Cold (archive.db)
        → Hit: return + promote to Warm (if 2nd access)
        → Miss: Check Frozen (JSONL)
          → Hit: return + promote to Cold
          → Miss: not found
```

**Latency targets:**
- Hot: <1ms
- Warm: 1-10ms
- Cold: 10-100ms
- Frozen: 100ms-1s

### D-8 — Integration with ADR-0010 (Sound Memory)

The hot-cold split aligns with Sound Memory principles:
- **Raw context is never destroyed** — it moves to frozen tier, not deleted
- **Compression is derived** — cold tier may use compression, but raw context survives in frozen
- **Scarcity of compression** — cold tier uses compression sparingly (only for derived data)
- **Verification against raw** — frozen tier provides the ultimate verification source

### D-9 — Integration with ADR-0009 (Multi-Agent Memory)

Agent isolation applies to hot-cold tiers:
- **Hot cache:** Per-agent (each agent has its own hot cache)
- **Warm/Cold/Frozen:** Shared (with agent-scoped access control)
- **Promotion:** Agent-scoped writes to warm, async promotion to cold/frozen

## Consequences

### Positive

- **Performance:** Hot data in memory (<1ms), cold data on disk (10-100ms)
- **Scalability:** Memory can grow unbounded (frozen tier is append-only)
- **Cost efficiency:** Hot tier is small (100MB), cold/frozen are cheap storage
- **Sound Memory compliance:** Raw context never destroyed, just tiered
- **Graceful degradation:** If hot cache misses, warm/cold/frozen provide fallback

### Negative

- **Complexity:** 4 tiers + migration logic + query routing
- **Storage overhead:** Raw context in frozen tier + derived in warm/cold
- **Migration cost:** Background tasks for tiering
- **Testing:** Need to test all tier combinations

### Neutral

- **Backward compatible:** Existing data starts in warm tier, migrates based on access
- **Incremental adoption:** Can start with 2 tiers (hot + warm), add cold/frozen later
- **Aligns with existing ADRs:** ADR-0005 (episodic storage), ADR-0010 (Sound Memory)

## Implementation Priority

| Priority | Decision | Issue | Effort |
|----------|----------|-------|--------|
| 1 | D-1 (4-tier model) | 21 (W5.3) | Medium (schema) |
| 2 | D-2 (Hot cache) | 21 (W5.3) | Medium (LRU cache) |
| 3 | D-3 (Warm tier tracking) | 16/17 (consolidation) | Low (add columns) |
| 4 | D-4 (Cold tier) | 26 (W6.4) | Medium (archive.db) |
| 5 | D-5 (Frozen tier) | 26 (W6.4) | Medium (JSONL archive) |
| 6 | D-6 (Automatic tiering) | 31 (W6.7) | High (background tasks) |
| 7 | D-7 (Query routing) | 24 (W6.2) | Medium (routing logic) |

## Follow-ups

- **Issue 21 (W5.3):** Add `last_accessed_at`, `access_count`, `tier` columns to `agent_sessions`. Implement hot cache.
- **Issue 16/17 (consolidation):** Add warm→cold migration in consolidation pipeline. Track access patterns.
- **Issue 26 (W6.4):** Implement cold tier (archive.db) and frozen tier (JSONL). Add export/import for all tiers.
- **Issue 24 (W6.2):** Implement query routing (hot → warm → cold → frozen). Add tier-aware API endpoints.
- **Issue 31 (W6.7):** Build "memory tier dashboard" showing hot/warm/cold/frozen distribution.
- **ADR-0010 amendment:** Hot-cold split is the implementation of "scarcity of compression" — cold data is preserved raw, just tiered.
- **ADR-0005 amendment:** Add tiering columns and migration logic.

## References

- MemGPT/Letta: OS-inspired hierarchical paging (core/recall/archival)
- E-mem: Wang et al. 2026 — segment-preserving reconstruction
- SimpleMem: Liu et al. 2026 — semantic lossless compression
- Goyal et al. 2026 — format-dependent survival (KG > notes > embeddings)
- When Money Dies: Fergusson 1975 — Weimar hyperinflation (fiat debasement analogy)
- Broken Money: Alden 2023 — financial system failures
- ADR-0005: Episodic storage model
- ADR-0009: Multi-agent memory architecture
- ADR-0010: Sound Memory context preservation
- Open Problem #7: Context preservation vs compression
- Open Problem #8: Memory portability